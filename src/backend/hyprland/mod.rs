use anyhow::Result;
use std::sync::atomic::AtomicBool;

use crate::backend::ColorTemperatureBackend;
use crate::config::Config;
use crate::constants::*;
use crate::logger::Log;
use crate::time_state::TransitionState;

pub mod client;
pub mod process;

pub use client::HyprsunsetClient;
pub use process::{HyprsunsetProcess, is_hyprsunset_running};

/// Hyprland backend implementation using hyprsunset for color temperature control.
/// 
/// This backend wraps the existing hyprsunset client functionality without modification,
/// providing a bridge between the new backend abstraction and the existing Hyprland code.
pub struct HyprlandBackend {
    client: HyprsunsetClient,
    process: Option<HyprsunsetProcess>,
}

impl HyprlandBackend {
    /// Create a new Hyprland backend instance.
    /// 
    /// This function initializes the hyprsunset client and optionally starts
    /// the hyprsunset process if configured to do so.
    /// 
    /// # Arguments
    /// * `config` - Configuration containing Hyprland-specific settings
    /// 
    /// # Returns
    /// A new HyprlandBackend instance ready for use
    /// 
    /// # Errors
    /// Returns an error if:
    /// - hyprsunset is not installed or incompatible
    /// - Process management conflicts are detected
    /// - Client initialization fails
    pub fn new(config: &Config) -> Result<Self> {
        // Verify hyprsunset installation and version compatibility
        verify_hyprsunset_installed_and_version()?;

        // Handle process management if configured
        let process = if config.start_hyprsunset.unwrap_or(DEFAULT_START_HYPRSUNSET) {
            // Check for conflicts with existing hyprsunset instances
            if is_hyprsunset_running() {
                anyhow::bail!(
                    "hyprsunset is already running but start_hyprsunset is set to true.\n\
                    This conflict prevents sunsetr from starting its own hyprsunset instance.\n\
                    \n\
                    To fix this, either:\n\
                    • Kill the existing hyprsunset process: pkill hyprsunset\n\
                    • Change start_hyprsunset = false in sunsetr.toml\n\
                    \n\
                    Choose the first option if you want sunsetr to manage hyprsunset.\n\
                    Choose the second option if you're using another method to start hyprsunset."
                );
            }

            // Determine initial values for hyprsunset startup
            let startup_transition = config.startup_transition.unwrap_or(DEFAULT_STARTUP_TRANSITION);
            let (initial_temp, initial_gamma) = if startup_transition {
                // If startup transition is enabled, start with day values
                (
                    config.day_temp.unwrap_or(DEFAULT_DAY_TEMP),
                    config.day_gamma.unwrap_or(DEFAULT_DAY_GAMMA),
                )
            } else {
                // If startup transition is disabled, start with current interpolated values
                let current_state = crate::time_state::get_transition_state(config);
                crate::time_state::get_initial_values_for_state(current_state, config)
            };

            Some(HyprsunsetProcess::new(initial_temp, initial_gamma)?)
        } else {
            None
        };

        // Initialize hyprsunset client
        let mut client = HyprsunsetClient::new()?;

        // Verify connection to hyprsunset
        verify_hyprsunset_connection(&mut client)?;

        Ok(Self { client, process })
    }

    /// Get a reference to the managed hyprsunset process, if any.
    #[allow(dead_code)]
    pub fn process(&self) -> Option<&HyprsunsetProcess> {
        self.process.as_ref()
    }

    /// Take ownership of the managed hyprsunset process, if any.
    /// 
    /// This is used during cleanup to properly terminate the process.
    #[allow(dead_code)]
    pub fn take_process(self) -> Option<HyprsunsetProcess> {
        self.process
    }
}

impl ColorTemperatureBackend for HyprlandBackend {
    fn test_connection(&mut self) -> bool {
        self.client.test_connection()
    }

    fn apply_transition_state(
        &mut self,
        state: TransitionState,
        config: &Config,
        running: &AtomicBool,
    ) -> Result<()> {
        self.client.apply_transition_state(state, config, running)
    }

    fn apply_startup_state(
        &mut self,
        state: TransitionState,
        config: &Config,
        running: &AtomicBool,
    ) -> Result<()> {
        self.client.apply_startup_state(state, config, running)
    }

