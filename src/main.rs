use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use fs2::FileExt;
use signal_hook::{
    consts::signal::{SIGINT, SIGTERM},
    iterator::Signals,
};
use std::{
    env,
    fs::File,
    io::{self, Write},
    os::unix::io::AsRawFd,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use termios::{os::linux::ECHOCTL, *};

mod config;
mod constants;
mod backend;
mod logger;
mod startup_transition;
mod time_state;
mod utils;

use config::Config;
use constants::*;
use backend::{detect_backend, create_backend};
use logger::Log;
use startup_transition::StartupTransition;
use time_state::{TimeState, TransitionState, get_transition_state, time_until_next_event};

/// Automatic color temperature controller for Wayland compositors
#[derive(Parser)]
#[command(author, version, about, long_about = None)]
#[command(help_template = "\
{before-help}{name} {version}
{about-with-newline}
{usage-heading} {usage}

{all-args}{after-help}
")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Show version information
    #[command(visible_alias = "v")]
    Version,
    /// Show help information
    #[command(visible_alias = "h")]
    Help,
}

fn print_help() {
    println!("sunsetr v{}", env!("CARGO_PKG_VERSION"));
    println!("Automatic color temperature controller for Wayland compositors");
    println!();
    println!("USAGE:");
    println!("    sunsetr [COMMAND]");
    println!();
    println!("COMMANDS:");
    println!("    help, -h, --help       Show this help message");
    println!("    version, -v, --version Show version information");
    println!();
    println!("CONFIG:");
    println!("    Configuration file is automatically created in:");
    println!("    $HOME/.config/sunsetr/sunsetr.toml");
    println!();
    println!("    Legacy location is also supported:");
    println!("    $HOME/.config/hypr/sunsetr.toml");
}

fn print_version() {
    println!("sunsetr v{}", env!("CARGO_PKG_VERSION"));
}

fn handle_invalid_argument(arg: &str) {
    eprintln!("error: unknown argument '{}'", arg);
    eprintln!();
    print_help();
}

// Constants
const CHECK_INTERVAL: Duration = Duration::from_secs(CHECK_INTERVAL_SECS);

/// Manages terminal state to hide cursor and suppress control character echoing.
///
/// This struct automatically restores the original terminal state when dropped,
/// ensuring clean cleanup even if the program exits unexpectedly.
struct TerminalGuard {
    original_termios: Termios,
}

