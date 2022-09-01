use std::fs::File;

use crate::CosmicPanelConfig;
use serde::{Deserialize, Serialize};
use xdg::BaseDirectories;
use xdg_shell_wrapper_config::{WrapperConfig, WrapperOutput};

/// Config structure for the cosmic panel
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
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

static CONFIG_PATH: &str = "cosmic-panel/config.ron";

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
        ron::de::from_str(include_str!("../config.ron")).unwrap()
    }
}
