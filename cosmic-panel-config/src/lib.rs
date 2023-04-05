//! Config for cosmic-panel
#[cfg(feature = "wayland-rs")]
mod container_config;
mod panel_config;

#[cfg(feature = "wayland-rs")]
pub use container_config::*;
pub use panel_config::*;
