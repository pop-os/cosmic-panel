// SPDX-License-Identifier: MPL-2.0-only

use std::{cell::RefCell, os::unix::net::UnixStream, rc::Rc};

use crate::space::{AppletMsg, PanelSpace};
use cosmic_panel_config::CosmicPanelContainerConfig;
use slog::Logger;
use smithay::{
    backend::{egl::EGLDisplay, renderer::gles2::Gles2Renderer},
    reexports::wayland_server::{self, backend::ClientId, Client},
};
use tokio::sync::mpsc;
use wayland_server::Resource;
use xdg_shell_wrapper::client_state::ClientFocus;

#[derive(Debug)]
pub struct SpaceContainer {
    pub(crate) config: CosmicPanelContainerConfig,
    pub(crate) space_list: Vec<PanelSpace>,
    pub(crate) renderer: Option<Gles2Renderer>,
    pub(crate) egl_display: Option<EGLDisplay>,
    pub(crate) s_display: Option<wayland_server::DisplayHandle>,
    pub(crate) c_focused_surface: Rc<RefCell<ClientFocus>>,
    pub(crate) c_hovered_surface: Rc<RefCell<ClientFocus>>,
    pub log: Logger,
    pub applet_tx: mpsc::Sender<AppletMsg>,
}

impl SpaceContainer {
    pub fn new(
        config: CosmicPanelContainerConfig,
        log: Logger,
        tx: mpsc::Sender<AppletMsg>,
    ) -> Self {
        Self {
            config,
            log: log.clone(),
            space_list: vec![],
            renderer: None,
            egl_display: None,
            s_display: None,
            c_focused_surface: Default::default(),
            c_hovered_surface: Default::default(),
            applet_tx: tx,
        }
    }

    pub fn set_theme_window_color(&mut self, color: [f32; 4]) {
        for space in &mut self.space_list {
            space.set_theme_window_color(color);
        }
    }

    pub fn replace_client(
        &mut self,
        id: String,
        old_client_id: ClientId,
        client: Client,
        socket: UnixStream,
    ) {
        for s in &mut self.space_list {
            if let Some((_, s_client, s_socket)) = s
                .clients_left
                .iter_mut()
                .chain(s.clients_center.iter_mut())
                .chain(s.clients_right.iter_mut())
                .find(|(c_id, old_client, _)| c_id == &id && old_client_id == old_client.id())
            {
                // cleanup leftover windows
                let w = {
                    s.space
                        .elements()
                        .find(|w| {
                            w.toplevel().wl_surface().client().map(|c| c.id())
                                == Some(s_client.id())
                        })
                        .cloned()
                };
                if let Some(w) = w {
                    s.space.unmap_elem(&w);
                }
                // TODO Popups?

                *s_client = client;
                *s_socket = socket;
                s.full_clear = 4;
                // s.w_accumulated_damage.clear();
                break;
            }
        }
    }
}
