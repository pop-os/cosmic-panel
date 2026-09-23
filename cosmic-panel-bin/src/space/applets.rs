// SPDX-License-Identifier: MPL-2.0

use std::ffi::OsString;
use std::fs;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::PathBuf;

use cctk::wayland_client::Proxy;
use cosmic_panel_config::{CosmicPanelConfig, NAME, Side};
use freedesktop_desktop_entry::{DesktopEntry, Iter};
use launch_pad::process::Process;
use sctk::reexports::client::QueueHandle;
use shlex::Shlex;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::reexports::wayland_server::backend::ClientId;
use tokio::sync::{mpsc, oneshot};
use tracing::{error, error_span, info, info_span, trace, warn};

use super::panel_space::{
    AppletAutoClickAnchor, AppletMsg, ClientShrinkSize, Clients, PanelClient, PanelSpace,
};
use crate::space_container::SpaceContainer;
use crate::xdg_shell_wrapper::client::handlers::wp_security_context::SecurityContext;
use crate::xdg_shell_wrapper::shared_state::GlobalState;
use crate::xdg_shell_wrapper::util::get_client_sock;
use crate::xdg_shell_wrapper::wp_security_context::SecurityContextManager;

fn section(index: usize) -> Side {
    match index {
        0 => Side::WingStart,
        1 => Side::Center,
        _ => Side::WingEnd,
    }
}

#[derive(Debug, Clone)]
struct AppletDesktopEntry {
    path: PathBuf,
    exec: String,
    requests_wayland_display: bool,
    shrink_min_size: Option<ClientShrinkSize>,
    shrink_priority: Option<u32>,
    padding_shrinkable: bool,
    minimize_priority: Option<u32>,
    auto_popup_hover_press: Option<AppletAutoClickAnchor>,
    is_notification_applet: bool,
}

impl AppletDesktopEntry {
    fn find(name: &str, locales: &[String]) -> Option<Self> {
        let stem = OsString::from(name);

        for path in Iter::new(freedesktop_desktop_entry::default_paths()) {
            if path.file_stem() != Some(stem.as_os_str()) {
                continue;
            }

            let Ok(bytes) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(entry) = DesktopEntry::from_str(&path, &bytes, Some(locales)) else {
                continue;
            };
            let Some(exec) = entry.exec() else {
                continue;
            };

            return Some(Self {
                path: path.clone(),
                exec: exec.to_string(),
                requests_wayland_display: entry.desktop_entry("X-HostWaylandDisplay").is_some(),
                shrink_min_size: entry
                    .desktop_entry("X-OverflowMinSize")
                    .and_then(|x| x.parse::<u32>().ok())
                    .map(ClientShrinkSize::AppletUnit),
                shrink_priority: entry
                    .desktop_entry("X-OverflowPriority")
                    .and_then(|x| x.parse::<u32>().ok()),
                padding_shrinkable: entry
                    .desktop_entry("X-CosmicShrinkable")
                    .map(|x| x == "true")
                    .unwrap_or_default(),
                minimize_priority: entry
                    .desktop_entry("X-MinimizeApplet")
                    .map(|x| x.parse::<u32>().unwrap_or(0)),
                auto_popup_hover_press: entry
                    .desktop_entry("X-CosmicHoverPopup")
                    .map(|v| v.parse::<AppletAutoClickAnchor>().unwrap_or_default()),
                is_notification_applet: entry.desktop_entry("X-NotificationsApplet").is_some(),
            });
        }

        None
    }

    fn apply(&self, client: &mut PanelClient) {
        client.path = Some(self.path.clone());
        client.exec = Some(self.exec.clone());
        client.requests_wayland_display = Some(self.requests_wayland_display);
        client.shrink_min_size = self.shrink_min_size;
        client.shrink_priority = self.shrink_priority;
        client.padding_shrinkable = self.padding_shrinkable;
        client.minimize_priority = self.minimize_priority;
        client.auto_popup_hover_press = self.auto_popup_hover_press;
        client.is_notification_applet = Some(self.is_notification_applet);
    }
}

impl PanelSpace {
    fn applet_sections(&self) -> [(Side, &Clients); 3] {
        [
            (Side::WingStart, &self.clients_left),
            (Side::Center, &self.clients_center),
            (Side::WingEnd, &self.clients_right),
        ]
    }

    fn section_clients(&self, index: usize) -> Clients {
        match index {
            0 => self.clients_left.clone(),
            1 => self.clients_center.clone(),
            _ => self.clients_right.clone(),
        }
    }