    fn apply_temperature_gamma(
        &mut self,
        temperature: u32,
        gamma: f32,
        running: &AtomicBool,
    ) -> Result<()> {
        self.client.apply_temperature_gamma(temperature, gamma, running)
    }

    fn backend_name(&self) -> &'static str {
        "Hyprland"
    }

    fn cleanup(self: Box<Self>) {
        // Stop any managed hyprsunset process
        if let Some(process) = self.process {
            Log::log_decorated("Stopping managed hyprsunset process...");
            match process.stop() {
                Ok(_) => Log::log_decorated("Hyprsunset process stopped successfully"),
                Err(e) => Log::log_decorated(&format!("Warning: Failed to stop hyprsunset process: {}", e)),
            }
        }
    }
}

/// Verify that hyprsunset is installed and check version compatibility.
/// 
/// This function is moved from main.rs and performs both installation verification
/// and version checking in a single step for efficiency.
pub fn verify_hyprsunset_installed_and_version() -> Result<()> {
    use crate::utils::extract_version_from_output;

    match std::process::Command::new("hyprsunset")
        .arg("--version")
        .output()
    {
        Ok(output) => {
            let version_output = if !output.stdout.is_empty() {
                String::from_utf8_lossy(&output.stdout)
            } else {
                String::from_utf8_lossy(&output.stderr)
            };

            if let Some(version) = extract_version_from_output(&version_output) {
                Log::log_decorated(&format!("Found hyprsunset {}", version));

                if is_version_compatible(&version) {
                    Ok(())
                } else {
                    anyhow::bail!(
                        "hyprsunset {} is not compatible with sunsetr.\n\
                        Required minimum version: {}\n\
                        Compatible versions: {}\n\
                        Please update hyprsunset to a compatible version.",
                        version,
                        REQUIRED_HYPRSUNSET_VERSION,
                        COMPATIBLE_HYPRSUNSET_VERSIONS.join(", ")
                    )
                }
            } else {
                Log::log_warning("Could not parse version from hyprsunset output");
                Log::log_decorated("Attempting to proceed with compatibility test...");
                Ok(())
            }
        }
        Err(_) => {
            match std::process::Command::new("which")
                .arg("hyprsunset")
                .output()
            {
                Ok(which_output) if which_output.status.success() => {
                    Log::log_warning("hyprsunset found but version check failed");
                    Log::log_decorated("This might be an older version. Will attempt compatibility test...");
                    Ok(())
                }
                _ => anyhow::bail!("hyprsunset is not installed on the system"),
            }
        }
    }
}

/// Check if a hyprsunset version is compatible with sunsetr.
pub fn is_version_compatible(version: &str) -> bool {
    use crate::utils::compare_versions;
    
    if COMPATIBLE_HYPRSUNSET_VERSIONS.contains(&version) {
        return true;
    }

    compare_versions(version, REQUIRED_HYPRSUNSET_VERSION) >= std::cmp::Ordering::Equal
}

/// Verify that we can establish a connection to the hyprsunset socket.
pub fn verify_hyprsunset_connection(client: &mut HyprsunsetClient) -> Result<()> {
    use std::{thread, time::Duration};

    if client.test_connection() {
        return Ok(());
    }

    Log::log_decorated("Waiting 10 seconds for hyprsunset to become available...");
    thread::sleep(Duration::from_secs(10));

    if client.test_connection() {
        Log::log_decorated("Successfully connected to hyprsunset after waiting.");
        return Ok(());
    }

    Log::log_critical("Cannot connect to hyprsunset socket.");
    println!();

    anyhow::bail!(
        "This usually means:\n\
          • hyprsunset is not running\n\
          • hyprsunset service is not enabled\n\
          • You're not running on Hyprland\n\
        \n\
        Please ensure hyprsunset is running and try again.\n\
        \n\
        Suggested hyprsunset startup methods:\n\
          1. Autostart hyprsunset: set start_hyprsunset to true in sunsetr.toml\n\
          2. Start hyprsunset manually: hyprsunset\n\
          3. Enable the service: systemctl --user enable hyprsunset.service"
    );
} 