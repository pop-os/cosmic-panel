// SPDX-License-Identifier: MPL-2.0-only

use std::{cell::RefCell, rc::Rc};

use cosmic_panel_config::CosmicPanelContainerConfig;
use sctk::reexports::client;
use slog::Logger;
use smithay::{
    backend::renderer::gles2::Gles2Renderer,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
};
use xdg_shell_wrapper::{client_state::ClientFocus, server_state::ServerFocus};

use crate::space::PanelSpace;

#[derive(Debug)]
pub struct SpaceContainer {
    pub(crate) config: CosmicPanelContainerConfig,
    pub(crate) space_list: Vec<PanelSpace>,
    pub(crate) renderer: Option<Gles2Renderer>,
    pub(crate) c_display: Option<client::Display>,
    pub(crate) c_focused_surface: ClientFocus,
    pub(crate) c_hovered_surface: ClientFocus,
    pub log: Logger,
}

impl SpaceContainer {
    pub fn new(config: CosmicPanelContainerConfig, log: Logger) -> Self {
        Self {
            config,
            log,
            space_list: vec![],
            renderer: None,
            c_display: None,
            c_focused_surface: Default::default(),
            c_hovered_surface: Default::default(),
        }
    }
}