impl TerminalGuard {
    /// Create a new terminal guard and modify terminal settings.
    ///
    /// Sets up the terminal to:
    /// - Hide the cursor for cleaner output
    /// - Suppress echoing of control characters like ^C
    ///
    /// # Returns
    /// - `Ok(Some(guard))` if terminal is available and settings were applied
    /// - `Ok(None)` if no terminal is available (e.g., running as a service)
    /// - `Err` only for unexpected errors
    fn new() -> io::Result<Option<Self>> {
        // Try to open the controlling tty - if it fails, we're likely running headless
        let tty = match File::open("/dev/tty") {
            Ok(tty) => tty,
            Err(e) if e.kind() == io::ErrorKind::NotFound || e.raw_os_error() == Some(6) => {
                // No controlling terminal (common in systemd services) - this is not an error
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        
        let fd = tty.as_raw_fd();

        // Take a snapshot of the current settings for restoration
        let mut term = Termios::from_fd(fd)?;
        let original = term;

        // Disable the "^C" echo to prevent visual noise during shutdown
        term.c_lflag &= !ECHOCTL;
        tcsetattr(fd, TCSANOW, &term)?;

        // Hide the cursor for cleaner output display
        print!("\x1b[?25l");
        io::stdout().flush()?; // always flush control sequences

        Ok(Some(Self {
            original_termios: original,
        }))
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        // Best-effort restore of termios + cursor visibility
        if let Ok(tty) = File::open("/dev/tty") {
            let _ = tcsetattr(tty.as_raw_fd(), TCSANOW, &self.original_termios);
        }
        let _ = write!(io::stdout(), "\x1b[?25h");
        let _ = io::stdout().flush();
    }
}

/// Perform cleanup operations when shutting down the application.
///
/// This function handles:
/// - Backend-specific cleanup (stopping managed processes)
/// - Releasing the lock file
/// - Removing the lock file from disk
///
/// # Arguments
/// * `backend` - The backend instance to clean up
/// * `lock_file` - File handle for the application lock
/// * `lock_path` - Path to the lock file for removal
fn cleanup(backend: Box<dyn crate::backend::ColorTemperatureBackend>, lock_file: File, lock_path: &str) {
    Log::log_decorated("Performing cleanup...");

    // Handle backend-specific cleanup
    backend.cleanup();

    // Drop the lock file handle to release the lock
    drop(lock_file);

    // Remove the lock file from disk
    if let Err(e) = std::fs::remove_file(lock_path) {
        Log::log_decorated(&format!("Warning: Failed to remove lock file: {}", e));
    } else {
        Log::log_decorated("Lock file removed successfully");
    }

    Log::log_decorated("Cleanup complete");
}

/// Determine whether the application state should be updated.
///
/// This function implements the logic for deciding when to apply state changes
/// to the backend. It considers:
/// - Transition start/end detection
/// - Progress during ongoing transitions  
/// - State changes between stable periods
/// - Time jump detection (system sleep/resume)
///
/// # Arguments
/// * `current_state` - The last known transition state
/// * `new_state` - The newly calculated transition state
/// * `time_since_last_check` - Duration since last state check
///
/// # Returns
/// `true` if the state should be updated, `false` to skip this update cycle
fn should_update_state(
    current_state: &TransitionState,
    new_state: &TransitionState,
    time_since_last_check: Duration,
) -> bool {
    match (current_state, new_state) {
        // Detect entering a transition (from stable to transitioning)
        (TransitionState::Stable(_), TransitionState::Transitioning { progress, from, to })
            if *progress < 0.01 =>
        {
            Log::log_pipe();
            let transition_type = match (from, to) {
                (TimeState::Day, TimeState::Night) => "sunset 󰖛 ",
                (TimeState::Night, TimeState::Day) => "sunrise 󰖜 ",
                _ => "transition",
            };
            Log::log_decorated(&format!("Commencing {}", transition_type));
            true
        }
        // Detect completing a transition (progress near 100%)
        (
            TransitionState::Transitioning {
                progress: prev_progress,
                ..
            },
            TransitionState::Transitioning {
                progress: curr_progress,
                from,
                to,
            },
        ) if *prev_progress < 0.99 && *curr_progress >= 0.99 => {
            Log::log_pipe();
            let transition_type = match (from, to) {
                (TimeState::Day, TimeState::Night) => "sunset 󰖛 ",
                (TimeState::Night, TimeState::Day) => "sunrise 󰖜 ",
                _ => "transition",
            };
            Log::log_decorated(&format!("Completed {}", transition_type));
            true
        }
        // Detect change from transitioning to stable state
        (TransitionState::Transitioning { .. }, TransitionState::Stable(_)) => true,
        // Detect change from one stable state to another (should be rare)
        (TransitionState::Stable(prev), TransitionState::Stable(curr)) if prev != curr => {
            Log::log_block_start(&format!("State changed from {:?} to {:?}", prev, curr));
            true
        }
        // We're in a transition and it's time for a regular update
        (TransitionState::Transitioning { .. }, TransitionState::Transitioning { .. }) => true,
        // Large time jump detected - force update to handle system sleep/resume
        _ if time_since_last_check > Duration::from_secs(SLEEP_DETECTION_THRESHOLD_SECS) => {
            Log::log_decorated("Applying state due to time jump detection");
            true
        }
        _ => false,
    }
}

fn main() -> Result<()> {
    // Parse command-line arguments
    let args: Vec<String> = env::args().collect();
    
    // Handle help and version flags
    if args.len() == 2 {
        let arg = &args[1];
        match arg.as_str() {
            "-h" | "--help" | "help" => {
                print_help();
                return Ok(());
            }
            "-v" | "--version" | "version" => {
                print_version();
                return Ok(());
            }
            _ => {
                // Check if it's an invalid argument (starts with -)
                if arg.starts_with('-') {
                    handle_invalid_argument(arg);
                    std::process::exit(1);
                }
                // If it doesn't start with -, treat it as an invalid non-flag argument
                handle_invalid_argument(arg);
                std::process::exit(1);
            }
        }
    } else if args.len() > 2 {
        // Multiple arguments - invalid
        let invalid_args = args[1..].join(" ");
        handle_invalid_argument(&invalid_args);
        std::process::exit(1);
    }
    // If args.len() == 1, just run normally (no arguments provided)

    // Try to set up terminal features (cursor hiding, echo suppression)
    // This will gracefully handle cases where no terminal is available (e.g., systemd service)
    let _term = TerminalGuard::new()
        .context("failed to initialize terminal features")?;

    Log::log_version();

    // Set up signal handling
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    let mut signals = Signals::new([SIGTERM, SIGINT])?;
    thread::spawn(move || {
        for signal in signals.forever() {
            Log::log_pipe();
            Log::log_info(&format!("Shutdown signal received: {:?}", signal));
            r.store(false, Ordering::SeqCst);
            Log::log_info("Set running flag to false");
        }
    });

    // Create lock file path
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    let lock_path = format!("{}/sunsetr.lock", runtime_dir);

    // Quick check if another instance is already running before doing expensive config validation
    if let Ok(existing_lock) = File::open(&lock_path) {
        if existing_lock.try_lock_exclusive().is_err() {
            Log::log_error("Another instance of sunsetr is already running");
            std::process::exit(1);
        }
        // Lock succeeded, but we opened for reading - close it and create properly below
        drop(existing_lock);
    }

    // Load and validate configuration now that we know no other instance is running
    let config = Config::load()?;
    
    // Detect and validate the backend early
    let backend_type = detect_backend(&config)?;

    // Create and acquire lock file properly
    let lock_file = File::create(&lock_path)?;

    // Try to acquire exclusive lock (should succeed since we checked above, but race conditions possible)
    match lock_file.try_lock_exclusive() {
        Ok(_) => {
            Log::log_decorated("Lock acquired, starting sunsetr...");

            // Log configuration after acquiring lock
            config.log_config();

            Log::log_decorated(&format!("Detected backend: {}", backend_type.name()));
            
            let mut backend = create_backend(backend_type, &config)?;
            
            // Test backend connection
            if !backend.test_connection() {
                anyhow::bail!(
                    "Failed to connect to {} backend. Please ensure the compositor is running and supports the required protocols.",
                    backend.backend_name()
                );
            }
            
            Log::log_decorated(&format!("Successfully connected to {} backend", backend.backend_name()));

            let mut current_transition_state = get_transition_state(&config);
            let mut last_check_time = Instant::now();

            // Apply initial settings
            if running.load(Ordering::SeqCst) {
                // Check if startup transition is enabled
                let startup_transition = config
                    .startup_transition
                    .unwrap_or(DEFAULT_STARTUP_TRANSITION);
                let startup_duration = config
                    .startup_transition_duration
                    .unwrap_or(DEFAULT_STARTUP_TRANSITION_DURATION);

                if startup_transition && startup_duration > 0 {
                    // Use the smooth transition system (from day values to current state)

                    // Create and execute the startup transition
                    let mut transition = StartupTransition::new(current_transition_state, &config);
                    match transition.execute(backend.as_mut(), &config, &running) {
                        Ok(_) => {}
                        Err(e) => {
                            Log::log_warning(&format!(
                                "Failed to apply smooth startup transition: {}",
                                e
                            ));
                            Log::log_decorated("Falling back to immediate transition...");

                            // Fallback to immediate application
                            match backend.apply_startup_state(
                                current_transition_state,
                                &config,
                                &running,
                            ) {
                                Ok(_) => {
                                    Log::log_block_start(
                                        "Initial state applied successfully (fallback)",
                                    );
                                }
                                Err(e) => {
                                    Log::log_warning(&format!(
                                        "Failed to apply initial state: {}",
                                        e
                                    ));
                                    Log::log_decorated(
                                        "Continuing anyway - will retry during operation...",
                                    );
                                }
                            }
                        }
                    }
                } else {
                    // Use immediate transition to current interpolated values
                    match backend.apply_startup_state(current_transition_state, &config, &running) {
                        Ok(_) => {
                            Log::log_block_start("Initial state applied successfully");
                        }
                        Err(e) => {
                            Log::log_warning(&format!("Failed to apply initial state: {}", e));
                            Log::log_decorated(
                                "Continuing anyway - will retry during operation...",
                            );
                        }
                    }
                }
            }

            // Main loop with transition support and sleep/resume detection
            // Skip first iteration to prevent false state change detection due to startup timing
            let mut first_iteration = true;
            while running.load(Ordering::SeqCst) {
                // Detect large time jumps (system sleep/resume scenarios)
                let current_time = Instant::now();
                let time_since_last_check = current_time.duration_since(last_check_time);
                if time_since_last_check > Duration::from_secs(SLEEP_DETECTION_THRESHOLD_SECS) {
                    // 5+ minutes
                    Log::log_decorated(&format!(
                        "Large time jump detected ({} minutes). System may have resumed from sleep.",
                        time_since_last_check.as_secs() / 60
                    ));
                    Log::log_decorated("Forcing immediate state recalculation...");
                }
                last_check_time = current_time;

                let new_state = get_transition_state(&config);

                // Skip first iteration to prevent false state change detection caused by
                // timing differences between startup state application and main loop start
                let should_update = if first_iteration {
                    first_iteration = false;
                    false
                } else {
                    should_update_state(
                        &current_transition_state,
                        &new_state,
                        time_since_last_check,
                    )
                };

                if should_update && running.load(Ordering::SeqCst) {
                    match backend.apply_transition_state(new_state, &config, &running) {
                        Ok(_) => {
                            // Success - update our state
                            current_transition_state = new_state;
                            // Only log completion for non-transition states or specific transition points
                            // which are already handled in the should_update match above
                        }
                        Err(e) => {
                            // Failure - check if it's a connection issue that couldn't be resolved
                            if e.to_string().contains("reconnection attempt") {
                                Log::log_error(&format!(
                                    "Cannot communicate with {}: {}",
                                    backend.backend_name(), e
                                ));
                                Log::log_decorated(
                                    &format!("{} appears to be permanently unavailable. Exiting...", backend.backend_name())
                                );
                                break; // Exit the main loop
                            } else {
                                // Other error - just log it and retry next cycle
                                Log::log_warning(&format!("Failed to apply state: {}", e));
                                Log::log_decorated("Will retry on next cycle...");
                            }
                            // Don't update current_transition_state - try again next cycle
                        }
                    }
                }

                // Determine sleep duration based on state (simple, no special failure handling)
                let sleep_duration = match new_state {
                    TransitionState::Transitioning { .. } => Duration::from_secs(
                        config.update_interval.unwrap_or(DEFAULT_UPDATE_INTERVAL),
                    ),
                    TransitionState::Stable(_) => time_until_next_event(&config),
                };

                // Show next update timing with more context
                match new_state {
                    TransitionState::Transitioning { progress, .. } => {
                        Log::log_decorated(&format!(
                            "Transition {}% complete. Next update in {} seconds",
                            (progress * 100.0) as u8,
                            sleep_duration.as_secs()
                        ));
                    }
                    TransitionState::Stable(_) => {
                        Log::log_block_start(&format!(
                            "Next transition in {} minutes {} seconds",
                            sleep_duration.as_secs() / 60,
                            sleep_duration.as_secs() % 60
                        ));
                    }
                }

                // Sleep in smaller intervals to check running status
                let mut slept = Duration::from_secs(0);
                while slept < sleep_duration && running.load(Ordering::SeqCst) {
                    let sleep_chunk = CHECK_INTERVAL.min(sleep_duration - slept);
                    thread::sleep(sleep_chunk);
                    slept += sleep_chunk;
                }
            }

            // Ensure proper cleanup on shutdown
            Log::log_block_start("Shutting down sunsetr...");
            cleanup(backend, lock_file, &lock_path);
            Log::log_end();
        }
        Err(_) => {
            Log::log_error("Another instance of sunsetr is already running");
            std::process::exit(1);
        }
    }

    Ok(())
}
