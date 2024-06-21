// SPDX-License-Identifier: MPL-2.0

use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

use sctk::{
    compositor::CompositorState,
    output::OutputInfo,
    reexports::client::{
        protocol::{wl_output as c_wl_output, wl_surface},
        Connection, QueueHandle,
    },
    seat::pointer::PointerEvent,
    shell::{
        wlr_layer::{LayerShell, LayerSurface, LayerSurfaceConfigure},
        xdg::{XdgPositioner, XdgShell},
    },
};
use smithay::{
    backend::renderer::gles::GlesRenderer,
    desktop::{PopupManager, Window},
    output::Output,
    reexports::wayland_server::{
        self, protocol::wl_surface::WlSurface as s_WlSurface, DisplayHandle,
    },
    wayland::shell::xdg::{PopupSurface, PositionerState},
};

use crate::{
    iced::elements::target::SpaceTarget,
    xdg_shell_wrapper::{
        client::handlers::{
            wp_fractional_scaling::FractionalScalingManager, wp_viewporter::ViewporterState,
        },
        client_state::ClientFocus,
        config::WrapperConfig,
        server_state::ServerPointerFocus,
        shared_state::GlobalState,
        wp_security_context::SecurityContextManager,
    },
};

/// Space events
#[derive(Eq, PartialEq, Copy, Clone, Debug)]
pub enum SpaceEvent {
    /// waiting for the next configure event
    WaitConfigure {
        /// whether it is waiting for the first configure event
        first: bool,
        /// width
        width: i32,
        /// height
        height: i32,
    },
    /// the space has been scheduled to cleanup and exit
    Quit,
}

/// Visibility of the space
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Visibility {
    /// hidden
    Hidden,
    /// visible
    Visible,
    /// transitioning to hidden
    TransitionToHidden {
        /// previous instant that was processed
        last_instant: Instant,
        /// duration of the transition progressed
        progress: Duration,
        /// previously calculated value
        prev_margin: i32,
    },
    /// transitioning to visible
    TransitionToVisible {
        /// previous instant that was processed
        last_instant: Instant,
        /// duration of the transition progressed
        progress: Duration,
        /// previously calculated value
        prev_margin: i32,
    },
}

impl Default for Visibility {
    fn default() -> Self {
        Self::Visible
    }
}

// TODO break this trait into several traits so that it can be better organized
// not all "space" implementations really need all of these exact methods as
// long as they are wrapped by a space that does see cosmic-panel for an example

/// Wrapper Space
/// manages and renders xdg-shell-window(s) on a layer shell surface
pub trait WrapperSpace {
    /// Wrapper config type
    type Config: WrapperConfig;

    /// set the display handle of the space
    fn set_display_handle(&mut self, display: wayland_server::DisplayHandle);

    /// get the client hovered surface of the space
    fn get_client_hovered_surface(&self) -> Rc<RefCell<ClientFocus>>;

    /// get the client focused surface of the space
    fn get_client_focused_surface(&self) -> Rc<RefCell<ClientFocus>>;

    /// setup of the space after the wayland connection is ready
    fn setup(
        &mut self,
        compositor_state: &CompositorState,
        fractional_scale_manager: Option<&FractionalScalingManager>,
        security_context_manager: Option<SecurityContextManager>,
        viewport: Option<&ViewporterState>,
        layer_state: &mut LayerShell,
        conn: &Connection,
        qh: &QueueHandle<GlobalState>,
    );

    /// add the configured output to the space
    fn new_output(
        &mut self,
        compositor_state: &CompositorState,
        fractional_scale_manager: Option<&FractionalScalingManager>,
        viewport: Option<&ViewporterState>,
        layer_state: &mut LayerShell,
        conn: &Connection,
        qh: &QueueHandle<GlobalState>,
        c_output: Option<c_wl_output::WlOutput>,
        s_output: Option<Output>,
        info: Option<OutputInfo>,
    ) -> anyhow::Result<()>;

    /// update the configured output in the space
    fn update_output(
        &mut self,
        c_output: c_wl_output::WlOutput,
        s_output: Output,
        info: OutputInfo,
    ) -> anyhow::Result<bool>;

    /// remove the configured output from the space
    fn output_leave(
        &mut self,
        c_output: c_wl_output::WlOutput,
        s_output: Output,
    ) -> anyhow::Result<()>;

    /// handle pointer motion on the space
    fn update_pointer(
        &mut self,
        dim: (i32, i32),
        seat_name: &str,
        surface: wl_surface::WlSurface,
    ) -> Option<ServerPointerFocus>;

    /// add a top level window to the space
    fn add_window(&mut self, s_top_level: Window);