    fn configured_applets(config: &CosmicPanelConfig, side: Side) -> Vec<String> {
        match side {
            Side::WingStart => config.plugins_left().unwrap_or_default(),
            Side::Center => config.plugins_center().unwrap_or_default(),
            Side::WingEnd => config.plugins_right().unwrap_or_default(),
        }
    }

    pub(crate) fn update_applet_clients(
        &mut self,
        config: &CosmicPanelConfig,
        display: &mut DisplayHandle,
        qh: &QueueHandle<GlobalState>,
        security_context_manager: Option<&SecurityContextManager>,
    ) -> Vec<ClientId> {
        let locales = freedesktop_desktop_entry::get_languages_from_env();
        let mut removed = Vec::new();

        let mut existing: Vec<(usize, PanelClient)> = Vec::new();
        let mut spacers: Vec<(usize, PanelClient)> = Vec::new();
        for (side, clients) in self.applet_sections() {
            let index = section_index(side);
            for client in clients.lock().unwrap().drain(..) {
                if client.client.is_some() {
                    existing.push((index, client));
                } else {
                    spacers.push((index, client));
                }
            }
        }

        let running: std::collections::HashSet<String> =
            existing.iter().map(|(_, c)| c.name.clone()).collect();
        let mut placed: std::collections::HashSet<String> = std::collections::HashSet::new();

        let mut lists: [Vec<PanelClient>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        let mut moved: Vec<(usize, usize, String)> = Vec::new();
        let mut added: Vec<(usize, String)> = Vec::new();

        for index in 0..3 {
            let side = section(index);
            for name in Self::configured_applets(config, side) {
                if !placed.insert(name.clone()) {
                    // The applet is configured more than once.
                    continue;
                }

                if let Some(position) = existing.iter().position(|(_, c)| c.name == name) {
                    let (old_index, client) = existing.remove(position);
                    if old_index != index {
                        moved.push((old_index, index, name.clone()));
                    }
                    lists[index].push(client);
                    continue;
                }

                let (client, stream) = get_client_sock(display);
                let mut panel_client = PanelClient::new(name.clone(), None, client, Some(stream));
                match AppletDesktopEntry::find(&name, &locales) {
                    Some(entry) => {
                        entry.apply(&mut panel_client);
                        added.push((index, name.clone()));
                    },
                    None => {
                        warn!("Failed to find the desktop entry of applet {}", name);
                    },
                }
                lists[index].push(panel_client);
            }
        }

        for (_, client) in existing {
            info!("Removing applet {} of panel {}", client.name, self.id());
            let _ = self
                .shared
                .applet_tx
                .try_send(AppletMsg::StopApplet(self.id(), client.name.clone()));
            if let Some(id) = client.client.as_ref().map(|c| c.id()) {
                removed.push(id);
            }
        }

        for (index, spacer) in spacers {
            if spacer.name == "spacer-start" {
                lists[index].insert(0, spacer);
            } else {
                lists[index].push(spacer);
            }
        }

        let mut minimize_applet: Option<(u32, String)> = None;
        for client in lists.iter().flatten() {
            if let Some(priority) = client.minimize_priority
                && minimize_applet.as_ref().is_none_or(|(p, _)| priority > *p)
            {
                minimize_applet = Some((priority, client.name.clone()));
            }
        }

        let mut restart: Vec<(usize, String)> = Vec::new();
        for (index, list) in lists.iter_mut().enumerate() {
            for client in list.iter_mut() {
                let is_minimize_applet =
                    minimize_applet.as_ref().is_some_and(|(_, name)| name == &client.name);
                let minimize_applet_changed = client.is_minimize_applet != is_minimize_applet;
                client.is_minimize_applet = is_minimize_applet;

                if client.exec.is_none() || !running.contains(&client.name) {
                    continue;
                }

                let bind_settings_protocol =
                    client.applet_settings.as_ref().is_some_and(|settings| settings.is_alive());
                let applet_size_changed = moved.iter().any(|(old, new, name)| {
                    *new == index
                        && name == &client.name
                        && !bind_settings_protocol
                        && config.get_effective_applet_size(section(*new))
                            != self.config.get_effective_applet_size(section(*old))
                });

                if minimize_applet_changed || applet_size_changed {
                    restart.push((index, client.name.clone()));
                }
            }
        }

        *self.clients_left.lock().unwrap() = std::mem::take(&mut lists[0]);
        *self.clients_center.lock().unwrap() = std::mem::take(&mut lists[1]);
        *self.clients_right.lock().unwrap() = std::mem::take(&mut lists[2]);

        if !moved.is_empty() || !added.is_empty() || !removed.is_empty() {
            self.is_dirty = true;
            self.needs_layout = true;
        }

        for (index, name) in restart {
            self.restart_applet(config, index, &name, display, qh, security_context_manager);
        }

        for (index, name) in added {
            self.start_applet(config, index, &name, display, qh, security_context_manager);
        }

        removed
    }

    fn start_applet(
        &mut self,
        config: &CosmicPanelConfig,
        index: usize,
        name: &str,
        display: &mut DisplayHandle,
        qh: &QueueHandle<GlobalState>,
        security_context_manager: Option<&SecurityContextManager>,
    ) {
        let panel_id = self.id();
        let active_output = self.output_name();
        let applet_tx = self.shared.applet_tx.clone();
        let clients = self.section_clients(index);

        let mut applets = clients.lock().unwrap();
        let Some(panel_client) = applets.iter_mut().find(|c| c.name == name) else {
            error!("Failed to find applet {} in the panel", name);
            return;
        };

        if let Err(err) = start_applet_process(
            &panel_id,
            &active_output,
            &applet_tx,
            &clients,
            config,
            section(index),
            panel_client,
            display,
            qh,
            security_context_manager,
        ) {
            error!(?err, "Failed to start applet {}", name);
        }
    }

    fn restart_applet(
        &mut self,
        config: &CosmicPanelConfig,
        index: usize,
        name: &str,
        display: &mut DisplayHandle,
        qh: &QueueHandle<GlobalState>,
        security_context_manager: Option<&SecurityContextManager>,
    ) {
        let panel_id = self.id();
        let active_output = self.output_name();
        let applet_tx = self.shared.applet_tx.clone();

        let clients = self.section_clients(index);
        let mut applets = clients.lock().unwrap();
        let Some(panel_client) = applets.iter_mut().find(|c| c.name == name) else {
            error!("Failed to find applet {} in the panel", name);
            return;
        };

        info!("Restarting applet {} of panel {}", name, panel_id);
        let _ = applet_tx.try_send(AppletMsg::StopApplet(panel_id.clone(), name.to_string()));

        // The restarted applet gets a new connection to the panel.
        let old_client_id = panel_client.client.as_ref().map(|c| c.id());
        let (client, stream) = get_client_sock(display);
        panel_client.client = Some(client);
        panel_client.stream = Some(stream);
        panel_client.security_ctx = None;

        if let Some(old_client_id) = old_client_id {
            let _ = applet_tx.try_send(AppletMsg::ClientSocketPair(old_client_id));
        }

        if let Err(err) = start_applet_process(
            &panel_id,
            &active_output,
            &applet_tx,
            &clients,
            config,
            section(index),
            panel_client,
            display,
            qh,
            security_context_manager,
        ) {
            error!(?err, "Failed to restart applet {}", name);
        }
    }
}

fn section_index(side: Side) -> usize {
    match side {
        Side::WingStart => 0,
        Side::Center => 1,
        Side::WingEnd => 2,
    }
}

#[allow(clippy::too_many_arguments)]
fn start_applet_process(
    panel_id: &str,
    active_output: &str,
    applet_tx: &mpsc::Sender<AppletMsg>,
    clients: &Clients,
    config: &CosmicPanelConfig,
    side: Side,
    panel_client: &mut PanelClient,
    display: &DisplayHandle,
    qh: &QueueHandle<GlobalState>,
    security_context_manager: Option<&SecurityContextManager>,
) -> anyhow::Result<()> {
    let Some(exec_line) = panel_client.exec.clone() else {
        anyhow::bail!("applet {} does not have an executable", panel_client.name);
    };
    let Some(socket) = panel_client.stream.take() else {
        anyhow::bail!("applet {} does not have a connection to the panel", panel_client.name);
    };

    let env_vars = vec![
        ("COSMIC_PANEL_NAME".to_string(), config.name.clone()),
        ("COSMIC_PANEL_OUTPUT".to_string(), active_output.to_string()),
        (
            "COSMIC_PANEL_SPACING".to_string(),
            ron::ser::to_string(&config.spacing).unwrap_or_default(),
        ),
        (
            "COSMIC_PANEL_ANCHOR".to_string(),
            ron::ser::to_string(&config.anchor).unwrap_or_default(),
        ),
        (
            "COSMIC_PANEL_BACKGROUND".to_string(),
            ron::ser::to_string(&config.background).unwrap_or_default(),
        ),
        (
            "COSMIC_PANEL_PADDING_OVERLAP".to_string(),
            ron::ser::to_string(&config.padding_overlap()).unwrap_or_default(),
        ),
    ];

    let is_notification_applet = panel_client.is_notification_applet.unwrap_or(false);
    let requests_wayland_display = panel_client.requests_wayland_display.unwrap_or(false);

    let mut exec_iter = Shlex::new(&exec_line);
    let exec = exec_iter.next().expect("exec parameter must contain at least on word");

    let mut args = Vec::new();
    for arg in exec_iter {
        trace!("child argument: {}", &arg);
        args.push(arg);
    }

    let mut fds: Vec<OwnedFd> = Vec::with_capacity(2);
    let mut applet_env = Vec::new();
    applet_env.push(("X_MINIMIZE_APPLET".to_string(), panel_client.is_minimize_applet.to_string()));
    let config_size =
        ron::ser::to_string(&config.get_effective_applet_size(side)).unwrap_or_default();
    applet_env.push(("COSMIC_PANEL_SIZE".to_string(), config_size));

    if requests_wayland_display && let Some(security_context_manager) = security_context_manager {
        match security_context_manager.create_listener::<SpaceContainer>(qh) {
            Ok(security_context) => {
                security_context.set_sandbox_engine(NAME.to_string());
                security_context.set_app_id(panel_client.name.clone());
                security_context
                    .set_instance_id(format!("{}.{}", panel_client.name, active_output));
                security_context.commit();

                let data = security_context.data::<SecurityContext>().unwrap();
                let privileged_socket = data.conn.lock().unwrap().take().unwrap();
                applet_env.push((
                    "X_PRIVILEGED_WAYLAND_SOCKET".to_string(),
                    privileged_socket.0.as_raw_fd().to_string(),
                ));

                fds.push(privileged_socket.0.into());
                panel_client.security_ctx = Some(security_context);
            },
            Err(why) => {
                error!(?why, "Failed to create a listener");
            },
        }
    }

    for (key, val) in &env_vars {
        if !requests_wayland_display && *key == "WAYLAND_DISPLAY" {
            continue;
        }
        applet_env.push((key.clone(), val.clone()));
    }
    applet_env.push(("WAYLAND_SOCKET".to_string(), socket.as_raw_fd().to_string()));

    fds.push(socket.into());
    let display_handle = display.clone();
    let applet_tx_callback = applet_tx.clone();
    let clients = clients.clone();
    let panel_id = panel_id.to_string();
    let name = panel_client.name.clone();
    let name_info = panel_client.name.clone();
    let name_err = panel_client.name.clone();
    let Some(client) = panel_client.client.as_ref() else {
        panic!("Failed to get client");
    };
    let client_id = client.id();
    let client_id_info = client.id();
    let client_id_err = client.id();
    let security_context_manager_clone = security_context_manager.cloned();
    let qh_clone = qh.clone();

    // arg forwarding WAYLAND_SOCKET is required
    // env must be passed in args
    let is_flatpak = panel_client.is_flatpak();

    if is_flatpak {
        args.insert(args.len().saturating_sub(2), "--socket=inherit-wayland-socket".to_string());
        args.insert(args.len().saturating_sub(2), "--die-with-parent".to_string());
        for (k, v) in &applet_env {
            args.insert(args.len().saturating_sub(2), format!("--env={k}={v}"))
        }
    }
    trace!("child: {}, {:?} {:?}", &exec, args, applet_env);

    info!("Starting: {}", exec);
    let active_output = active_output.to_string();

    let mut process = Process::new()
        .with_executable(&exec)
        .with_args(args.clone())
        .with_on_stderr(move |_, _, out| {
            // TODO why is span not included in logs to journald
            let name = name_err.clone();
            let client_id = client_id_err.clone();

            async move {
                error_span!("stderr", client = ?client_id).in_scope(|| {
                    error!("{}: {}", name, out);
                });
            }
        })
        .with_on_stdout(move |_, _, out| {
            let name = name_info.clone();
            let client_id = client_id_info.clone();
            // TODO why is span not included in logs to journald
            async move {
                info_span!("stdout", client = ?client_id).in_scope(|| {
                    info!("{}: {}", name, out);
                });
            }
        })
        .with_on_exit(move |mut pman, key, err_code, is_restarting| {
            let client_id_clone = client_id.clone();
            let name = name.clone();

            if let Some(err_code) = err_code {
                error_span!("stderr", client = ?client_id).in_scope(|| {
                    error!("{}: exited with code {}", name, err_code);
                });
            } else {
                info_span!("stderr", client = ?client_id).in_scope(|| {
                    error!("{}: exited without error", name);
                });
            }

            let display_handle = display_handle.clone();
            let applet_tx = applet_tx_callback.clone();
            let clients = clients.clone();
            let mut applet_env: Vec<(String, String)> = Vec::with_capacity(1);
            let mut fds: Vec<OwnedFd> = Vec::with_capacity(2);
            let should_restart = is_restarting && err_code.is_some();
            let security_context = if requests_wayland_display && should_restart {
                security_context_manager_clone.as_ref().and_then(|security_context_manager| {
                    let active_output = active_output.clone();

                    security_context_manager
                        .create_listener::<SpaceContainer>(&qh_clone)
                        .ok()
                        .inspect(|security_context| {
                            security_context.set_sandbox_engine(NAME.to_string());
                            security_context.set_app_id(name.clone());
                            security_context.set_instance_id(format!("{}.{}", name, active_output));
                            security_context.commit();

                            let data = security_context.data::<SecurityContext>().unwrap();
                            let privileged_socket = data.conn.lock().unwrap().take().unwrap();
                            applet_env.push((
                                "X_PRIVILEGED_WAYLAND_SOCKET".to_string(),
                                privileged_socket.0.as_raw_fd().to_string(),
                            ));
                            fds.push(privileged_socket.0.into());
                        })
                })
            } else {
                None
            };

            let args = args.clone();
            async move {
                if !should_restart {
                    _ = pman.stop_process(key).await;
                    return;
                }

                // The restarted applet gets a new connection to the panel.
                let mut display_handle = display_handle;
                let (c, client_socket) = get_client_sock(&mut display_handle);
                let raw_client_socket = client_socket.as_raw_fd();

                if let Some(old_client) = clients
                    .lock()
                    .unwrap()
                    .iter_mut()
                    .find(|PanelClient { name: client_name, .. }| client_name == &name)
                {
                    old_client.client = Some(c);
                    old_client.security_ctx = security_context;
                    info!("Replaced the client socket");
                } else {
                    error!("Failed to find matching client... {}", &name)
                }
                let _ = applet_tx.send(AppletMsg::ClientSocketPair(client_id_clone)).await;

                if is_notification_applet {
                    let (tx, rx) = oneshot::channel();
                    _ = applet_tx.send(AppletMsg::NeedNewNotificationFd(tx)).await;
                    let Ok(fd) = rx.await else {
                        error!("Failed to get new fd");
                        return;
                    };
                    if let Err(err) = pman
                        .update_process_env(
                            &key,
                            vec![("COSMIC_NOTIFICATIONS".to_string(), fd.as_raw_fd().to_string())],
                        )
                        .await
                    {
                        error!("Failed to update process env: {}", err);
                        return;
                    }
                    fds.push(fd);
                    fds.push(client_socket.into());
                    if let Err(err) = pman.update_process_fds(&key, move || fds).await {
                        error!("Failed to update process fds: {}", err);
                        return;
                    }
                } else {
                    fds.push(client_socket.into());
                    if let Err(err) = pman.update_process_fds(&key, move || fds).await {
                        error!("Failed to update process fds: {}", err);
                        return;
                    }
                }

                applet_env.retain(|(k, _)| k.as_str() != "WAYLAND_SOCKET");
                applet_env.push(("WAYLAND_SOCKET".to_string(), raw_client_socket.to_string()));

                let mut args = args.clone();
                if is_flatpak {
                    args.retain(|arg| !arg.contains("WAYLAND_SOCKET"));
                    args.insert(
                        args.len().saturating_sub(2),
                        format!("--env=WAYLAND_SOCKET={}", raw_client_socket),
                    );
                }
                let _ = pman.update_process_env(&key, applet_env.clone()).await;
                let _ = pman.update_process_args(&key, args).await;
            }
        });

    let msg = if is_notification_applet {
        AppletMsg::NewNotificationsProcess {
            panel: panel_id,
            applet: panel_client.name.clone(),
            process,
            env: applet_env,
            fds,
        }
    } else {
        process = process.with_fds(move || fds);

        AppletMsg::NewProcess {
            panel: panel_id,
            applet: panel_client.name.clone(),
            process: process.with_env(applet_env),
        }
    };
    match applet_tx.try_send(msg) {
        Ok(_) => {},
        Err(e) => error!("{e}"),
    };

    Ok(())
}
