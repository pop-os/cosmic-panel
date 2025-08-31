// From: https://github.com/rust-windowing/winit/blob/master/src/platform_impl/linux/wayland/types/wp_fractional_scaling.rs
//! Handling of the fractional scaling.

use sctk::reexports::{
    client::{
        Connection, Dispatch, Proxy, QueueHandle, delegate_dispatch,
        globals::{BindError, GlobalList},
        protocol::wl_surface::WlSurface,
    },
    protocols::wp::fractional_scale::v1::client::{
        wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
        wp_fractional_scale_v1::{Event as FractionalScalingEvent, WpFractionalScaleV1},
    },
};

use sctk::globals::GlobalData;

use crate::xdg_shell_wrapper::shared_state::GlobalState;

/// The scaling factor denominator.
const SCALE_DENOMINATOR: f64 = 120.;

/// Fractional scaling manager.
#[derive(Debug, Clone)]
pub struct FractionalScalingManager {
    manager: WpFractionalScaleManagerV1,
}

/// Fractional scaling data.
#[derive(Debug, Clone)]
pub struct FractionalScaling {
    /// The surface used for scaling.
    surface: WlSurface,
}

impl FractionalScalingManager {
    /// Create new viewporter.
    pub fn new(
        globals: &GlobalList,
        queue_handle: &QueueHandle<GlobalState>,
    ) -> Result<Self, BindError> {
        let manager = globals.bind(queue_handle, 1..=1, GlobalData)?;
        Ok(Self { manager })
    }

    /// Create a fractional scaling object for a given surface.
    pub fn fractional_scaling(
        &self,
        surface: &WlSurface,
        queue_handle: &QueueHandle<GlobalState>,
    ) -> WpFractionalScaleV1 {
        let data = FractionalScaling { surface: surface.clone() };
        self.manager.get_fractional_scale(surface, queue_handle, data)
    }
}

impl Dispatch<WpFractionalScaleManagerV1, GlobalData, GlobalState> for FractionalScalingManager {
    fn event(
        _: &mut GlobalState,
        _: &WpFractionalScaleManagerV1,
        _: <WpFractionalScaleManagerV1 as Proxy>::Event,
        _: &GlobalData,
        _: &Connection,
        _: &QueueHandle<GlobalState>,
    ) {
        // No events.
    }
}

impl Dispatch<WpFractionalScaleV1, FractionalScaling, GlobalState> for FractionalScalingManager {
    fn event(
        state: &mut GlobalState,
        _: &WpFractionalScaleV1,
        event: <WpFractionalScaleV1 as Proxy>::Event,
        data: &FractionalScaling,
        _: &Connection,
        _: &QueueHandle<GlobalState>,
    ) {
        if let FractionalScalingEvent::PreferredScale { scale } = event {
            state.scale_factor_changed(&data.surface, scale as f64 / SCALE_DENOMINATOR, false);
        }
    }
}

delegate_dispatch!(GlobalState: [WpFractionalScaleManagerV1: GlobalData] => FractionalScalingManager);
delegate_dispatch!(GlobalState: [WpFractionalScaleV1: FractionalScaling] => FractionalScalingManager);
