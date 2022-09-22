// SPDX-License-Identifier: MPL-2.0-only

use std::{cell::RefCell, rc::Rc};

use cosmic_panel_config::CosmicPanelContainerConfig;
use launch_pad::process::Process;
use slog::Logger;
use smithay::{backend::renderer::gles2::Gles2Renderer, reexports::wayland_server};
use tokio::sync::mpsc;
use xdg_shell_wrapper::client_state::ClientFocus;

use crate::space::PanelSpace;

#[derive(Debug)]
pub struct SpaceContainer {
    pub(crate) config: CosmicPanelContainerConfig,
    pub(crate) space_list: Vec<PanelSpace>,
    pub(crate) renderer: Option<Gles2Renderer>,
    pub(crate) s_display: Option<wayland_server::DisplayHandle>,
    pub(crate) c_focused_surface: Rc<RefCell<ClientFocus>>,
    pub(crate) c_hovered_surface: Rc<RefCell<ClientFocus>>,
    pub log: Logger,
    pub applet_tx: mpsc::Sender<Process>,
}

impl SpaceContainer {
    pub fn new(config: CosmicPanelContainerConfig, log: Logger, tx: mpsc::Sender<Process>) -> Self {
        Self {
            config,
            log,
            space_list: vec![],
            renderer: None,
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
}
