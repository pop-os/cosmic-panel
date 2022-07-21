// SPDX-License-Identifier: MPL-2.0-only

use cosmic_panel_config::CosmicPanelContainerConfig;
use slog::Logger;
use smithay::backend::renderer::gles2::Gles2Renderer;

use crate::space::PanelSpace;

#[derive(Debug)]
pub struct SpaceContainer {
    pub(crate) space_list: Vec<PanelSpace>,
    pub(crate) renderer: Option<Gles2Renderer>,
}

impl SpaceContainer {
    pub fn new( config: CosmicPanelContainerConfig, log: Logger) -> Self {
        Self {
            space_list: config.config_list.iter().map(|c| PanelSpace::new(c.clone(), log.clone())).collect(),
            renderer: None,
        }
    }
}