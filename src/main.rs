use anyhow::{Context, Result};
use chrono::{Local, NaiveTime, Timelike};
use fs2::FileExt;
use serde::Deserialize;
use signal_hook::{
    consts::signal::{SIGINT, SIGTERM},
    iterator::Signals,
};
use std::fs::{self, File};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[derive(Debug, Deserialize)]
struct Config {
    sunset: String,
    sunrise: String,
    temp: Option<u32>,
}

impl Config {
    fn get_config_path() -> Result<PathBuf> {
        dirs::config_dir()
            .map(|p| p.join("hypr").join("sunsetr.toml"))
            .context("Could not determine config directory")
    }

    fn create_default_config(path: &PathBuf) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).context("Failed to create config directory")?;
        }

        let default_config = r#"# Sunsetr configuration
sunset = "19:00:00"   # Time to transition to night mode (HH:MM:SS)
sunrise = "06:00:00"  # Time to transition to day mode (HH:MM:SS)
temp = 4000           # Color temperature after sunset (1000-6000) Kelvin"#;

        fs::write(path, default_config).context("Failed to write default config file")?;
        println!("Created default configuration file at {:?}", path);
        Ok(())
    }

    fn load() -> Result<Self> {
        let config_path = Self::get_config_path()?;

        if !config_path.exists() {
            Self::create_default_config(&config_path)?;
        }

        let content = fs::read_to_string(&config_path).context("Failed to read sunsetr.toml")?;

        let config: Config = toml::from_str(&content).context("Failed to parse config file")?;

        // Validate time formats
        NaiveTime::parse_from_str(&config.sunset, "%H:%M:%S")
            .context("Invalid sunset time format in config. Use HH:MM:SS format")?;
        NaiveTime::parse_from_str(&config.sunrise, "%H:%M:%S")
            .context("Invalid sunrise time format in config. Use HH:MM:SS format")?;

        // Validate temperature if specified
        if let Some(temp) = config.temp {
            if !(1000..=6000).contains(&temp) {
                anyhow::bail!("Temperature must be between 1000 and 6000 Kelvin");
            }
        }

        Ok(config)
    }
}

#[derive(Debug, PartialEq)]
enum TimeState {
    Day,
    Night,
}

fn get_current_state(config: &Config) -> TimeState {
    let now = Local::now().time();
    let sunset = NaiveTime::parse_from_str(&config.sunset, "%H:%M:%S").unwrap();
    let sunrise = NaiveTime::parse_from_str(&config.sunrise, "%H:%M:%S").unwrap();

    if (sunset < sunrise && (now >= sunset && now < sunrise))
        || (sunset >= sunrise && (now >= sunset || now < sunrise))
    {
        TimeState::Night
    } else {
        TimeState::Day
    }
}

fn time_until_next_event(config: &Config) -> Duration {
    let now = Local::now();
    let current_time = now.time();
    let sunset = NaiveTime::parse_from_str(&config.sunset, "%H:%M:%S").unwrap();
    let sunrise = NaiveTime::parse_from_str(&config.sunrise, "%H:%M:%S").unwrap();

    // Convert all times to seconds since midnight for easier comparison
    let current_secs =
        current_time.hour() * 3600 + current_time.minute() * 60 + current_time.second();
    let sunset_secs = sunset.hour() * 3600 + sunset.minute() * 60 + sunset.second();
    let sunrise_secs = sunrise.hour() * 3600 + sunrise.minute() * 60 + sunrise.second();

    let seconds_until = match get_current_state(config) {
        TimeState::Day => {
            if sunset_secs > current_secs {
                // Sunset is later today
                sunset_secs - current_secs
            } else {
                // Sunset is tomorrow
                (24 * 3600) - current_secs + sunset_secs
            }
        }
        TimeState::Night => {
            if sunrise_secs > current_secs {
                // Sunrise is later today
                sunrise_secs - current_secs
            } else {
                // Sunrise is tomorrow
                (24 * 3600) - current_secs + sunrise_secs
            }
        }
    };

    Duration::from_secs(seconds_until as u64)
}

