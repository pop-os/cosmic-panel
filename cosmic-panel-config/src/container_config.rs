use std::fs::File;

use crate::CosmicPanelConfig;
use crate::{AutoHide, CosmicPanelBackground, CosmicPanelOuput, PanelAnchor, PanelSize};
use serde::{Deserialize, Serialize};
use xdg::BaseDirectories;
use xdg_shell_wrapper_config::{KeyboardInteractivity, Layer, WrapperConfig, WrapperOutput};

/// Config structure for the cosmic panel
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CosmicPanelContainerConfig {
    pub config_list: Vec<CosmicPanelConfig>,
}

impl WrapperConfig for CosmicPanelContainerConfig {
    fn outputs(&self) -> WrapperOutput {
        self.config_list
            .iter()
            .fold(WrapperOutput::Name(vec![]), |mut acc, c| {
                let c_output = c.outputs();
                if matches!(acc, WrapperOutput::All) || matches!(c_output, WrapperOutput::All) {
                    return WrapperOutput::All;
                } else if let (WrapperOutput::Name(mut new_n), WrapperOutput::Name(acc_vec)) =
                    (c_output, &mut acc)
                {
                    acc_vec.append(&mut new_n);
                }
                acc
            })
    }

    fn name(&self) -> &str {
        "Cosmic Panel Config"
    }
}

static CONFIG_PATH: &'static str = "cosmic-panel/config.ron";

impl CosmicPanelContainerConfig {
    /// load config with the provided name
    pub fn load() -> anyhow::Result<Self> {
        let config_path =
            match BaseDirectories::new().map(|dirs| dirs.find_config_file(CONFIG_PATH)) {
                Ok(Some(path)) => path,
                _ => anyhow::bail!("Failed to get find config file"),
            };

        let file = match File::open(&config_path) {
            Ok(file) => file,
            Err(err) => {
                anyhow::bail!("Failed to open '{}': {}", config_path.display(), err);
            }
        };

        match ron::de::from_reader::<_, Self>(file) {
            Ok(config) => Ok(config),
            Err(err) => {
                anyhow::bail!("Failed to parse '{}': {}", config_path.display(), err);
            }
        }
    }

    /// write config to config file
    pub fn write(&self) -> anyhow::Result<()> {
        let xdg = BaseDirectories::new()?;
        let f = xdg.place_config_file(CONFIG_PATH).unwrap();
        let f = File::create(f)?;
        ron::ser::to_writer_pretty(&f, self, ron::ser::PrettyConfig::default())?;
        Ok(())
    }
}

impl Default for CosmicPanelContainerConfig {
    fn default() -> Self {
        Self {
            config_list: vec![
                CosmicPanelConfig {
                    name: "panel".to_string(),
                    anchor: PanelAnchor::Top,
                    anchor_gap: false,
                    layer: Layer::Top,
                    keyboard_interactivity: KeyboardInteractivity::OnDemand,
                    size: PanelSize::XS,
                    output: CosmicPanelOuput::All,
                    background: CosmicPanelBackground::Color([0.2, 0.2, 0.2, 0.8]),
                    plugins_wings: Some((
                        vec!["com.system76.CosmicAppletWorkspaces".to_string()],
                        vec![
                            "com.system76.CosmicAppletNetwork".to_string(),
                            "com.system76.CosmicAppletGraphics".to_string(),
                            "com.system76.CosmicAppletBattery".to_string(),
                            "com.system76.CosmicAppletNotifications".to_string(),
                            "com.system76.CosmicAppletPower".to_string(),
                        ],
                    )),
                    plugins_center: Some(vec!["com.system76.CosmicAppletTime".to_string()]),
                    expand_to_edges: false,
                    padding: 2,
                    spacing: 2,
                    exclusive_zone: true,
                    autohide: None,
                },
                CosmicPanelConfig {
                    name: "dock".to_string(),
                    anchor: PanelAnchor::Bottom,
                    anchor_gap: false,
                    layer: Layer::Top,
                    keyboard_interactivity: KeyboardInteractivity::OnDemand,
                    size: PanelSize::L,
                    output: CosmicPanelOuput::All,
                    background: CosmicPanelBackground::Color([0.2, 0.2, 0.2, 0.8]),
                    plugins_center: Some(vec!["com.system76.CosmicAppList".to_string()]),
                    plugins_wings: None,
                    expand_to_edges: true,
                    padding: 4,
                    spacing: 4,
                    exclusive_zone: false,
                    autohide: Some(AutoHide {
                        wait_time: 500,
                        transition_time: 200,
                        handle_size: 2,
                    }),
                },
            ],
        }
    }
}
