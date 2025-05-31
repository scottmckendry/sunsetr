# Sunsetr Wayland Implementation Analysis and Planning

## Overview

This document provides a comprehensive analysis of the current sunsetr v0.3.0 implementation and detailed planning for adding Wayland protocols support while preserving the existing Hyprland functionality.

## Current Architecture Analysis

### Core Application Structure

The sunsetr application follows a modular architecture with clear separation of concerns:

1. **Main Application Flow** (`main.rs`)
   - Single-instance enforcement using file locking
   - Signal handling for graceful shutdown
   - Terminal state management
   - Main loop with state checking and transitions

2. **Configuration Management** (`config.rs`)
   - TOML-based configuration loading
   - Validation and defaults application
   - Currently loads from `~/.config/hypr/sunsetr.toml`

3. **Time State Management** (`time_state.rs`)
   - Core business logic for sunrise/sunset calculations
   - Transition state determination and progress calculation
   - Temperature/gamma interpolation
   - **Critical: This is completely compositor-agnostic and MUST be preserved**

4. **Hyprland Integration** (`hyprsunset.rs` + `process.rs`)
   - Unix socket communication with hyprsunset daemon
   - Process management for hyprsunset lifecycle
   - Retry logic and error handling
   - **Critical: This is Hyprland-specific and MUST remain unchanged**

5. **Utilities and Support** (`utils.rs`, `logger.rs`, `constants.rs`)
   - Version comparison, interpolation, logging
   - **Can be shared between implementations**

### Critical Components That Must NOT Change

#### 1. Hyprland Implementation (`hyprsunset.rs`)
**Location**: `src/hyprsunset.rs` (837 lines)

**Key Functions**:
- `HyprsunsetClient::new()` - Socket path determination using `HYPRLAND_INSTANCE_SIGNATURE`
- `send_command()` - Unix socket communication with hyprsunset
- `apply_transition_state()` - Applying temperature/gamma changes
- Error classification and retry logic
- Connection management and recovery

**Critical Details**:
- Uses `HYPRLAND_INSTANCE_SIGNATURE` environment variable for socket path
- Communicates via Unix socket at `{runtime_dir}/hypr/{instance}/.hyprsunset.sock`
- Sends commands like `set_temperature X` and `set_gamma Y`
- Implements sophisticated retry and reconnection logic
- **This entire module must remain untouched**

#### 2. Process Management (`process.rs`)
**Location**: `src/process.rs` (138 lines)

**Key Functions**:
- `HyprsunsetProcess::new()` - Starts hyprsunset with initial values
- `HyprsunsetProcess::stop()` - Graceful process termination
- `is_hyprsunset_running()` - Detection of running hyprsunset instances

**Critical Details**:
- Manages hyprsunset daemon lifecycle when `start_hyprsunset = true`
- Handles PID tracking and cleanup
- **This entire module must remain untouched**

#### 3. Time State Logic (`time_state.rs`)
**Location**: `src/time_state.rs` (1018 lines)

**Key Functions**:
- `get_transition_state()` - Core state determination
- `calculate_transition_windows()` - Timing calculations
- `calculate_interpolated_temp()` / `calculate_interpolated_gamma()` - Value interpolation
- `time_until_next_event()` - Sleep duration calculation

**Critical Details**:
- This is the heart of sunsetr's functionality
- Completely compositor-agnostic
- Contains extensive test coverage (lines 373-1018)
- **This entire module should be shared between implementations**

### Current Configuration Structure

```toml
start_hyprsunset = true           # Whether to start hyprsunset process
startup_transition = false       # Smooth startup transition
startup_transition_duration = 10 # Startup transition duration (seconds)
sunset = "19:00:00"              # Sunset time
sunrise = "06:00:00"             # Sunrise time
night_temp = 3300                # Night temperature (Kelvin)
day_temp = 6500                  # Day temperature (Kelvin)
night_gamma = 90.0               # Night gamma (percentage)
day_gamma = 100.0                # Day gamma (percentage)
transition_duration = 45         # Transition duration (minutes)
update_interval = 60             # Update frequency during transitions (seconds)
transition_mode = "finish_by"    # Transition timing mode
```

### Current Application Flow

1. **Startup**:
   - Acquire exclusive lock
   - Load configuration from `~/.config/hypr/sunsetr.toml`
   - Verify hyprsunset installation and version
   - Optionally start hyprsunset process
   - Initialize HyprsunsetClient
   - Verify socket connection

2. **Main Loop**:
   - Calculate current transition state using time_state logic
   - Apply state changes via HyprsunsetClient
   - Sleep until next update or event
   - Handle signals and cleanup

## Wayland Implementation Requirements

Based on the `considerations.md` file and analysis, the Wayland implementation should:

### 1. Configuration Changes

