// SPDX-License-Identifier: MPL-2.0-only

use std::{cell::RefCell, rc::Rc};

use cosmic_panel_config::CosmicPanelContainerConfig;
use slog::Logger;
use smithay::{backend::renderer::gles2::Gles2Renderer, reexports::wayland_server};
use xdg_shell_wrapper::client_state::ClientFocus;

use crate::space::PanelSpace;

#[derive(Debug)]
pub struct SpaceContainer {
    pub(crate) config: CosmicPanelContainerConfig,
    pub(crate) space_list: Vec<PanelSpace>,
    pub(crate) renderer: Option<Gles2Renderer>,
    pub(crate) c_display: Option<wayland_server::DisplayHandle>,
    pub(crate) c_focused_surface: Rc<RefCell<ClientFocus>>,
    pub(crate) c_hovered_surface: Rc<RefCell<ClientFocus>>,
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

    pub fn set_theme_window_color(&mut self, color: [f32; 4]) {
        for space in &mut self.space_list {
            space.set_theme_window_color(color);
        }
    }
}
