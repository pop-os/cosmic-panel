// SPDX-License-Identifier: MPL-2.0

use std::ffi::{c_int, c_void};
use std::sync::Arc;

use anyhow::Result;
use sctk::reexports::client::Proxy;
use sctk::reexports::client::protocol::wl_display::WlDisplay;
use sctk::reexports::client::protocol::wl_surface::WlSurface;
use smithay::backend::egl::display::EGLDisplayHandle;
use smithay::backend::egl::native::{EGLNativeDisplay, EGLNativeSurface, EGLPlatform};
use smithay::backend::egl::{EGLError, ffi, wrap_egl_call_ptr};
use smithay::egl_platform;

/// Client Egl surface
#[derive(Debug)]
pub struct ClientEglSurface {
    // XXX implicitly drops wl_egl_surface first before _wl_surface
    /// egl surface
    pub wl_egl_surface: wayland_egl::WlEglSurface,
    /// wl surface
    _wl_surface: WlSurface,
}

impl ClientEglSurface {
    /// Create a Client Egl Surface
    /// must be dropped before the associated WlSurface is destroyed
    pub unsafe fn new(wl_egl_surface: wayland_egl::WlEglSurface, _wl_surface: WlSurface) -> Self {
        Self { wl_egl_surface, _wl_surface }
    }
}

#[derive(Debug)]
/// wrapper around WlDisplay
pub struct ClientEglDisplay {
    /// client display
    pub display: WlDisplay,
}

static SURFACE_ATTRIBUTES: [c_int; 1] = [ffi::egl::NONE as c_int];

impl EGLNativeDisplay for ClientEglDisplay {
    fn supported_platforms(&self) -> Vec<EGLPlatform<'_>> {
        let display: *mut c_void = self.display.id().as_ptr() as *mut _;
        vec![
            // see: https://www.khronos.org/registry/EGL/extensions/KHR/EGL_KHR_platform_wayland.txt
            egl_platform!(PLATFORM_WAYLAND_KHR, display, &["EGL_KHR_platform_wayland"]),
            // see: https://www.khronos.org/registry/EGL/extensions/EXT/EGL_EXT_platform_wayland.txt
            egl_platform!(PLATFORM_WAYLAND_EXT, display, &["EGL_EXT_platform_wayland"]),
        ]
    }
}

unsafe impl EGLNativeSurface for ClientEglSurface {
    unsafe fn create(
        &self,
        display: &Arc<EGLDisplayHandle>,
        config_id: ffi::egl::types::EGLConfig,
    ) -> Result<*const c_void, EGLError> {
        let ptr = self.wl_egl_surface.ptr();
        if ptr.is_null() {
            return Err(EGLError::BadNativeWindow);
        }
        wrap_egl_call_ptr(|| unsafe {
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