fn kill_existing_hyprsunset() -> Result<()> {
    Command::new("pkill")
        .arg("hyprsunset")
        .output()
        .context("Failed to kill existing hyprsunset processes")?;

    // Small delay to ensure process is killed
    thread::sleep(Duration::from_millis(100));
    Ok(())
}

fn start_hyprsunset_night(temp: u32) -> Result<Child> {
    println!("Setting night temperature to {}K", temp);
    Command::new("hyprsunset")
        .arg("--temperature")
        .arg(temp.to_string())
        .spawn()
        .context("Failed to start hyprsunset")
}

fn start_hyprsunset_day() -> Result<Child> {
    println!("Setting to default temperature");
    Command::new("hyprsunset")
        .arg("-i")
        .spawn()
        .context("Failed to start hyprsunset")
}

fn main() -> Result<()> {
    println!("Starting sunsetr...");

    // Set up signal handling
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    let mut signals = Signals::new([SIGTERM, SIGINT])?;
    thread::spawn(move || {
        for _ in signals.forever() {
            println!("Received shutdown signal");
            r.store(false, Ordering::SeqCst);
        }
    });

    // Create and acquire lock file
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    let lock_path = format!("{}/sunsetr.lock", runtime_dir);
    let lock_file = File::create(&lock_path)?;

    // Try to acquire exclusive lock
    match lock_file.try_lock_exclusive() {
        Ok(_) => {
            println!("Lock acquired, starting sunsetr...");
            kill_existing_hyprsunset()?;

            let config = Config::load()?;
            println!(
                "Loaded configuration: sunset={}, sunrise={}, temp={}",
                config.sunset,
                config.sunrise,
                config.temp.unwrap_or(4000)
            );

            let temp = config.temp.unwrap_or(4000);
            let mut current_state = get_current_state(&config);

            // Start initial hyprsunset process
            let mut hyprsunset_process = match current_state {
                TimeState::Night => start_hyprsunset_night(temp)?,
                TimeState::Day => start_hyprsunset_day()?,
            };

            // Main loop
            while running.load(Ordering::SeqCst) {
                let sleep_duration = time_until_next_event(&config);
                println!(
                    "Sleeping until next transition in {} minutes {} seconds",
                    sleep_duration.as_secs() / 60,
                    sleep_duration.as_secs() % 60
                );

                // Sleep in smaller intervals to check running status
                let mut slept = Duration::from_secs(0);
                while slept < sleep_duration && running.load(Ordering::SeqCst) {
                    let sleep_chunk = Duration::from_secs(1).min(sleep_duration - slept);
                    thread::sleep(sleep_chunk);
                    slept += sleep_chunk;
                }

                if !running.load(Ordering::SeqCst) {
                    break;
                }

                let new_state = get_current_state(&config);
                if new_state != current_state {
                    // Kill the existing hyprsunset process
                    if let Err(e) = hyprsunset_process.kill() {
                        println!("Warning: Failed to kill hyprsunset process: {}", e);
                    }
                    thread::sleep(Duration::from_millis(100));

                    // Start new hyprsunset process with new settings
                    hyprsunset_process = match new_state {
                        TimeState::Night => start_hyprsunset_night(temp)?,
                        TimeState::Day => start_hyprsunset_day()?,
                    };
                    current_state = new_state;
                }
            }

            // Cleanup on exit
            println!("Shutting down sunsetr...");
            if let Err(e) = hyprsunset_process.kill() {
                println!(
                    "Warning: Failed to kill hyprsunset process during shutdown: {}",
                    e
                );
            }
            kill_existing_hyprsunset()?;

            // Remove lock file
            drop(lock_file);
            if let Err(e) = fs::remove_file(&lock_path) {
                println!("Warning: Failed to remove lock file: {}", e);
            }
        }
        Err(_) => {
            println!(
                "Another instance of sunsetr is already running. Kill sunsetr before restarting."
            );
            std::process::exit(1);
        }
    }

    Ok(())
}
