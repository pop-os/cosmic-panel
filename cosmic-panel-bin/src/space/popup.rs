use std::rc::Rc;

use crate::xdg_shell_wrapper::space::{ClientEglSurface, WrapperPopupState};
use cctk::wayland_client::Proxy;
use sctk::shell::xdg::popup::{self, Popup};
use smithay::{
    backend::{egl::EGLSurface, renderer::gles::GlesRenderer},
    desktop::{PopupKind, PopupManager},
    utils::Rectangle,
};
use wayland_egl::WlEglSurface;

use super::PanelSpace;

impl PanelSpace {
    pub(crate) fn close_popups<'a>(&mut self, exclude: impl AsRef<[Popup]>) {
        tracing::info!("Closing popups");
        let exclude = exclude.as_ref();

        self.popups.retain_mut(|p| {
            if exclude.iter().any(|e| e == &p.popup.c_popup) {
                return true;
            }

            tracing::info!("Closing popup: {:?}", p.popup.c_popup.wl_surface());
            p.s_surface.send_popup_done();
            p.popup.c_popup.xdg_popup().destroy();
            p.popup.c_popup.wl_surface().destroy();
            false
        });
        if self.overflow_popup.as_ref().is_some_and(|(p, _)| !exclude.contains(&p.c_popup)) {
            let (popup, _) = self.overflow_popup.take().unwrap();
            tracing::info!("Closing overflow popup: {:?}", popup.c_popup.wl_surface());
            popup.c_popup.xdg_popup().destroy();
            popup.c_popup.wl_surface().destroy();
        }
    }

    pub fn configure_panel_popup(
        &mut self,
        popup: &sctk::shell::xdg::popup::Popup,
        mut config: sctk::shell::xdg::popup::PopupConfigure,
        renderer: Option<&mut GlesRenderer>,
    ) {
        let Some(renderer) = renderer else {
            return;
        };

        if let Some((p, s_popup)) = self
            .popups
            .iter_mut()
            .map(|p| (&mut p.popup, Some(&mut p.s_surface)))
            .find(|(p, _)| popup.wl_surface() == p.c_popup.wl_surface())
        {
            tracing::info!("Configuring popup: {:?}", config);
            // use the size that we have already if the new size is 0
            if config.width == 0 {
                config.width = p.wrapper_rectangle.size.w;
            }
            if config.height == 0 {
                config.height = p.wrapper_rectangle.size.h;
            }
            let (width, height) = (config.width, config.height);
            p.wrapper_rectangle = Rectangle::from_loc_and_size(config.position, (width, height));

            p.state = match p.state {
                None | Some(WrapperPopupState::WaitConfigure) => None,
                Some(r) => Some(r),
            };

            if let Some(s) = s_popup {
                _ = s.send_configure()
            }

            match config.kind {
                popup::ConfigureKind::Initial => {
                    tracing::info!("Popup Initial Configure");
                    let wl_egl_surface =
                        match WlEglSurface::new(p.c_popup.wl_surface().id(), width, height) {
                            Ok(s) => s,
                            Err(err) => {
                                tracing::error!("Failed to create WlEglSurface: {:?}", err);
                                return;
                            },
                        };
                    let client_egl_surface = unsafe {
                        ClientEglSurface::new(wl_egl_surface, p.c_popup.wl_surface().clone())
                    };
                    let egl_surface = Rc::new(unsafe {
                        EGLSurface::new(
                            renderer.egl_context().display(),
                            renderer
                                .egl_context()
                                .pixel_format()
                                .expect("Failed to get pixel format from EGL context "),
                            renderer.egl_context().config_id(),
                            client_egl_surface,
                        )
                        .expect("Failed to initialize EGL Surface")
                    });
                    p.egl_surface.replace(egl_surface);
                    p.dirty = true;
                    tracing::info!("Popup configured");
                },
                popup::ConfigureKind::Reactive => {},
                popup::ConfigureKind::Reposition { token: _token } => {},
                _ => {},
            };
        } else if self
            .overflow_popup
            .as_ref()
            .is_some_and(|p| p.0.c_popup.wl_surface() == popup.wl_surface())
        {
            let (p, _) = self.overflow_popup.as_mut().unwrap();
            tracing::info!("Configuring overflow popup: {:?}", config);
            // use the size that we have already if the new size is 0
            if config.width == 0 {
                config.width = p.wrapper_rectangle.size.w;
            }
            if config.height == 0 {
                config.height = p.wrapper_rectangle.size.h;
            }
            let (width, height) = (config.width, config.height);
            p.wrapper_rectangle = Rectangle::from_loc_and_size(config.position, (width, height));

            p.state = match p.state {
                None | Some(WrapperPopupState::WaitConfigure) => None,
                Some(r) => Some(r),
            };

            match config.kind {
                popup::ConfigureKind::Initial => {
                    tracing::info!("Popup Initial Configure");
                    let width_scaled = (width as f64 * self.scale) as i32;
                    let height_scaled = (height as f64 * self.scale) as i32;
                    let wl_egl_surface = match WlEglSurface::new(
                        p.c_popup.wl_surface().id(),
                        width_scaled,
                        height_scaled,
                    ) {
                        Ok(s) => s,
                        Err(err) => {
                            tracing::error!("Failed to create WlEglSurface: {:?}", err);
                            return;
                        },
                    };
                    let client_egl_surface = unsafe {
                        ClientEglSurface::new(wl_egl_surface, p.c_popup.wl_surface().clone())
                    };
                    let egl_surface = Rc::new(unsafe {
                        EGLSurface::new(
                            renderer.egl_context().display(),
                            renderer
                                .egl_context()
                                .pixel_format()
                                .expect("Failed to get pixel format from EGL context "),
                            renderer.egl_context().config_id(),
                            client_egl_surface,
                        )
                        .expect("Failed to initialize EGL Surface")
                    });
                    p.egl_surface.replace(egl_surface);
                    p.dirty = true;
                    tracing::info!("Popup configured");
                },
                popup::ConfigureKind::Reactive => {},
                popup::ConfigureKind::Reposition { token: _token } => {},
                _ => {},
            };
        }
    }
}
