mod config_watching;
mod iced;
mod minimize;
mod notifications;
mod space;
mod space_container;

use anyhow::Result;
use cctk::{
    cosmic_protocols::toplevel_info::v1::client::zcosmic_toplevel_handle_v1,
    wayland_client::protocol::wl_output::WlOutput,
};
use config_watching::{watch_config, watch_cosmic_theme};
use cosmic_panel_config::CosmicPanelConfig;
use launch_pad::{ProcessKey, ProcessManager};
use minimize::MinimizeApplet;
use notifications::notifications_conn;
use sctk::reexports::calloop::channel::SyncSender;
use smithay::reexports::{calloop, wayland_server::backend::ClientId};
use std::{
    collections::HashMap,
    mem,
    os::fd::{AsRawFd, OwnedFd},
    time::Duration,
};
use tokio::{runtime, sync::mpsc};
use tracing::{error, info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use xdg_shell_wrapper::{
    client_state::ClientState, run, server_state::ServerState, shared_state::GlobalState,
};

#[derive(Debug)]
pub enum PanelCalloopMsg {
    ClientSocketPair(ClientId),
    RestartSpace(CosmicPanelConfig, WlOutput),
    MinimizeRect { output: String, applet_info: MinimizeApplet },
    UpdateToplevel(zcosmic_toplevel_handle_v1::ZcosmicToplevelHandleV1),
}

fn main() -> Result<()> {
    let fmt_layer = fmt::layer().with_target(false);
    let filter_layer =
        EnvFilter::try_from_default_env().or_else(|_| EnvFilter::try_new("info")).unwrap();
    if let Ok(journal_layer) = tracing_journald::layer() {
        tracing_subscriber::registry().with(fmt_layer).with(filter_layer).init();
    } else {
        tracing_subscriber::registry().with(fmt_layer).with(filter_layer).init();
    }

    log_panics::init();

    let arg = std::env::args().nth(1);
    let usage = "USAGE: cosmic-panel";
    let config = match arg.as_ref().map(|s| &s[..]) {
        Some(arg) if arg == "--help" || arg == "-h" => {
            println!("{}", usage);
            std::process::exit(1);
        },
        None => match cosmic_panel_config::CosmicPanelContainerConfig::load() {
            Ok(c) => c,
            Err((errors, c)) => {
                for e in errors {
                    error!("Panel Entry Error: {:?}", e);
                }
                let _ = c.write_entries();
                c
            },
        },
        _ => {
            println!("{}", usage);
            std::process::exit(1);
        },
    };

    let (applet_tx, mut applet_rx) = mpsc::channel(200);
    let (calloop_tx, calloop_rx): (SyncSender<PanelCalloopMsg>, _) =
        calloop::channel::sync_channel(100);

    let event_loop = calloop::EventLoop::try_new()?;

    let mut space = space_container::SpaceContainer::new(
        config,
        applet_tx.clone(),
        calloop_tx.clone(),
        event_loop.handle(),
    );

    let handle = event_loop.handle();
    match watch_config(&space.config, handle) {
        Ok(watchers) => {
            info!("Watching panel config successful");
            space.watchers = watchers;
        },
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
                        PanelCalloopMsg::ClientSocketPair(client_id) => {
                            state.space.cleanup_client(client_id);
                        },
                        PanelCalloopMsg::RestartSpace(config, o) => {
                            state.space.update_space(
                                config,
                                &state.client_state.compositor_state,
                                state.client_state.fractional_scaling_manager.as_ref(),
                                state.client_state.viewporter_state.as_ref(),
                                &mut state.client_state.layer_state,
                                &state.client_state.queue_handle,
                                Some(o),
                            );
                        },
                        PanelCalloopMsg::UpdateToplevel(toplevel) => {
                            minimize::update_toplevel(state, toplevel)
                        },
                        PanelCalloopMsg::MinimizeRect { output, applet_info } => {
                            minimize::set_rectangles(state, output, applet_info)
                        },
                    },
                    calloop::channel::Event::Closed => {},
                };
            },
        )
        .expect("failed to insert dbus event source");

    std::thread::spawn(move || -> anyhow::Result<()> {
        let rt = runtime::Builder::new_current_thread().enable_all().build()?;
        let mut process_ids: HashMap<String, Vec<ProcessKey>> = HashMap::new();

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
                    },
                };

            while let Some(msg) = applet_rx.recv().await {
                match msg {
                    space::AppletMsg::NewProcess(id, process) => {
                        if let Ok(key) = process_manager.start(process).await {
                            let entry = process_ids.entry(id).or_insert_with(|| Vec::new());
                            entry.push(key);
                        }
                    },
                    space::AppletMsg::NewNotificationsProcess(
                        id,
                        mut process,
                        mut env,
                        mut fds,
                    ) => {
                        let Some(proxy) = notifications_proxy.as_mut() else {
                            notifications_proxy = match tokio::time::timeout(
                                Duration::from_secs(1),
                                notifications_conn(),
                            )
                            .await
                            {
                                Ok(Ok(p)) => Some(p),
                                err => {
                                    error!(
                                        "Failed to connect to the notifications daemon {:?}",
                                        err
                                    );
                                    None
                                },
                            };
                            warn!("Can't start notifications applet without a connection");
                            continue;
                        };
                        info!("Getting fd for notifications applet");
                        let notif_fd = match tokio::time::timeout(
                            Duration::from_secs(1),
                            proxy.get_fd(),
                        )
                        .await
                        {
                            Ok(Ok(fd)) => fd,
                            Ok(Err(err)) => {
                                error!("Failed to get fd for the notifications applet {}", err);
                                continue;
                            },
                            Err(err) => {
                                error!("Failed to get fd for the notifications applet {}", err);
                                continue;
                            },
                        };
                        let notif_fd = OwnedFd::from(notif_fd);
                        env.push((
                            "COSMIC_NOTIFICATIONS".to_string(),
                            notif_fd.as_raw_fd().to_string(),
                        ));
                        fds.push(notif_fd);
                        process = process.with_fds(move || fds);
                        process = process.with_env(env);
                        info!("Starting notifications applet");
                        if let Ok(key) = process_manager.start(process).await {
                            let entry = process_ids.entry(id).or_insert_with(|| Vec::new());
                            entry.push(key);
                        }
                    },
                    space::AppletMsg::ClientSocketPair(client_id) => {
                        let _ = calloop_tx.send(PanelCalloopMsg::ClientSocketPair(client_id));
                    },
                    space::AppletMsg::Cleanup(id) => {
                        for id in process_ids.remove(&id).unwrap_or_default() {
                            let _ = process_manager.stop_process(id).await;
                        }
                    },
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
                            },
                        };
                        let fd = OwnedFd::from(fd);

                        _ = sender.send(fd);
                    },
                };
            }
        });

        Ok(())
    });

    let server_display = smithay::reexports::wayland_server::Display::new().unwrap();
    let s_dh = server_display.handle();

    let mut server_state = ServerState::new(s_dh.clone());

    let mut client_state = ClientState::new(event_loop.handle(), &mut space, &mut server_state)?;
    client_state.init_workspace_state();
    client_state.init_toplevel_info_state();
    client_state.init_toplevel_manager_state();
    run(space, client_state, server_state, event_loop, server_display)?;
    Ok(())
}
