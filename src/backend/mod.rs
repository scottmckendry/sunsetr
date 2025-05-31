use anyhow::Result;
use std::sync::atomic::AtomicBool;

use crate::config::Config;
use crate::time_state::TransitionState;

pub mod hyprland;
pub mod wayland;

/// Trait for color temperature backends that can control display temperature and gamma.
/// 
/// This trait abstracts the differences between Hyprland (hyprsunset) and Wayland
/// (wlr-gamma-control-unstable-v1) implementations while providing a common interface
/// for the main application logic.
pub trait ColorTemperatureBackend {
    /// Test if the backend can establish a connection to the display control system.
    /// 
    /// # Returns
    /// - `true` if the backend is available and can control display settings
    /// - `false` if the backend cannot connect or is unavailable
    fn test_connection(&mut self) -> bool;

    /// Apply a specific transition state with proper interpolation.
    /// 
    /// This is the main method for applying color temperature and gamma changes.
    /// It handles both stable states and transitioning states with progress interpolation.
    /// 
    /// # Arguments
    /// * `state` - The transition state to apply (stable or transitioning)
    /// * `config` - Configuration containing temperature and gamma values
    /// * `running` - Atomic flag to check if the application should continue
    /// 
    /// # Returns
    /// - `Ok(())` if the state was applied successfully
    /// - `Err` if there was an error applying the state
    fn apply_transition_state(
        &mut self,
        state: TransitionState,
        config: &Config,
        running: &AtomicBool,
    ) -> Result<()>;

    /// Apply startup state during application initialization.
    /// 
    /// This method is called during startup to set the initial display state.
    /// It may handle startup transitions differently than regular transitions.
    /// 
    /// # Arguments
    /// * `state` - The initial transition state to apply
    /// * `config` - Configuration containing startup settings
    /// * `running` - Atomic flag to check if the application should continue
    /// 
    /// # Returns
    /// - `Ok(())` if the startup state was applied successfully
    /// - `Err` if there was an error applying the startup state
    fn apply_startup_state(
        &mut self,
        state: TransitionState,
        config: &Config,
        running: &AtomicBool,
    ) -> Result<()>;

    /// Get a human-readable name for this backend.
    /// 
    /// # Returns
    /// A string identifying the backend (e.g., "Hyprland", "Wayland")
    fn backend_name(&self) -> &'static str;
}

/// Detect the appropriate backend based on the current environment and configuration.
/// 
/// This function examines environment variables and system state to determine
/// whether to use the Hyprland or Wayland backend.
/// 
/// # Arguments
/// * `config` - Configuration that may explicitly specify backend preference
/// 
/// # Returns
/// - `BackendType::Hyprland` if running on Hyprland or explicitly configured
/// - `BackendType::Wayland` if running on other Wayland compositors
/// 
/// # Errors
/// Returns an error if no suitable backend can be determined or if the
/// environment is not supported (e.g., not running on Wayland).
pub fn detect_backend(_config: &Config) -> Result<BackendType> {
    // TODO: Implement use_wayland configuration field
    // Check explicit configuration first
    // if let Some(use_wayland) = config.use_wayland {
    //     if use_wayland {
    //         // Verify we're actually on Wayland
    //         if std::env::var("WAYLAND_DISPLAY").is_err() {
    //             anyhow::bail!(
    //                 "Configuration specifies use_wayland=true but WAYLAND_DISPLAY is not set.\n\
    //                 Are you running on Wayland?"
    //             );
    //         }
    //         return Ok(BackendType::Wayland);
    //     } else {
    //         return Ok(BackendType::Hyprland);
    //     }
    // }

    // Auto-detect based on environment
    if std::env::var("WAYLAND_DISPLAY").is_err() {
        anyhow::bail!(
            "sunsetr requires a Wayland session. WAYLAND_DISPLAY is not set.\n\
            Please ensure you're running on a Wayland compositor."
        );
    }

    // Check if we're running on Hyprland
    if std::env::var("HYPRLAND_INSTANCE_SIGNATURE").is_ok() {
        Ok(BackendType::Hyprland)
    } else {
        Ok(BackendType::Wayland)
    }
}

/// Create a backend instance based on the detected or configured backend type.
/// 
/// # Arguments
/// * `backend_type` - The type of backend to create
/// * `config` - Configuration for backend initialization
/// 
/// # Returns
/// A boxed backend implementation ready for use
/// 
/// # Errors
/// Returns an error if the backend cannot be initialized or if required
/// dependencies are missing.
pub fn create_backend(backend_type: BackendType, config: &Config) -> Result<Box<dyn ColorTemperatureBackend>> {
    match backend_type {
        BackendType::Hyprland => {
            Ok(Box::new(hyprland::HyprlandBackend::new(config)?) as Box<dyn ColorTemperatureBackend>)
        }
        BackendType::Wayland => {
            Ok(Box::new(wayland::WaylandBackend::new(config)?) as Box<dyn ColorTemperatureBackend>)
        }
    }
}

/// Enumeration of available backend types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendType {
    /// Hyprland compositor using hyprsunset for color temperature control
    Hyprland,
    /// Generic Wayland compositor using wlr-gamma-control-unstable-v1 protocol
    Wayland,
}

impl BackendType {
    /// Get the human-readable name for this backend type.
    pub fn name(&self) -> &'static str {
        match self {
            BackendType::Hyprland => "Hyprland",
            BackendType::Wayland => "Wayland",
        }
    }

    /// Get the default configuration values for this backend type.
    /// 
    /// # Returns
    /// Tuple of (start_hyprsunset, use_wayland) defaults for this backend
    pub fn default_config_values(&self) -> (bool, bool) {
        match self {
            BackendType::Hyprland => (true, false),   // Start hyprsunset, don't use wayland protocols
            BackendType::Wayland => (false, true),    // Don't start hyprsunset, use wayland protocols
        }
    }
} 