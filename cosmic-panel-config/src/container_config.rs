use crate::{CosmicPanelBackground, CosmicPanelConfig, CosmicPanelOuput};
use cosmic_config::{Config, ConfigGet, ConfigSet, CosmicConfigEntry};
use serde::{Deserialize, Serialize};
use tracing::warn;
use xdg_shell_wrapper_config::{Layer, WrapperConfig, WrapperOutput};

/// Config structure for the cosmic panel
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct CosmicPanelContainerConfig {
    pub config_list: Vec<CosmicPanelConfig>,
}

impl WrapperConfig for CosmicPanelContainerConfig {
    fn outputs(&self) -> WrapperOutput {
        self.config_list.iter().fold(WrapperOutput::Name(vec![]), |mut acc, c| {
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

pub const NAME: &str = "com.system76.CosmicPanel";
pub const VERSION: u64 = 1;

impl CosmicPanelContainerConfig {
    /// load config with the provided name
    pub fn load() -> Result<Self, (Vec<cosmic_config::Error>, Self)> {
        let config = match Self::cosmic_config() {
            Ok(config) => config,
            Err(e) => {
                warn!("Falling back to default panel configuration");
                return Err((vec![e], Self::default()));
            },
        };
        Self::load_from_config(&config, false)
    }

    pub fn load_from_config(
        config: &Config,
        system: bool,
    ) -> Result<Self, (Vec<cosmic_config::Error>, Self)> {
        let entry_names = match config.get::<Vec<String>>("entries") {
            Ok(names) => names,
            Err(e) => {
                warn!("Falling back to default panel configuration");
                return Err((vec![e], Self::default()));
            },
        };
        let mut config_list = Vec::new();
        let mut entry_errors = Vec::new();

        for name in entry_names {
            let config = match if system {
                Config::system(format!("{}.{}", NAME, name).as_str(), VERSION)
            } else {
                Config::new(format!("{}.{}", NAME, name).as_str(), VERSION)
            } {
                Ok(config) => config,
                Err(e) => {
                    entry_errors.push(e);
                    continue;
                },
            };
            match CosmicPanelConfig::get_entry(&config) {
                Ok(entry) => {
                    config_list.push(entry);
                },
                Err((mut errors, entry)) => {
                    config_list.push(entry);
                    entry_errors.append(&mut errors);
                },
            };
        }
        if entry_errors.is_empty() {
            Ok(Self { config_list })
        } else {
            Err((entry_errors, Self { config_list }))
        }
    }

    pub fn configs_for_output(&self, output_name: &str) -> Vec<&CosmicPanelConfig> {
        let mut configs: Vec<_> = self
            .config_list
            .iter()
            .filter(|c| match &c.output {
                CosmicPanelOuput::All => true,
                CosmicPanelOuput::Name(n) => n == output_name,
                _ => false,
            })
            .collect();
        configs.sort_by(|a, b| b.get_priority().cmp(&a.get_priority()));
        configs
    }

    pub fn cosmic_config() -> Result<Config, cosmic_config::Error> {
        Config::new(NAME, VERSION)
    }

    pub fn write_entries(&self) -> Result<(), cosmic_config::Error> {
        let config = Self::cosmic_config()?;
        let entry_names = self.config_list.iter().map(|c| c.name.clone()).collect::<Vec<_>>();
        config.set("entries", entry_names)?;
        for entry in &self.config_list {
            let config = Config::new(format!("{}.{}", NAME, entry.name).as_str(), VERSION)?;
            entry.write_entry(&config)?;
        }
        Ok(())
    }
}

impl Default for CosmicPanelContainerConfig {
    fn default() -> Self {
        Self {
            config_list: vec![
                CosmicPanelConfig {
                    name: "Panel".to_string(),
                    anchor: crate::PanelAnchor::Top,
                    anchor_gap: false,
                    layer: Layer::Top,
                    keyboard_interactivity:
                        xdg_shell_wrapper_config::KeyboardInteractivity::OnDemand,
                    size: crate::PanelSize::XS,
                    output: CosmicPanelOuput::All,
                    background: CosmicPanelBackground::ThemeDefault,
                    plugins_wings: Some((
                        vec![
                            "com.system76.CosmicPanelWorkspacesButton".to_string(),
                            "com.system76.CosmicPanelAppButton".to_string(),
                        ],
                        vec![
                            "com.system76.CosmicAppletInputSources".to_string(),
                            "com.system76.CosmicAppletStatusArea".to_string(),
                            "com.system76.CosmicAppletTiling".to_string(),
                            "com.system76.CosmicAppletAudio".to_string(),
                            "com.system76.CosmicAppletNetwork".to_string(),
                            "com.system76.CosmicAppletBattery".to_string(),
                            "com.system76.CosmicAppletNotifications".to_string(),
                            "com.system76.CosmicAppletBluetooth".to_string(),
                            "com.system76.CosmicAppletPower".to_string(),
                        ],
                    )),
                    plugins_center: Some(vec!["com.system76.CosmicAppletTime".to_string()]),
                    size_wings: None,
                    size_center: None,
                    expand_to_edges: true,
                    padding: 0,
                    spacing: 2,
                    border_radius: 0,
                    border_width: 0.0,
                    exclusive_zone: true,
                    autohide: None,
                    margin: 0,
                    opacity: 1.0,
                },
                CosmicPanelConfig {
                    name: "Dock".to_string(),
                    anchor: crate::PanelAnchor::Bottom,
                    anchor_gap: false,
                    layer: Layer::Top,
                    keyboard_interactivity:
                        xdg_shell_wrapper_config::KeyboardInteractivity::OnDemand,
                    size: crate::PanelSize::L,
                    output: CosmicPanelOuput::All,
                    background: CosmicPanelBackground::ThemeDefault,
                    plugins_wings: None,
                    plugins_center: Some(vec![
                        "com.system76.CosmicPanelLauncherButton".to_string(),
                        "com.system76.CosmicPanelWorkspacesButton".to_string(),
                        "com.system76.CosmicPanelAppButton".to_string(),
                        "com.system76.CosmicAppList".to_string(),
                        "com.system76.CosmicAppletMinimize".to_string(),
                    ]),
                    size_wings: None,
                    size_center: None,
                    expand_to_edges: false,
                    padding: 0,
                    spacing: 4,
                    border_radius: 160,
                    border_width: 0.0,
                    exclusive_zone: false,
                    autohide: Some(crate::AutoHide {
                        wait_time: 500,
                        transition_time: 200,
                        handle_size: 2,
                    }),
                    margin: 0,
                    opacity: 1.0,
                },
            ],
        }
    }
}
