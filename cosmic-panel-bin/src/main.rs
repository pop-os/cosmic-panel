mod config_watching;
mod notifications;
mod space;
mod space_container;

use anyhow::Result;
use config_watching::{watch_config, watch_cosmic_theme};
use launch_pad::{ProcessKey, ProcessManager};
use notifications::notifications_conn;
use sctk::reexports::client::backend::ObjectId;
use smithay::reexports::{
    calloop,
    wayland_server::{backend::ClientId, Client},
};
use std::{
    collections::HashMap,
    mem,
    os::{fd::IntoRawFd, unix::net::UnixStream},
    time::Duration,
};
use tokio::{runtime, sync::mpsc};
use tracing::{error, info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use xdg_shell_wrapper::{run, shared_state::GlobalState, server_state::ServerState, client_state::ClientState};

pub enum PanelCalloopMsg {
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
            Ok(c) => c,
            Err((errors, c)) => {
                for e in errors {
                    error!("Panel Entry Error: {:?}", e);
                }
                let _ = c.write_entries();
                c
            }
        },
        _ => {
            println!("{}", usage);
            std::process::exit(1);
        }
    };

    let (applet_tx, mut applet_rx) = mpsc::channel(200);
    let (unpause_launchpad_tx, unpause_launchpad_rx) = std::sync::mpsc::sync_channel(200);

    let mut space = space_container::SpaceContainer::new(config, applet_tx.clone());

    let (calloop_tx, calloop_rx) = calloop::channel::sync_channel(100);
    let event_loop = calloop::EventLoop::try_new()?;

    let handle = event_loop.handle();
    match watch_config(&space.config, handle) {
        Ok(watchers) => {
            info!("Watching panel config successful");
            space.watchers = watchers;
        }
        Err(e) => warn!("Failed to watch config: {:?}", e),
    };
    match watch_cosmic_theme(event_loop.handle()) {
        Ok(w) => mem::forget(w),
        Err(e) => error!("Error while watching cosmic theme: {:?}", e),
    };

    event_loop
        .handle()
        .insert_source(
            calloop_rx,
            move |e, _, state: &mut GlobalState<space_container::SpaceContainer>| {
                match e {
                    calloop::channel::Event::Msg(e) => match e {
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

    std::thread::spawn(move || -> anyhow::Result<()> {
        let rt = runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let mut process_ids: HashMap<ObjectId, Vec<ProcessKey>> = HashMap::new();

        rt.block_on(async move {
            let process_manager = ProcessManager::new().await;
            let _ = process_manager
                .set_restart_mode(launch_pad::RestartMode::ExponentialBackoff(
                    Duration::from_millis(2),
                ))
                .await;
            let _ = process_manager.set_max_restarts(999999).await;

            let mut notifications_proxy =
                match tokio::time::timeout(Duration::from_secs(1), notifications_conn()).await {
                    Ok(Ok(p)) => Some(p),
                    err => {
                        error!("Failed to connect to the notifications daemon {:?}", err);
                        None
                    }
                };

            while let Some(msg) = applet_rx.recv().await {
                match msg {
                    space::AppletMsg::NewProcess(id, process) => {
                        if let Ok(key) = process_manager.start(process).await {
                            let entry = process_ids.entry(id).or_insert_with(|| Vec::new());
                            entry.push(key);
                        }
                    }
                    space::AppletMsg::NewNotificationsProcess(id, mut process, mut env) => {
                        let Some(proxy) = notifications_proxy.as_mut() else {
                            
                                notifications_proxy = match tokio::time::timeout(Duration::from_secs(1), notifications_conn()).await {
                                    Ok(Ok(p)) => Some(p),
                                    err => {
                                        error!("Failed to connect to the notifications daemon {:?}", err);
                                        None
                                    }
                                };
                            warn!("Can't start notifications applet without a connection");
                            continue;
                        };
                        info!("Getting fd for notifications applet");
                        let fd = match tokio::time::timeout(Duration::from_secs(1), proxy.get_fd())
                            .await
                        {
                            Ok(Ok(fd)) => fd,
                            Ok(Err(err)) => {
                                error!("Failed to get fd for the notifications applet {}", err);
                                continue;
                            }
                            Err(err) => {
                                error!("Failed to get fd for the notifications applet {}", err);
                                continue;
                            }
                        };
                        let fd = fd.into_raw_fd();
                        env.push(("COSMIC_NOTIFICATIONS".to_string(), fd.to_string()));
                        process = process.with_fds(move || vec![fd]);
                        process = process.with_env(env);
                        info!("Starting notifications applet");
                        if let Ok(key) = process_manager.start(process).await {
                            let entry = process_ids.entry(id).or_insert_with(|| Vec::new());
                            entry.push(key);
                        }
                    }
                    space::AppletMsg::ClientSocketPair(id, client_id, c, s) => {
                        let _ =
                            calloop_tx.send(PanelCalloopMsg::ClientSocketPair(id, client_id, c, s));
                        // XXX This is done to avoid a possible race,
                        // the client & socket need to be update in the panel_space state before the process starts again
                        let _ = unpause_launchpad_rx.recv();
                    }
                    space::AppletMsg::Cleanup(id) => {
                        for id in process_ids.remove(&id).unwrap_or_default() {
                            let _ = process_manager.stop_process(id).await;
                        }
                    }
                    space::AppletMsg::NeedNewNotificationFd(sender) => {
                        let Some(proxy) = notifications_proxy.as_mut() else {
                            warn!("Can't start notifications applet without a connection");
                            continue;
                        };
                        let fd = match proxy.get_fd().await {
                            Ok(fd) => fd,
                            Err(err) => {
                                error!("Failed to get fd for the notifications applet {}", err);
                                continue;
                            }
                        };
                        let fd = fd.into_raw_fd();

                        _ = sender.send(fd);
                    }
                };
            }
        });

        Ok(())
    });

    let mut server_display = smithay::reexports::wayland_server::Display::new().unwrap();
    let s_dh = server_display.handle();

    let mut server_state = ServerState::new(s_dh.clone());

    let mut client_state =
        ClientState::new(event_loop.handle(), &mut space, &mut server_state)?;
    client_state.init_toplevel_info_state();
    run(space, client_state, server_state, event_loop, server_display)?;
    Ok(())
}