    /// add a popup to the space
    fn add_popup(
        &mut self,
        compositor_state: &CompositorState,
        fractional_scale_manager: Option<&FractionalScalingManager>,
        viewport: Option<&ViewporterState>,
        conn: &Connection,
        qh: &QueueHandle<GlobalState>,
        xdg_shell_state: &mut XdgShell,
        s_surface: PopupSurface,
        positioner: XdgPositioner,
        positioner_state: PositionerState,
    ) -> anyhow::Result<()>;

    /// handle a button press or release on a client surface
    /// optionally returns an interacted server wl surface
    fn handle_button(&mut self, seat_name: &str, press: bool) -> Option<SpaceTarget>;

    /// keyboard focus lost handler
    fn keyboard_leave(&mut self, seat_name: &str, surface: Option<wl_surface::WlSurface>);

    /// keyboard focus gained handler
    /// optionally returns a focused server wl surface
    fn keyboard_enter(
        &mut self,
        seat_name: &str,
        surface: wl_surface::WlSurface,
    ) -> Option<s_WlSurface>;

    /// pointer focus lost handler
    fn pointer_leave(&mut self, seat_name: &str, surface: Option<wl_surface::WlSurface>);

    /// pointer focus gained handler
    fn pointer_enter(
        &mut self,
        dim: (i32, i32),
        seat_name: &str,
        surface: wl_surface::WlSurface,
    ) -> Option<ServerPointerFocus>;

    /// repositions a popup
    fn reposition_popup(
        &mut self,
        popup: PopupSurface,
        positioner_state: PositionerState,
        token: u32,
    ) -> anyhow::Result<()>;

    /// called in a loop by xdg-shell-wrapper
    /// handles events for the space
    /// returns the Instant it was last updated by clients and a list of
    /// surfaces to request frames for
    fn handle_events(
        &mut self,
        dh: &DisplayHandle,
        qh: &QueueHandle<GlobalState>,
        popup_manager: &mut PopupManager,
        time: u32,
    ) -> Instant;

    /// gets the config
    fn config(&self) -> Self::Config;

    /// spawns the clients for the wrapper
    fn spawn_clients(
        &mut self,
        display: wayland_server::DisplayHandle,
        qh: &QueueHandle<GlobalState>,
        security_context_manager: Option<SecurityContextManager>,
    ) -> anyhow::Result<()>;

    /// gets visibility of the wrapper
    fn visibility(&self) -> Visibility {
        Visibility::Visible
    }

    /// cleanup
    fn destroy(&mut self);

    /// Moves an already mapped Window to top of the stack
    /// This function does nothing for unmapped windows.
    /// If activate is true it will set the new windows state to be activate and
    /// removes that state from every other mapped window.
    fn raise_window(&mut self, _: &Window, _: bool) {}

    /// marks the window as dirtied
    fn dirty_window(&mut self, dh: &DisplayHandle, w: &s_WlSurface);

    /// marks the popup as dirtied()
    fn dirty_popup(&mut self, dh: &DisplayHandle, w: &s_WlSurface);

    /// configure popup
    fn configure_popup(
        &mut self,
        popup: &sctk::shell::xdg::popup::Popup,
        config: sctk::shell::xdg::popup::PopupConfigure,
    );

    /// finished popup
    fn close_popup(&mut self, popup: &sctk::shell::xdg::popup::Popup);

    /// configure layer
    fn configure_layer(&mut self, layer: &LayerSurface, configure: LayerSurfaceConfigure);

    /// close layer in space
    fn close_layer(&mut self, layer: &LayerSurface);

    /// gets the renderer for twl_surface::wl_surface::he space
    fn renderer(&mut self) -> Option<&mut GlesRenderer>;

    /// received a frame event for the given surface
    fn frame(&mut self, surface: &wl_surface::WlSurface, time: u32);

    /// scale factor changed for the given surface
    /// if this is a surface for this space, it should be tracked
    fn scale_factor_changed(
        &mut self,
        surface: &wl_surface::WlSurface,
        new_scale: f64,
        legacy: bool,
    );

    /// preferred transform changed for the given surface
    fn transform_changed(
        &mut self,
        conn: &Connection,
        surface: &wl_surface::WlSurface,
        new_transform: sctk::reexports::client::protocol::wl_output::Transform,
    );

    /// get the scale factor for a surface
    /// returns none if the surface is not tracked by this space
    fn get_scale_factor(&self, surface: &s_WlSurface) -> Option<f64>;

    /// Generate Pointer events for clients
    fn generate_pointer_events(&mut self) -> Vec<PointerEvent> {
        Vec::new()
    }
}
