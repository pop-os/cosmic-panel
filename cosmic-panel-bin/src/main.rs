// SPDX-License-Identifier: MPL-2.0-only

use std::{collections::HashMap, os::unix::net::UnixStream};

use anyhow::Result;
use cosmic_panel_config::{CosmicPanelBackground, CosmicPanelContainerConfig};
use launch_pad::{ProcessKey, ProcessManager};
use sctk::reexports::client::backend::ObjectId;
use slog::{o, warn, Drain};
use smithay::reexports::{
    calloop,
    wayland_server::{backend::ClientId, Client},
};
use tokio::{runtime, sync::mpsc};
use xdg_shell_wrapper::{run, shared_state::GlobalState};

mod space;
mod space_container;

pub enum PanelCalloopMsg {
    Color([f32; 4]),
    ClientSocketPair(String, ClientId, Client, UnixStream),
}

fn main() -> Result<()> {
    let term_drain = slog_term::term_full().ignore_res();
    let journald_drain = slog_journald::JournaldDrain.ignore_res();
    let drain = slog::Duplicate::new(term_drain, journald_drain);
    let log = slog::Logger::root(slog_async::Async::default(drain.fuse()).fuse(), o!());

    let _guard = slog_scope::set_global_logger(log.clone());
    slog_stdlog::init().expect("Could not setup log backend");
    log_panics::init();

    let arg = std::env::args().nth(1);
    let usage = "USAGE: cosmic-panel";
    let config = match arg.as_ref().map(|s| &s[..]) {
        Some(arg) if arg == "--help" || arg == "-h" => {
            println!("{}", usage);
            std::process::exit(1);
        }
        None => match cosmic_panel_config::CosmicPanelContainerConfig::load() {
            Ok(c) => c,
            Err(e) => {
                warn!(
                    log.clone(),
                    "Falling back to default panel configuration: {}", e
                );
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

    let mut space = space_container::SpaceContainer::new(config, log, applet_tx);

    let (calloop_tx, calloop_rx) = calloop::channel::sync_channel(100);
    let event_loop = calloop::EventLoop::try_new()?;

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
                    };
                }
            });

            Ok(())
        });
    }

    run(space, event_loop)?;
    Ok(())
}
