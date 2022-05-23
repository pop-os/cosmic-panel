// SPDX-License-Identifier: MPL-2.0-only

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Result;
use libc::{c_int, c_void};
use sctk::reexports::client;
use smithay::{
    backend::egl::{
        display::EGLDisplayHandle,
        ffi,
        native::{EGLNativeDisplay, EGLNativeSurface, EGLPlatform},
        wrap_egl_call, EGLError,
    },
    egl_platform,
    utils::{Logical, Point},
    wayland::shell::xdg::PopupSurface,
};

#[derive(Debug)]
pub struct ClientEglSurface {
    pub(crate) wl_egl_surface: wayland_egl::WlEglSurface,
    pub(crate) display: client::Display,
}

static SURFACE_ATTRIBUTES: [c_int; 3] = [
    ffi::egl::RENDER_BUFFER as c_int,
    ffi::egl::BACK_BUFFER as c_int,
    ffi::egl::NONE as c_int,
];

impl EGLNativeDisplay for ClientEglSurface {
    fn supported_platforms(&self) -> Vec<EGLPlatform<'_>> {
        let display: *mut c_void = self.display.c_ptr() as *mut _;
        vec![
            // see: https://www.khronos.org/registry/EGL/extensions/KHR/EGL_KHR_platform_wayland.txt
            egl_platform!(PLATFORM_WAYLAND_KHR, display, &["EGL_KHR_platform_wayland"]),
            // see: https://www.khronos.org/registry/EGL/extensions/EXT/EGL_EXT_platform_wayland.txt
            egl_platform!(PLATFORM_WAYLAND_EXT, display, &["EGL_EXT_platform_wayland"]),
        ]
    }
}

unsafe impl EGLNativeSurface for ClientEglSurface {
    fn create(
        &self,
        display: &Arc<EGLDisplayHandle>,
        config_id: ffi::egl::types::EGLConfig,
    ) -> Result<*const c_void, EGLError> {
        let ptr = self.wl_egl_surface.ptr();
        if ptr.is_null() {
            panic!("recieved a null pointer for the wl_egl_surface.");
        }
        wrap_egl_call(|| unsafe {
            ffi::egl::CreatePlatformWindowSurfaceEXT(
                display.handle,
                config_id,
                ptr as *mut _,
                SURFACE_ATTRIBUTES.as_ptr(),
            )
        })
    }

    fn resize(&self, width: i32, height: i32, dx: i32, dy: i32) -> bool {
        wayland_egl::WlEglSurface::resize(&self.wl_egl_surface, width, height, dx, dy);
        true
    }
}

pub enum ServerSurface {
    TopLevel(Point<i32, Logical>, Rc<RefCell<smithay::desktop::Window>>),
    Popup(
        Point<i32, Logical>,
        Rc<RefCell<smithay::desktop::Window>>,
        PopupSurface,
    ),
}
