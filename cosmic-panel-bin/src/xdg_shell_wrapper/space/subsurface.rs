use cctk::wayland_client::protocol::{
    wl_subsurface::WlSubsurface as c_WlSubsurface, wl_surface::WlSurface as c_WlSurface,
};
use smithay::{
    backend::{egl::EGLSurface, renderer::damage::OutputDamageTracker},
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{IsAlive, Logical, Point, Rectangle},
};
use wayland_protocols::wp::{
    fractional_scale::v1::client::wp_fractional_scale_v1::WpFractionalScaleV1,
    viewporter::client::wp_viewport::WpViewport,
};

use crate::iced::elements::CosmicMappedInternal;

/// Popup
#[derive(Debug)]
pub struct WrapperSubsurface {
    /// parent
    pub parent: CosmicMappedInternal,
    /// panel subsurface
    pub subsurface: PanelSubsurface,
    /// the embedded subsurface
    pub s_surface: WlSurface,
}

#[derive(Debug)]
pub struct PanelSubsurface {
    // XXX implicitly drops egl_surface first to avoid segfault
    /// the egl surface
    pub egl_surface: EGLSurface,

    /// the subsurface on the layer shell surface
    pub c_subsurface: c_WlSubsurface,

    /// the wl_surface
    pub c_surface: c_WlSurface,
    /// whether or not the subsurface needs to be rendered
    pub dirty: bool,
    /// full rectangle of the inner subsurface, including dropshadow borders
    pub rectangle: Rectangle<i32, Logical>,
    /// location of the subsurface wrapper
    pub wrapper_rectangle: Point<i32, Logical>,
    /// received a frame callback
    pub has_frame: bool,
    /// fractional scale for the subsurface
    pub fractional_scale: Option<WpFractionalScaleV1>,
    /// viewport for the subsurface
    pub viewport: Option<WpViewport>,
    /// scale factor for the subsurface
    pub scale: f64,
    /// damage tracking renderer
    pub damage_tracked_renderer: OutputDamageTracker,
    /// parent of the subsurface
    pub parent: c_WlSurface,
}

impl WrapperSubsurface {
    /// Handles any events that have occurred since the last call, redrawing if
    /// needed. Returns true if the surface is alive.
    pub fn handle_events(&mut self) -> bool {
        if !self.s_surface.alive() {
            self.subsurface.c_subsurface.destroy();
            false
        } else {
            true
        }
    }
}
