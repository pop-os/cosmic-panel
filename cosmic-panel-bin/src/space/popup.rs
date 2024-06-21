use std::rc::Rc;

use crate::xdg_shell_wrapper::space::{ClientEglSurface, WrapperPopupState};
use cctk::wayland_client::Proxy;
use sctk::shell::xdg::popup;
use smithay::{
    backend::{egl::EGLSurface, renderer::gles::GlesRenderer},
    desktop::{PopupKind, PopupManager},
    utils::Rectangle,
    wayland::seat::WaylandFocus,
};
use wayland_egl::WlEglSurface;

use super::PanelSpace;

impl PanelSpace {
    pub(crate) fn close_popups(&mut self) {
        for t in &mut self.space.elements().filter_map(|w| w.toplevel()) {
            for (p, _) in PopupManager::popups_for_surface(t.wl_surface()) {
                match p {
                    PopupKind::Xdg(p) => {
                        if !self.s_hovered_surface.iter().any(|hs| {
                            hs.surface.wl_surface().is_some_and(|s| s.as_ref() == t.wl_surface())
                        }) {
                            p.send_popup_done();
                        }
                    },
                    PopupKind::InputMethod(_) => {
                        // TODO handle IME
                        continue;
                    },
                }
            }
        }
        self.popups.clear();
    }

    pub fn configure_panel_popup(
        &mut self,
        popup: &sctk::shell::xdg::popup::Popup,
        config: sctk::shell::xdg::popup::PopupConfigure,
        renderer: Option<&mut GlesRenderer>,
    ) {
        let Some(renderer) = renderer else {
            return;
        };

        if let Some((p, s_popup)) = self
            .popups
            .iter_mut()
            .map(|p| (&mut p.popup, Some(&mut p.s_surface)))
            .chain(self.overflow_popup.iter_mut().map(|p| (&mut p.0, None)))
            .find(|(p, _)| popup.wl_surface() == p.c_popup.wl_surface())
        {
            // use the size that we have already
            p.wrapper_rectangle =
                Rectangle::from_loc_and_size(config.position, (config.width, config.height));

            let (width, height) = (config.width, config.height);
            p.state = match p.state {
                None | Some(WrapperPopupState::WaitConfigure) => None,
                Some(r) => Some(r),
            };

            if let Some(s) = s_popup {
                _ = s.send_configure()
            }

            match config.kind {
                popup::ConfigureKind::Initial => {
                    let wl_egl_surface =
                        match WlEglSurface::new(p.c_popup.wl_surface().id(), width, height) {
                            Ok(s) => s,
                            Err(_) => return,
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
                },
                popup::ConfigureKind::Reactive => {},
                popup::ConfigureKind::Reposition { token: _token } => {},
                _ => {},
            };
        }
    }
}
