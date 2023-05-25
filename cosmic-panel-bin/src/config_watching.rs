use std::collections::HashMap;

use crate::space_container::SpaceContainer;
use anyhow::anyhow;
use cosmic_config::{Config, ConfigGet, CosmicConfigEntry};
use cosmic_panel_config::{CosmicPanelConfig, CosmicPanelContainerConfig};
use cosmic_theme::{palette::Srgba, util::CssColor, Theme};
use notify::RecommendedWatcher;
use smithay::reexports::calloop::{channel, LoopHandle};
use tracing::error;
use xdg_shell_wrapper::shared_state::GlobalState;

#[derive(Debug, Clone)]
enum ConfigUpdate {
    Entries(Vec<String>),
    EntryChanged(String),
    Opacity(f32, String),
}

pub fn watch_cosmic_theme(
    handle: LoopHandle<GlobalState<SpaceContainer>>,
) -> Result<RecommendedWatcher, Box<dyn std::error::Error>> {
    let (entries_tx, entries_rx) = channel::sync_channel::<Srgba>(30);
    let entries_tx_clone = entries_tx.clone();
    let config_helper =
        Config::new("com.system76.CosmicTheme", 1).map_err(|e| anyhow!(format!("{:?}", e)))?;

    handle.insert_source(entries_rx, move |event, _, state| {
        match event {
            channel::Event::Msg(color) => {
                state.space.set_theme_window_color([
                    color.red,
                    color.green,
                    color.blue,
                    color.alpha,
                ]);
            }
            channel::Event::Closed => {}
        };
    })?;

    let theme_watcher = config_helper
        .watch(
            move |helper, _keys| match Theme::<CssColor>::get_entry(&helper) {
                Ok(entry) => {
                    entries_tx_clone.send(entry.bg_color()).unwrap();
                }
                Err((err, entry)) => {
                    for e in err {
                        error!("Failed to get theme entry value: {:?}", e);
                    }
                    entries_tx_clone.send(entry.bg_color()).unwrap();
                }
            },
        )
        .map_err(|e| anyhow!(format!("{:?}", e)))?;

    Ok(theme_watcher)
}

pub fn watch_config(
    config: &CosmicPanelContainerConfig,
    handle: LoopHandle<GlobalState<SpaceContainer>>,
) -> Result<HashMap<String, RecommendedWatcher>, Box<dyn std::error::Error>> {
    let (entries_tx, entries_rx) = channel::sync_channel::<ConfigUpdate>(30);

    let entries_tx_clone = entries_tx.clone();
    handle.insert_source(entries_rx, move |event, _, state| {
        match event {
            channel::Event::Msg(ConfigUpdate::Entries(entries)) => {
                let to_update = entries
                    .iter()
                    .filter(|c| !state.space.config.config_list.iter().any(|e| e.name == **c))
                    .map(|c| c.clone())
                    .collect::<Vec<String>>();

                for entry in to_update {
                    let cosmic_config = match CosmicPanelConfig::cosmic_config(&entry) {
                        Ok(config) => config,
                        Err(err) => {
                            error!("Failed to load cosmic config: {:?}", err);
                            return;
                        }
                    };

                    let entry = match CosmicPanelConfig::get_entry(&cosmic_config) {
                        Ok(entry) => entry,
                        Err((err, entry)) => {
                            for error in err {
                                error!("Failed to get entry value: {:?}", error);
                            }
                            entry
                        }
                    };

                    let entries_tx_clone = entries_tx_clone.clone();
                    let name_clone = entry.name.clone();
                    let helper = CosmicPanelConfig::cosmic_config(&name_clone)
                        .expect("Failed to load cosmic config");
                    let watcher = helper
                        .watch(move |_helper, _keys| {
                            entries_tx_clone
                                .send(ConfigUpdate::EntryChanged(name_clone.clone()))
                                .expect("Failed to send Config Update");
                        })
                        .expect("Failed to watch cosmic config");
                    state.space.watchers.insert(entry.name.clone(), watcher);

                    state.space.update_space(
                        entry,
                        &state.client_state.compositor_state,
                        &mut state.client_state.layer_state,
                        &state.client_state.queue_handle,
                    );
                }
                let to_remove = state
                    .space
                    .config
                    .config_list
                    .iter()
                    .filter(|c| !entries.contains(&c.name))
                    .map(|c| c.name.clone())
                    .collect::<Vec<String>>();
                for entry in to_remove {
                    state.space.remove_space(entry);
                }
            }
            channel::Event::Msg(ConfigUpdate::EntryChanged(entry)) => {
                let cosmic_config = match CosmicPanelConfig::cosmic_config(&entry) {
                    Ok(config) => config,
                    Err(err) => {
                        error!("Failed to load cosmic config: {:?}", err);
                        return;
                    }
                };

                let entry = match CosmicPanelConfig::get_entry(&cosmic_config) {
                    Ok(entry) => entry,
                    Err((err, entry)) => {
                        for error in err {
                            error!("Failed to get entry value: {:?}", error);
                        }
                        entry
                    }
                };

                state.space.update_space(
                    entry,
                    &state.client_state.compositor_state,
                    &mut state.client_state.layer_state,
                    &state.client_state.queue_handle,
                );
            }
            channel::Event::Msg(ConfigUpdate::Opacity(o, name)) => {
                state.space.set_opacity(o, name);
            }
            channel::Event::Closed => {}
        };
    })?;

    let cosmic_config_entries =
        CosmicPanelContainerConfig::cosmic_config().expect("Failed to load cosmic config");
    let entries_tx_clone = entries_tx.clone();
    let entries_watcher = cosmic_config_entries
        .watch(
            move |helper, keys| match helper.get::<Vec<String>>(&keys[0]) {
                Ok(entries) => {
                    entries_tx_clone
                        .send(ConfigUpdate::Entries(entries))
                        .expect("Failed to send entries");
                }
                Err(err) => {
                    error!("Failed to get entries: {:?}", err);
                }
            },
        )
        .expect("Failed to watch cosmic config");

    let mut watchers = HashMap::from([("entries".to_string(), entries_watcher)]);

    for entry in &config.config_list {
        let entries_tx_clone = entries_tx.clone();
        let name_clone = entry.name.clone();
        let helper =
            CosmicPanelConfig::cosmic_config(&name_clone).expect("Failed to load cosmic config");
        let watcher = helper
            .watch(move |helper, keys| {
                if keys.len() == 1 && keys[0] == "opacity" {
                    if let Ok(opacity) = helper.get::<f32>("opacity") {
                        entries_tx_clone
                            .send(ConfigUpdate::Opacity(opacity, name_clone.clone()))
                            .expect("Failed to send Config Update");
                    }
                } else {
                    entries_tx_clone
                        .send(ConfigUpdate::EntryChanged(name_clone.clone()))
                        .expect("Failed to send Config Update");
                }
            })
            .expect("Failed to watch cosmic config");
        watchers.insert(entry.name.clone(), watcher);
    }

    Ok(watchers)
}
