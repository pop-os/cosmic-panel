//! Handling of the wp-viewporter.

use sctk::reexports::{
    client::{
        delegate_dispatch,
        globals::{BindError, GlobalList},
        protocol::wl_surface::WlSurface,
        Connection, Dispatch, Proxy, QueueHandle,
    },
    protocols::wp::viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
};

use sctk::globals::GlobalData;

use crate::xdg_shell_wrapper::shared_state::GlobalState;

/// Viewporter.
#[derive(Debug, Clone)]
pub struct ViewporterState {
    viewporter: WpViewporter,
}

impl ViewporterState {
    /// Create new viewporter.
    pub fn new(
        globals: &GlobalList,
        queue_handle: &QueueHandle<GlobalState>,
    ) -> Result<Self, BindError> {
        let viewporter = globals.bind(queue_handle, 1..=1, GlobalData)?;
        Ok(Self { viewporter })
    }

    /// Get the viewport for the given object.
    pub fn get_viewport(
        &self,
        surface: &WlSurface,
        queue_handle: &QueueHandle<GlobalState>,
    ) -> WpViewport {
        self.viewporter.get_viewport(surface, queue_handle, GlobalData)
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
