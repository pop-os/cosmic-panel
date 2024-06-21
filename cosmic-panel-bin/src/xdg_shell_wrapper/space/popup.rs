// SPDX-License-Identifier: MPL-2.0

use std::rc::Rc;

use sctk::{
    compositor::Region,
    shell::xdg::{popup::Popup, XdgPositioner},
};
use smithay::{
    backend::{egl::surface::EGLSurface, renderer::damage::OutputDamageTracker},
    desktop::PopupManager,
    utils::{Logical, Rectangle, Size},
    wayland::shell::xdg::PopupSurface,
};
use wayland_protocols::wp::{
    fractional_scale::v1::client::wp_fractional_scale_v1::WpFractionalScaleV1,
    viewporter::client::wp_viewport::WpViewport,
};

/// Popup events
#[derive(Eq, PartialEq, Copy, Clone, Debug)]
pub enum WrapperPopupState {
    /// Wait for configure event to render
    WaitConfigure,
    /// Configure Event
    Rectangle {
        /// x position
        x: i32,
        /// y position
        y: i32,
        /// width
        width: i32,
        /// height
        height: i32,
    },
}

/// Popup
#[derive(Debug)]
pub struct WrapperPopup {
    /// panel popup
    pub popup: PanelPopup,
    /// the embedded popup
    pub s_surface: PopupSurface,
}

#[derive(Debug)]
pub struct PanelPopup {
    // XXX implicitly drops egl_surface first to avoid segfault
    /// the egl surface
    pub egl_surface: Option<Rc<EGLSurface>>,

    /// the popup on the layer shell surface
    pub c_popup: Popup,

    /// the state of the popup
    pub state: Option<WrapperPopupState>,
    /// whether or not the popup needs to be rendered
    pub dirty: bool,
    /// full rectangle of the inner popup, including dropshadow borders
    pub rectangle: Rectangle<i32, Logical>,
    /// input region for the popup
    pub input_region: Option<Region>,
    /// location of the popup wrapper
    pub wrapper_rectangle: Rectangle<i32, Logical>,
    /// positioner
    pub positioner: XdgPositioner,
    /// received a frame callback
    pub has_frame: bool,
    /// fractional scale for the popup
    pub fractional_scale: Option<WpFractionalScaleV1>,
    /// viewport for the popup
    pub viewport: Option<WpViewport>,
    /// scale factor for the popup
    pub scale: f64,
    /// damage tracking renderer
    pub damage_tracked_renderer: OutputDamageTracker,
}

impl WrapperPopup {
    /// Handles any events that have occurred since the last call, redrawing if
    /// needed. Returns true if the surface is alive.
    pub fn handle_events(&mut self, popup_manager: &mut PopupManager) -> bool {
        if let Some(WrapperPopupState::Rectangle { width, height, x, y }) = self.popup.state {
            self.popup.dirty = true;
            self.popup.rectangle = Rectangle::from_loc_and_size((x, y), (width, height));
            let scaled_size: Size<i32, _> =
                self.popup.rectangle.size.to_f64().to_physical(self.popup.scale).to_i32_round();
            if let Some(s) = self.popup.egl_surface.as_ref() {
                s.resize(scaled_size.w.max(1), scaled_size.h.max(1), 0, 0);
            }
            if let Some(viewport) = self.popup.viewport.as_ref() {
                viewport.set_destination(
                    self.popup.rectangle.size.w.max(1),
                    self.popup.rectangle.size.h.max(1),
                );
            }
            self.popup.damage_tracked_renderer =
                OutputDamageTracker::new(scaled_size, 1.0, smithay::utils::Transform::Flipped180);
            self.popup.c_popup.wl_surface().commit();
            popup_manager.commit(self.s_surface.wl_surface());

            self.popup.state = None;
        };
        self.s_surface.alive()
    }
}