**New Configuration Fields**:
```toml
use_wayland = true               # Use Wayland protocols instead of hyprsunset
start_hyprsunset = false         # Should be false when use_wayland = true
```

**Configuration Logic**:
- If `use_wayland = true` AND `start_hyprsunset = true` → Warning and exit
- If `use_wayland = true` AND `start_hyprsunset = false` → Use Wayland protocols
- If `use_wayland = false` AND `start_hyprsunset = true` → Use Hyprland + start hyprsunset
- If `use_wayland = false` AND `start_hyprsunset = false` → Use Hyprland, external hyprsunset

**Config Path Changes**:
- New configs generated at `~/.config/sunsetr/sunsetr.toml`
- Still read from `~/.config/hypr/sunsetr.toml` for backward compatibility
- Only generate if no config exists in either location

### 2. Compositor Detection

**Auto-detection Logic**:
- Check `HYPRLAND_INSTANCE_SIGNATURE` environment variable
- If present → Default to `start_hyprsunset = true`, `use_wayland = false`
- If absent → Default to `use_wayland = true`, `start_hyprsunset = false`

**Environment Validation**:
- Check for Wayland session (e.g., `$WAYLAND_DISPLAY`)
- If not running on Wayland → Warning and exit

### 3. New Wayland Implementation Structure

**New Files Needed**:
- `src/wayland.rs` - Wayland protocols client implementation
- `src/backend.rs` - Backend abstraction trait
- `src/compositor.rs` - Compositor detection logic

**Dependencies to Add**:
```toml
wayland-client = "0.31"
wayland-protocols = "0.32"
wayland-protocols-wlr = "0.3"
```

### 4. Backend Abstraction

**Trait Definition**:
```rust
pub trait ColorTemperatureBackend {
    fn apply_temperature(&mut self, temp: u32) -> Result<()>;
    fn apply_gamma(&mut self, gamma: f32) -> Result<()>;
    fn apply_transition_state(&mut self, state: TransitionState, config: &Config) -> Result<()>;
    fn test_connection(&mut self) -> bool;
}
```

**Implementations**:
- `HyprlandBackend` - Wrapper around existing `HyprsunsetClient`
- `WaylandBackend` - New Wayland protocols implementation

## Implementation Plan

### Phase 1: Backend Abstraction

1. **Create Backend Trait** (`src/backend.rs`)
   - Define common interface for color temperature control
   - Abstract away implementation details

2. **Wrap Existing Hyprland Code** (`src/hyprland_backend.rs`)
   - Create wrapper that implements the backend trait
   - Preserve all existing `hyprsunset.rs` and `process.rs` functionality
   - **No changes to existing Hyprland code**

### Phase 2: Compositor Detection

1. **Create Compositor Detection** (`src/compositor.rs`)
   - Detect Hyprland vs other compositors
   - Environment variable validation
   - Auto-configure backend selection

### Phase 3: Wayland Implementation

1. **Create Wayland Backend** (`src/wayland.rs`)
   - Implement Wayland protocols for gamma/temperature control
   - Use wlr-gamma-control-unstable-v1 protocol
   - Implement the backend trait

### Phase 4: Configuration Integration

1. **Extend Configuration** (`src/config.rs`)
   - Add new configuration fields
   - Implement auto-detection logic
   - Handle dual config path support
   - Validate configuration combinations

### Phase 5: Main Application Integration

1. **Update Main Application** (`src/main.rs`)
   - Remove direct hyprsunset dependencies
   - Use backend abstraction
   - Add compositor detection
   - Preserve all existing behavior for Hyprland

## Risk Mitigation

### Preserving Hyprland Functionality

1. **No Changes to Core Hyprland Code**:
   - `hyprsunset.rs` remains completely unchanged
   - `process.rs` remains completely unchanged
   - Existing functionality wrapped, not modified

2. **Backward Compatibility**:
   - Existing configs continue to work
   - Default behavior for Hyprland unchanged
   - No breaking changes to user workflows

3. **Testing Strategy**:
   - Extensive testing on Hyprland systems
   - Verify no regression in existing functionality
   - Test configuration migration scenarios

### Code Quality Assurance

1. **Idiomatic Rust**:
   - Use proper error handling with `Result<T, E>`
   - Avoid unsafe code completely
   - Follow established patterns from existing codebase

2. **Separation of Concerns**:
   - Clear abstraction boundaries
   - Minimal coupling between backends
   - Shared code only for truly common functionality

## Wayland Protocols Research

### wlr-gamma-control-unstable-v1 Protocol

**Protocol Overview**:
- Primary protocol for gamma/temperature control on Wayland
- Supported by wlroots-based compositors (Sway, river, Wayfire, etc.)
- Provides per-output gamma table control
- Warning: Unstable protocol, but widely adopted

**Protocol Interfaces**:

1. **zwlr_gamma_control_manager_v1**:
   - Factory interface to create gamma controls
   - `get_gamma_control(output)` → creates control for specific output

