//! Handling of the wp-viewporter.

use std::marker::PhantomData;

use sctk::reexports::client::globals::{BindError, GlobalList};
use sctk::reexports::client::protocol::wl_surface::WlSurface;
use sctk::reexports::client::Dispatch;
use sctk::reexports::client::{delegate_dispatch, Connection, Proxy, QueueHandle};
use sctk::reexports::protocols::wp::viewporter::client::wp_viewport::WpViewport;
use sctk::reexports::protocols::wp::viewporter::client::wp_viewporter::WpViewporter;

use sctk::globals::GlobalData;

use crate::shared_state::GlobalState;
use crate::space::WrapperSpace;

/// Viewporter.
#[derive(Debug, Clone)]
pub struct ViewporterState<T> {
    viewporter: WpViewporter,
    _phantom: PhantomData<T>,
}

impl ViewporterState<T> {
    /// Create new viewporter.
    pub fn new(
        globals: &GlobalList,
        queue_handle: &QueueHandle<GlobalState>,
    ) -> Result<Self, BindError> {
        let viewporter = globals.bind(queue_handle, 1..=1, GlobalData)?;
        Ok(Self { viewporter, _phantom: PhantomData })
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

impl Dispatch<WpViewporter, GlobalData, GlobalState> for ViewporterState<T> {
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

impl Dispatch<WpViewport, GlobalData, GlobalState> for ViewporterState<T> {
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

delegate_dispatch!(@<T: 'static + WrapperSpace> GlobalState: [WpViewporter: GlobalData] => ViewporterState<T>);
delegate_dispatch!(@<T: 'static + WrapperSpace> GlobalState: [WpViewport: GlobalData] => ViewporterState<T>);
