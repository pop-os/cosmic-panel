//! Handling of the wp-viewporter.

use sctk::reexports::client::globals::{BindError, GlobalList};
use sctk::reexports::client::protocol::wl_surface::WlSurface;
use sctk::reexports::client::{Connection, Dispatch, Proxy, QueueHandle, delegate_dispatch};
use sctk::reexports::protocols::wp::viewporter::client::wp_viewport::WpViewport;
use sctk::reexports::protocols::wp::viewporter::client::wp_viewporter::WpViewporter;

use sctk::globals::GlobalData;

use crate::xdg_shell_wrapper::shared_state::GlobalState;

/// Viewporter.
#[derive(Debug, Clone)]
pub struct ViewporterState {
    viewporter: WpViewporter,
}

impl ViewporterState {
    /// Create new viewporter.
    pub fn new(globals: &GlobalList, qh: &QueueHandle<GlobalState>) -> Result<Self, BindError> {
        let viewporter = globals.bind(qh, 1..=1, GlobalData)?;
        Ok(Self { viewporter })
    }

    /// Get the viewport for the given object.
    pub fn get_viewport(&self, surface: &WlSurface, qh: &QueueHandle<GlobalState>) -> WpViewport {
        self.viewporter.get_viewport(surface, qh, GlobalData)
    }
}

impl Dispatch<WpViewporter, GlobalData, GlobalState> for ViewporterState {
    fn event(
        _: &mut GlobalState,
        _: &WpViewporter,
        _: <WpViewporter as Proxy>::Event,
        _: &GlobalData,
        _: &Connection,
        _: &QueueHandle<GlobalState>,
    ) {
        // No events.
    }
}

impl Dispatch<WpViewport, GlobalData, GlobalState> for ViewporterState {
    fn event(
        _: &mut GlobalState,
        _: &WpViewport,
        _: <WpViewport as Proxy>::Event,
        _: &GlobalData,
        _: &Connection,
        _: &QueueHandle<GlobalState>,
    ) {
        // No events.
    }
}

delegate_dispatch!(GlobalState: [WpViewporter: GlobalData] => ViewporterState);
delegate_dispatch!(GlobalState: [WpViewport: GlobalData] => ViewporterState);
