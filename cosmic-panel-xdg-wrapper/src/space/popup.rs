// SPDX-License-Identifier: MPL-2.0-only

use std::cell::Cell;
use std::rc::Rc;

use sctk::reexports::{client::protocol::wl_surface as c_wl_surface, client::Main};
use smithay::{
    backend::egl::surface::EGLSurface,
    reexports::wayland_protocols::xdg_shell::client::{
        xdg_popup::XdgPopup, xdg_surface::XdgSurface,
    },
    wayland::shell::xdg::PopupSurface,
};

#[derive(PartialEq, Copy, Clone, Debug)]
pub enum PopupRenderEvent {
    WaitConfigure,
    Configure {
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    },
    Repositioned(u32),
    Closed,
}

#[derive(Debug)]
pub struct Popup {
    pub c_popup: Main<XdgPopup>,
    pub c_xdg_surface: Main<XdgSurface>,
    pub c_surface: c_wl_surface::WlSurface,
    pub s_surface: PopupSurface,
    pub egl_surface: Rc<EGLSurface>,
    pub next_render_event: Rc<Cell<Option<PopupRenderEvent>>>,
    pub dirty: bool,
    pub should_render: bool,
}

impl Drop for Popup {
    fn drop(&mut self) {
        self.s_surface.send_popup_done();
        self.c_popup.destroy();
        self.c_xdg_surface.destroy();
        self.c_surface.destroy();
    }
}
