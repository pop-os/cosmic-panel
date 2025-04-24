use crate::xdg_shell_wrapper::space::{ClientEglSurface, PanelPopup, WrapperPopupState};
use cctk::wayland_client::Proxy;
use sctk::shell::xdg::popup::{self};
use smithay::{
    backend::{egl::EGLSurface, renderer::gles::GlesRenderer},
    utils::Rectangle,
    wayland::seat::WaylandFocus,
};
use wayland_egl::WlEglSurface;

use super::PanelSpace;

impl PanelSpace {
    pub(crate) fn close_popups<'a>(&mut self, exclude: impl Fn(&PanelPopup) -> bool) {
        tracing::info!("Closing popups");
        let mut to_destroy = Vec::with_capacity(self.popups.len());
        self.popups.retain_mut(|p| {
            if exclude(&p.popup) {
                return true;
            }

            tracing::info!("Closing popup: {:?}", p.popup.c_popup.wl_surface());
            p.s_surface.send_popup_done();
            to_destroy.push((
                p.popup.c_popup.xdg_popup().clone(),
                p.popup.c_popup.wl_surface().clone(),
                Some(p.s_surface.wl_surface().clone()),
            ));
            false
        });
        if self.overflow_popup.as_ref().is_some_and(|(p, _)| !exclude(p)) {
            let (popup, _) = self.overflow_popup.take().unwrap();
            tracing::info!("Closing overflow popup: {:?}", popup.c_popup.wl_surface());
            to_destroy.push((
                popup.c_popup.xdg_popup().clone(),
                popup.c_popup.wl_surface().clone(),
                None,
            ));
        }

        for (popup, surface, s_surface) in to_destroy {
            self.c_focused_surface.borrow_mut().retain(|s| s.0 != surface);
            self.c_hovered_surface.borrow_mut().retain(|s| s.0 != surface);

            if let Some(s_surface) = s_surface {
                self.s_focused_surface
                    .retain(|s| !s.0.wl_surface().is_some_and(|s| s.as_ref() == &s_surface));
                self.s_hovered_surface
                    .retain(|s| !s.surface.wl_surface().is_some_and(|s| s.as_ref() == &s_surface));
            }
            popup.destroy();
            surface.destroy();
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
            let new_rect = Rectangle::new(config.position.into(), (width, height).into());
            p.wrapper_rectangle = new_rect;

            p.state = Some(WrapperPopupState::Rectangle {
                x: config.position.0,
                y: config.position.1,
                width: config.width,
                height: config.height,
            });

            if let Some(s) = s_popup {
                _ = s.send_configure()
            }

            match config.kind {
                popup::ConfigureKind::Initial => {
                    tracing::info!("Popup Initial Configure");
                    let width_scaled = (width as f64 * self.scale).ceil() as i32;
                    let height_scaled = (height as f64 * self.scale).ceil() as i32;
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
                    let egl_surface = unsafe {
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
                    };
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
            p.wrapper_rectangle = Rectangle::new(config.position.into(), (width, height).into());

            p.state = match p.state {
                None | Some(WrapperPopupState::WaitConfigure) => None,
                Some(r) => Some(r),
            };

            match config.kind {
                popup::ConfigureKind::Initial => {
                    tracing::info!("Popup Initial Configure");
                    let width_scaled = (width as f64 * self.scale).ceil() as i32;
                    let height_scaled = (height as f64 * self.scale).ceil() as i32;
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
                    let egl_surface = unsafe {
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
                    };
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
