// SPDX-License-Identifier: MPL-2.0-only

use std::{collections::HashMap, os::unix::net::UnixStream};

use anyhow::Result;
use config_watching::watch_config;
use cosmic_panel_config::{CosmicPanelBackground, CosmicPanelContainerConfig};
use launch_pad::{ProcessKey, ProcessManager};
use panel_dbus::PanelDbus;
use sctk::reexports::client::backend::ObjectId;
use smithay::reexports::{
    calloop,
    wayland_server::{backend::ClientId, Client},
};
use tokio::{runtime, sync::mpsc};
use tracing::warn;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use xdg_shell_wrapper::{run, shared_state::GlobalState};
use zbus::ConnectionBuilder;

mod config_watching;
mod panel_dbus;
mod space;
mod space_container;

pub enum PanelCalloopMsg {
    Color([f32; 4]),
    ClientSocketPair(String, ClientId, Client, UnixStream),
}

fn main() -> Result<()> {
    let fmt_layer = fmt::layer().with_target(false);
    let filter_layer = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("info"))
        .unwrap();
    if let Ok(journal_layer) = tracing_journald::layer() {
        tracing_subscriber::registry()
            .with(fmt_layer)
            .with(journal_layer)
            .with(filter_layer)
            .init();
    } else {
        tracing_subscriber::registry()
            .with(fmt_layer)
            .with(filter_layer)
            .init();
    }

    log_panics::init();

    let arg = std::env::args().nth(1);
    let usage = "USAGE: cosmic-panel";
    let config = match arg.as_ref().map(|s| &s[..]) {
        Some(arg) if arg == "--help" || arg == "-h" => {
            println!("{}", usage);
            std::process::exit(1);
        }
        None => match cosmic_panel_config::CosmicPanelContainerConfig::load() {
            Ok((c, errors)) => {
                for e in errors {
                    warn!("Panel Entry Error: {:?}", e);
                }
                c
            }
            Err(e) => {
                warn!("Falling back to default panel configuration: {:?}", e);
                CosmicPanelContainerConfig::default()
            }
        },
        _ => {
            println!("{}", usage);
            std::process::exit(1);
        }
    };

    let (applet_tx, mut applet_rx) = mpsc::channel(200);
    let (unpause_launchpad_tx, unpause_launchpad_rx) = std::sync::mpsc::sync_channel(200);

    let mut space = space_container::SpaceContainer::new(config, applet_tx);

    let (calloop_tx, calloop_rx) = calloop::channel::sync_channel(100);
    let event_loop = calloop::EventLoop::try_new()?;

    let handle = event_loop.handle();
    match watch_config(&space.config, handle) {
        Ok(watchers) => {
            space.watchers = watchers;
        }
        Err(e) => warn!("Failed to watch config: {:?}", e),
    };

    event_loop
        .handle()
        .insert_source(
            calloop_rx,
            move |e, _, state: &mut GlobalState<space_container::SpaceContainer>| {
                match e {
                    calloop::channel::Event::Msg(e) => match e {
                        PanelCalloopMsg::Color(c) => state.space.set_theme_window_color(c),
                        PanelCalloopMsg::ClientSocketPair(id, client_id, c, s) => {
                            state.space.replace_client(id, client_id, c, s);
                            unpause_launchpad_tx
                                .try_send(())
                                .expect("Failed to unblock launchpad");
                        }
                    },
                    calloop::channel::Event::Closed => {}
                };
            },
        )
        .expect("failed to insert dbus event source");

    if space
        .config
        .config_list
        .iter()
        .any(|c| matches!(c.background, CosmicPanelBackground::ThemeDefault(_)))
    {
        let t = cosmic_theme::Theme::dark_default();
        space.set_theme_window_color([
            t.bg_color().red,
            t.bg_color().green,
            t.bg_color().blue,
            t.bg_color().alpha,
        ]);

        // TODO load theme once theme colors are supported in cosmic apps
        // let path = xdg::BaseDirectories::with_prefix("gtk-4.0")
        //     .ok()
        //     .and_then(|xdg_dirs| xdg_dirs.find_config_file("cosmic.css"))
        //     .unwrap_or_else(|| "~/.config/gtk-4.0/cosmic.css".into());
        // if let Ok(xdg_dirs) = xdg::BaseDirectories::with_prefix(NAME) {
        //     // initital send of color
        //     space.set_theme_window_color(get_color(&path).unwrap_or([0.5, 0.5, 0.5, 0.5]));
        //     // Automatically select the best implementation for your platform.
        //     // You can also access each implementation directly e.g. INotifyWatcher.
        //     let color_tx_clone = calloop_tx.clone();
        //     if let Ok(mut watcher) = RecommendedWatcher::new(
        //         move |res: Result<notify::Event, notify::Error>| {
        //             if let Ok(e) = res {
        //                 let color_tx = color_tx_clone.clone();
        //                 match e.kind {
        //                     // TODO only notify for changed data file if it is the active file
        //                     notify::EventKind::Create(_)
        //                     | notify::EventKind::Modify(_)
        //                     | notify::EventKind::Remove(_) => {
        //                         let _ = color_tx.send(PanelCalloopMsg::Color(
        //                             get_color(&path).unwrap_or([0.5, 0.5, 0.5, 0.5]),
        //                         ));
        //                     }
        //                     _ => {}
        //                 }
        //             }
        //         },
        //         notify::Config::default(),
        //     ) {
        //         for config_dir in xdg_dirs.get_config_dirs() {
        //             let _ = watcher.watch(&config_dir, RecursiveMode::Recursive);
        //         }
        //         for data_dir in xdg_dirs.get_data_dirs() {
        //             let _ = watcher.watch(data_dir.as_ref(), RecursiveMode::Recursive);
        //         }

        //         for data_dir in xdg_dirs.get_data_dirs() {
        //             let _ = watcher.watch(data_dir.as_ref(), RecursiveMode::Recursive);
        //         }
        //     }
        // }

        std::thread::spawn(move || -> anyhow::Result<()> {
            let rt = runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let mut process_ids: HashMap<ObjectId, Vec<ProcessKey>> = HashMap::new();

            rt.block_on(async {
                let process_manager = ProcessManager::new().await;
                let _ = process_manager.set_max_restarts(100);
                let conn = ConnectionBuilder::session()
                    .ok()
                    .and_then(|conn| conn.name("com.system76.CosmicPanel").ok())
                    .and_then(|conn| {
                        conn.serve_at(
                            "/com/system76/CosmicPanel",
                            PanelDbus {
                                notification_ids: Vec::new(),
                            },
                        )
                        .ok()
                    })
                    .map(|conn| conn.build());
                let conn = match conn {
                    Some(conn) => conn.await.ok(),
                    None => None,
                };
                let mut id_map = HashMap::new();

                while let Some(msg) = applet_rx.recv().await {
                    match msg {
                        space::AppletMsg::NewProcess(id, process) => {
                            if let Ok(key) = process_manager.start(process).await {
                                let entry = process_ids.entry(id).or_insert_with(|| Vec::new());
                                entry.push(key);
                            }
                        }
                        space::AppletMsg::ClientSocketPair(id, client_id, c, s) => {
                            let _ = calloop_tx
                                .send(PanelCalloopMsg::ClientSocketPair(id, client_id, c, s));
                            // XXX This is done to avoid a possible race,
                            // the client & socket need to be update in the panel_space state before the process starts again
                            let _ = unpause_launchpad_rx.recv();
                        }
                        space::AppletMsg::Cleanup(id) => {
                            for id in process_ids.remove(&id).unwrap_or_default() {
                                let _ = process_manager.stop_process(id).await;
                            }
                        }
                        space::AppletMsg::NotificationId(object, id) => {
                            id_map.insert(object, id);
                            if let Some(conn) = &conn {
                                let object_server = conn.object_server();
                                let iface_ref = object_server
                                    .interface::<_, PanelDbus>("/com/system76/CosmicPanel")
                                    .await
                                    .expect("Failed to get interface");
                                let mut iface = iface_ref.get_mut().await;
                                iface.notification_ids = id_map.values().cloned().collect();
                                let _ = iface
                                    .notification_ids_changed(iface_ref.signal_context())
                                    .await;
                            }
                        }
                    };
                }
            });

            Ok(())
        });
    }
    run(space, event_loop)?;
    Ok(())
}