2. **zwlr_gamma_control_v1**:
   - Per-output gamma control interface
   - `set_gamma(fd)` → Set gamma tables via file descriptor
   - `gamma_size` event → Reports size of gamma ramps
   - `failed` event → Indicates control is no longer valid

**Compositor Support**:
- ✅ Sway, Wayfire, river, labwc, niri, weston
- ❌ KWin, Mutter, Hyprland (has own protocol)
- ⚠️  COSMIC (planned for color management story)

### Gamma Table Format

**Technical Details**:
- Gamma tables are passed via memory-mapped file descriptors
- Format: Sequential gamma ramps for Red, Green, Blue channels
- Each ramp: Array of 16-bit unsigned integers
- Total size: `3 * gamma_size * 2 bytes`
- Gamma size reported via `gamma_size` event (typically 256)

**Temperature to Gamma Conversion**:
- Need to implement color temperature → RGB gamma curve conversion
- Algorithms available: Blackbody radiation curves, CIE colorimetric functions
- Existing reference implementations in wlsunset, gammastep, redshift

### Available Rust Implementations

**Reference Implementations**:

1. **wl-gammarelay-rs** (MaxVerevkin/wl-gammarelay-rs):
   - Full-featured gamma control daemon
   - Uses `wayland-protocols-wlr` crate
   - Provides D-Bus interface for control
   - Good reference for protocol usage

2. **Rust Wayland Bindings**:
   - `wayland-client` (0.31): Core Wayland client library
   - `wayland-protocols-wlr` (0.3): wlr protocol bindings
   - Well-maintained, actively used

**Key Code Patterns**:
```rust
use wayland_protocols_wlr::gamma_control::v1::client::{
    zwlr_gamma_control_manager_v1::ZwlrGammaControlManagerV1,
    zwlr_gamma_control_v1::ZwlrGammaControlV1,
};

// Create gamma control for output
let gamma_control = manager.get_gamma_control(&output, qh, ());

// Set gamma table
gamma_control.set_gamma(fd);
```

### Implementation Considerations

1. **Per-Output Control**:
   - Wayland is inherently multi-output aware
   - Need to enumerate outputs and create controls for each
   - Consider configuration for per-output vs. global settings

2. **Protocol Availability Detection**:
   - Check if compositor supports wlr-gamma-control-unstable-v1
   - Graceful failure when protocol unavailable
   - Clear error messages for unsupported setups

3. **Gamma Curve Generation**:
   - Implement temperature → gamma table conversion
   - Support both temperature and gamma adjustment
   - Maintain precision across the temperature range

4. **Error Handling**:
   - Handle `failed` events gracefully
   - Detect when another client takes control
   - Retry logic for temporary failures

### Alternative Approaches

1. **wl-gammarelay Integration**:
   - Instead of direct protocol implementation
   - Use existing wl-gammarelay-rs as backend
   - Communicate via D-Bus interface
   - Lower implementation complexity

2. **Library Integration**:
   - Use existing gamma control libraries
   - Focus on integration rather than protocol implementation
   - Faster development timeline

## Success Criteria

1. **Hyprland Compatibility**: Zero regression in Hyprland functionality
2. **Wayland Support**: Working gamma/temperature control on wlroots compositors
3. **Auto-Detection**: Seamless experience with automatic backend selection
4. **Configuration**: Smooth migration and dual-path config support
5. **Code Quality**: Maintainable, idiomatic Rust code
6. **Documentation**: Clear setup instructions for all supported compositors

## Next Steps

1. **Research Implementation Details**: Study wl-gammarelay-rs implementation patterns
2. **Prototype Protocol Usage**: Create minimal working Wayland gamma control
3. **Design Backend Abstraction**: Finalize trait design and error handling
4. **Implement Compositor Detection**: Build robust auto-detection logic
5. **Create Wayland Backend**: Full implementation with proper error handling
6. **Integration Testing**: Verify no Hyprland regressions
7. **Multi-Compositor Testing**: Validate on Sway, river, Wayfire, etc.

This plan ensures a careful, measured approach that preserves the existing, working Hyprland implementation while adding robust Wayland support for broader compatibility.

## Research Summary

The research shows that implementing Wayland gamma control is well-understood with good Rust ecosystem support. The wlr-gamma-control-unstable-v1 protocol, while marked "unstable", is widely supported across wlroots-based compositors and has stable implementations. The main challenge will be:

1. **Gamma curve mathematics**: Converting temperature values to proper RGB gamma curves
2. **Multi-output handling**: Managing gamma control across multiple displays
3. **Protocol lifecycle**: Proper setup, error handling, and cleanup

The existence of wl-gammarelay-rs as a reference implementation significantly reduces implementation risk, and the protocol itself is straightforward to use with the available Rust bindings. 