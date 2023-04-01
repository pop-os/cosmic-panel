use crate::CosmicPanelConfig;
use cosmic_config::{Config, ConfigGet, ConfigSet};
use serde::{Deserialize, Serialize};
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

pub const NAME: &str = "com.system76.CosmicPanel";
pub const VERSION: u64 = 1;

impl CosmicPanelContainerConfig {
    /// load config with the provided name
    pub fn load() -> Result<(Self, Vec<cosmic_config::Error>), cosmic_config::Error> {
        let config = Self::cosmic_config()?;
        let entry_names = config.get::<Vec<String>>("entries")?;
        let mut config_list = Vec::new();
        let mut entry_errors = Vec::new();

        for name in entry_names {
            let config = Config::new(format!("{}.{}", NAME, name).as_str(), VERSION)?;
            let (entry, mut errors) = CosmicPanelConfig::get_entry(&config);
            config_list.push(entry);
            entry_errors.append(&mut errors);
        }
        Ok((Self { config_list }, entry_errors))
    }

    pub fn cosmic_config() -> Result<Config, cosmic_config::Error> {
        Config::new(NAME, VERSION)
    }

    pub fn write_entries(&self) -> Result<(), cosmic_config::Error> {
        let config = Self::cosmic_config()?;
        let entry_names = self
            .config_list
            .iter()
            .map(|c| c.name.clone())
            .collect::<Vec<_>>();
        config.set("entries", entry_names)?;
        for entry in &self.config_list {
            entry.write_entry()?;
        }
        Ok(())
    }
}

impl Default for CosmicPanelContainerConfig {
    fn default() -> Self {
        ron::de::from_str(include_str!("../config.ron")).unwrap()
    }
}
