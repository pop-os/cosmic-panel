use std::collections::HashMap;

use crate::space_container::SpaceContainer;
use cosmic_config::ConfigGet;
use cosmic_panel_config::{CosmicPanelConfig, CosmicPanelContainerConfig};
use notify::RecommendedWatcher;
use smithay::reexports::calloop::{channel, LoopHandle};
use tracing::error;
use xdg_shell_wrapper::shared_state::GlobalState;

#[derive(Debug, Clone)]
enum ConfigUpdate {
    Entries(Vec<String>),
    EntryChanged(String),
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

                    let (entry, errors) = CosmicPanelConfig::get_entry(&cosmic_config);

                    for error in errors {
                        error!("Failed to get entry value: {:?}", error);
                    }
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

                let (entry, errors) = CosmicPanelConfig::get_entry(&cosmic_config);

                for error in errors {
                    error!("Failed to get entry value: {:?}", error);
                }

                state.space.update_space(
                    entry,
                    &state.client_state.compositor_state,
                    &mut state.client_state.layer_state,
                    &state.client_state.queue_handle,
                );
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
            .watch(move |_helper, _keys| {
                entries_tx_clone
                    .send(ConfigUpdate::EntryChanged(name_clone.clone()))
                    .expect("Failed to send Config Update");
            })
            .expect("Failed to watch cosmic config");
        watchers.insert(entry.name.clone(), watcher);
    }

    Ok(watchers)
}
