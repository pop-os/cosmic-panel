use std::{
    cell::{Cell, RefCell},
    ffi::OsString,
    fs,
    os::{fd::OwnedFd, unix::prelude::AsRawFd},
    rc::Rc,
    sync::{Arc, Mutex},
    time::Instant,
};

use crate::{
    iced::elements::{target::SpaceTarget, PopupMappedInternal},
    space_container::SpaceContainer,
    xdg_shell_wrapper::{
        client_state::ClientFocus,
        server_state::ServerPointerFocus,
        shared_state::GlobalState,
        space::{
            PanelPopup, SpaceEvent, Visibility, WrapperPopup, WrapperPopupState, WrapperSpace,
        },
        util::get_client_sock,
        wp_fractional_scaling::FractionalScalingManager,
        wp_security_context::{SecurityContext, SecurityContextManager},
        wp_viewporter::ViewporterState,
    },
};
use anyhow::bail;
use cosmic::iced::id;
use cosmic_panel_config::{CosmicPanelConfig, CosmicPanelOuput, NAME};
use freedesktop_desktop_entry::{self, DesktopEntry, Iter};
use itertools::izip;
use launch_pad::process::Process;
use sctk::{
    compositor::{CompositorState, Region},
    output::OutputInfo,
    reexports::client::{
        protocol::{wl_output as c_wl_output, wl_surface as c_wl_surface},
        Connection, Proxy, QueueHandle,
    },
    seat::pointer::{PointerEvent, BTN_LEFT},
    shell::{
        wlr_layer::{
            KeyboardInteractivity, Layer, LayerShell, LayerSurface, LayerSurfaceConfigure,
        },
        xdg::popup,
        WaylandSurface,
    },
};
use shlex::Shlex;
use smithay::{
    backend::renderer::{damage::OutputDamageTracker, element, gles::GlesRenderer},
    desktop::{
        space::SpaceElement, utils::bbox_from_surface_tree, PopupKind, PopupManager, Window,
    },
    output::Output,
    reexports::wayland_server::{
        self, protocol::wl_surface::WlSurface as s_WlSurface, DisplayHandle, Resource,
    },
    utils::{Logical, Rectangle, Size},
    wayland::{
        compositor::{with_states, SurfaceAttributes},
        fractional_scale::with_fractional_scale,
        seat::WaylandFocus,
        shell::xdg::{PopupSurface, PositionerState, SurfaceCachedState},
    },
};
use tokio::sync::oneshot;
use tracing::{error, error_span, info, info_span, trace};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1;

use crate::{
    iced::elements::CosmicMappedInternal,
    space::{
        panel_space::{AppletAutoClickAnchor, PanelClient},
        AppletMsg,
    },
};

use super::{layout::OverflowSection, PanelSpace};

impl WrapperSpace for PanelSpace {
    type Config = CosmicPanelConfig;

    /// set the display handle of the space
    fn set_display_handle(&mut self, s_display: wayland_server::DisplayHandle) {
        self.s_display.replace(s_display);
    }

    /// get the client hovered surface of the space
    fn get_client_hovered_surface(&self) -> Rc<RefCell<ClientFocus>> {
        self.c_hovered_surface.clone()
    }

    /// get the client focused surface of the space
    fn get_client_focused_surface(&self) -> Rc<RefCell<ClientFocus>> {
        self.c_focused_surface.clone()
    }

    fn add_window(&mut self, w: Window) {
        self.is_dirty = true;
        if let Some(t) = w.toplevel() {
            let suggested_size = self.config.size.get_applet_icon_size(true) as i32
                + self.config.size.get_applet_padding(true) as i32 * 2;
            let is_horizontal = self.config.is_horizontal();
            t.with_pending_state(|state| {
                state.size = Some(if is_horizontal {
                    (0, suggested_size).into()
                } else {
                    (suggested_size, 0).into()
                });
            });
            t.send_pending_configure();
        }
        if let Some(s) = w.wl_surface() {
            with_states(&s, |states| {
                with_fractional_scale(states, |fractional_scale| {
                    fractional_scale.set_preferred_scale(self.scale);
                });
            });
        }
        self.space.map_element(CosmicMappedInternal::Window(w.clone()), (0, 0), false);
    }

    fn add_popup(
        &mut self,
        compositor_state: &sctk::compositor::CompositorState,
        fractional_scale_manager: Option<&FractionalScalingManager>,
        viewport: Option<&ViewporterState>,
        _conn: &sctk::reexports::client::Connection,
        qh: &QueueHandle<GlobalState>,
        xdg_shell_state: &mut sctk::shell::xdg::XdgShell,
        s_surface: PopupSurface,
        positioner: sctk::shell::xdg::XdgPositioner,
        positioner_state: PositionerState,
    ) -> anyhow::Result<()> {
        self.apply_positioner_state(&positioner, positioner_state, &s_surface);
        let c_wl_surface = compositor_state.create_surface(qh);

        let parent = self.popups.iter().find_map(|p| {
            if s_surface.get_parent_surface().is_some_and(|s| &s == p.s_surface.wl_surface()) {
                Some(p.popup.c_popup.xdg_surface())
            } else {
                None
            }
        });
        self.overflow_popup = None;

        let c_popup = popup::Popup::from_surface(
            parent,
            &positioner,
            qh,
            c_wl_surface.clone(),
            xdg_shell_state,
        )?;

        let input_region = Region::new(compositor_state)?;

        if let (Some(s_window_geometry), Some(input_regions)) =
            with_states(s_surface.wl_surface(), |states| {
                (
                    states.cached_state.current::<SurfaceCachedState>().geometry,
                    states
                        .cached_state
                        .current::<SurfaceAttributes>()
                        .input_region
                        .as_ref()
                        .cloned(),
                )
            })
        {
            c_popup.xdg_surface().set_window_geometry(
                s_window_geometry.loc.x,
                s_window_geometry.loc.y,
                s_window_geometry.size.w.max(1),
                s_window_geometry.size.h.max(1),
            );
            for r in input_regions.rects {
                input_region.add(0, 0, r.1.size.w, r.1.size.h);
            }
            c_wl_surface.set_input_region(Some(input_region.wl_region()));
        }

        if parent.is_none() {
            self.layer.as_ref().unwrap().get_popup(c_popup.xdg_popup());
        }
        let fractional_scale =
            fractional_scale_manager.map(|f| f.fractional_scaling(&c_wl_surface, &qh));

        let viewport = viewport.map(|v| {
            with_states(&s_surface.wl_surface(), |states| {
                with_fractional_scale(states, |fractional_scale| {
                    fractional_scale.set_preferred_scale(self.scale);
                });
            });
            let viewport = v.get_viewport(&c_wl_surface, &qh);
            viewport.set_destination(
                positioner_state.rect_size.w.max(1),
                positioner_state.rect_size.h.max(1),
            );
            viewport
        });
        if fractional_scale.is_none() {
            c_wl_surface.set_buffer_scale(self.scale as i32);
        }

        // must be done after role is assigned as popup
        c_wl_surface.commit();

        let cur_popup_state = Some(WrapperPopupState::WaitConfigure);

        self.popups.push(WrapperPopup {
            popup: PanelPopup {
                damage_tracked_renderer: OutputDamageTracker::new(
                    positioner_state.rect_size.to_f64().to_physical(self.scale).to_i32_round(),
                    1.0,
                    smithay::utils::Transform::Flipped180,
                ),
                c_popup,
                egl_surface: None,
                dirty: false,
                rectangle: Rectangle::from_loc_and_size((0, 0), positioner_state.rect_size),
                state: cur_popup_state,
                input_region: Some(input_region),
                wrapper_rectangle: Rectangle::from_loc_and_size((0, 0), positioner_state.rect_size),
                positioner,
                has_frame: true,
                fractional_scale,
                viewport,
                scale: self.scale,
            },
            s_surface,
        });

        Ok(())
    }

    fn reposition_popup(
        &mut self,
        popup: PopupSurface,
        pos_state: PositionerState,
        token: u32,
    ) -> anyhow::Result<()> {
        popup.with_pending_state(|pending| {
            pending.geometry = Rectangle::from_loc_and_size((0, 0), pos_state.rect_size);
        });
        if let Some(p) = self.popups.iter().find(|wp| &wp.s_surface == &popup) {
            let positioner = &p.popup.positioner;
            p.popup.c_popup.xdg_surface().set_window_geometry(
                0,
                0,
                pos_state.rect_size.w.max(1),
                pos_state.rect_size.h.max(1),
            );
            self.apply_positioner_state(&positioner, pos_state, &p.s_surface);

            if positioner.version() >= 3 {
                p.popup.c_popup.reposition(&positioner, token);
            }
            p.popup.c_popup.wl_surface().commit();
            if positioner.version() >= 3 {
                popup.send_repositioned(token);
            }
        }

        popup.send_configure()?;
        Ok(())
    }

    fn config(&self) -> Self::Config {
        self.config.clone()
    }

    fn spawn_clients(
        &mut self,
        mut display: DisplayHandle,
        qh: &QueueHandle<GlobalState>,
        security_context_manager: Option<SecurityContextManager>,
    ) -> anyhow::Result<()> {
        info!("Spawning applets");
        let mut left_guard = self.clients_left.lock().unwrap();
        let mut center_guard = self.clients_center.lock().unwrap();
        let mut right_guard = self.clients_right.lock().unwrap();

        if left_guard.is_empty() && center_guard.is_empty() && right_guard.is_empty() {
            *left_guard = self
                .config
                .plugins_left()
                .as_ref()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|name| {
                    let (c, s) = get_client_sock(&mut display);
                    PanelClient::new(name, c, Some(s))
                })
                .collect();

            *center_guard = self
                .config
                .plugins_center()
                .as_ref()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|name| {
                    let (c, s) = get_client_sock(&mut display);
                    PanelClient::new(name, c, Some(s))
                })
                .collect();

            *right_guard = self
                .config
                .plugins_right()
                .as_ref()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|name| {
                    let (c, s) = get_client_sock(&mut display);
                    PanelClient::new(name, c, Some(s))
                })
                .collect();

            let mut desktop_ids: Vec<_> = left_guard
                .iter_mut()
                .map(|c| (c, self.clients_left.clone()))
                .chain(center_guard.iter_mut().map(|c| (c, self.clients_center.clone())))
                .chain(right_guard.iter_mut().map(|c| (c, self.clients_right.clone())))
                .collect();

            let config_size = ron::ser::to_string(&self.config.size).unwrap_or_default();
            let active_output =
                self.output.as_ref().and_then(|o| o.2.name.clone()).unwrap_or_default();

            let config_anchor = ron::ser::to_string(&self.config.anchor).unwrap_or_default();
            let config_bg = ron::ser::to_string(&self.config.background).unwrap_or_default();
            let config_name = self.config.name.clone();
            let env_vars = vec![
                ("COSMIC_PANEL_NAME".to_string(), config_name),
                ("COSMIC_PANEL_SIZE".to_string(), config_size),
                ("COSMIC_PANEL_OUTPUT".to_string(), active_output),
                ("COSMIC_PANEL_ANCHOR".to_string(), config_anchor),
                ("COSMIC_PANEL_BACKGROUND".to_string(), config_bg),
                ("RUST_BACKTRACE".to_string(), "1".to_string()),
            ];
            info!("{:?}", &desktop_ids);

            let mut max_minimize_priority: u32 = 0;

            let mut panel_clients: Vec<(&mut PanelClient, Arc<Mutex<Vec<PanelClient>>>)> =
                Vec::new();

            for path in Iter::new(freedesktop_desktop_entry::default_paths()) {
                // This way each applet is at most started once,
                // even if multiple desktop files in different directories match
                if let Some(position) =
                    desktop_ids.iter().position(|(PanelClient { ref name, .. }, ..)| {
                        Some(OsString::from(name).as_os_str()) == path.file_stem()
                    })
                {
                    let (panel_client, my_list) = desktop_ids.remove(position);
                    info!(panel_client.name);

                    if let Ok(bytes) = fs::read_to_string(&path) {
                        if let Ok(entry) = DesktopEntry::decode(&path, &bytes) {
                            if let Some(exec) = entry.exec() {
                                panel_client.exec = Some(exec.to_string());
                                panel_client.requests_wayland_display =
                                    Some(entry.desktop_entry("X-HostWaylandDisplay").is_some());
                                panel_client.shrink_min_size = entry
                                    .desktop_entry("X-OverflowMinSize")
                                    .and_then(|x| x.parse::<u32>().ok());
                                panel_client.shrink_priority = entry
                                    .desktop_entry("X-OverflowPriority")
                                    .and_then(|x| x.parse::<u32>().ok());

                                panel_client.minimize_priority = if let Some(x_minimize_entry) =
                                    entry.desktop_entry("X-MinimizeApplet")
                                {
                                    match x_minimize_entry.parse::<u32>() {
                                        Ok(p) => {
                                            max_minimize_priority = max_minimize_priority.max(p);
                                            Some(p)
                                        },
                                        Err(_) => Some(0),
                                    }
                                } else {
                                    None
                                };

                                panel_client.auto_popup_hover_press =
                                    entry.desktop_entry("X-CosmicHoverPopup").map(|v| {
                                        v.parse::<AppletAutoClickAnchor>().unwrap_or_default()
                                    });

                                panel_client.is_notification_applet =
                                    Some(entry.desktop_entry("X-NotificationsApplet").is_some());

                                panel_clients.push((panel_client, my_list));
                            }
                        }
                    }
                }
            }

            // only allow 1 per panel
            let mut has_minimize = false;

            for (panel_client, my_list) in panel_clients {
                if !panel_client.exec.is_some() {
                    continue;
                }
                let Some(socket) = panel_client.stream.take() else {
                    error!("Failed to get socket for {}", &panel_client.name);
                    continue;
                };

                // Ensure there is only one applet per panel with minimize
                panel_client.minimize_priority = if panel_client
                    .minimize_priority
                    .is_some_and(|x| x == max_minimize_priority && !has_minimize)
                {
                    has_minimize = true;
                    Some(max_minimize_priority)
                } else {
                    None
                };

                let is_notification_applet = panel_client.is_notification_applet.unwrap_or(false);
                let requests_wayland_display =
                    panel_client.requests_wayland_display.unwrap_or(false);

                let mut exec_iter = Shlex::new(&panel_client.exec.as_deref().unwrap());
                let exec = exec_iter.next().expect("exec parameter must contain at least on word");

                let mut args = Vec::new();
                for arg in exec_iter {
                    trace!("child argument: {}", &arg);
                    args.push(arg);
                }
                let mut fds = Vec::with_capacity(2);
                let mut applet_env = Vec::new();

                applet_env.push((
                    "X_MINIMIZE_APPLET".to_string(),
                    panel_client.minimize_priority.is_some().to_string(),
                ));
                if requests_wayland_display {
                    if let Some(security_context_manager) = security_context_manager.as_ref() {
                        match security_context_manager.create_listener::<SpaceContainer>(qh) {
                            Ok(security_context) => {
                                security_context.set_sandbox_engine(NAME.to_string());
                                security_context.commit();

                                let data = security_context.data::<SecurityContext>().unwrap();
                                let privileged_socket = data.conn.lock().unwrap().take().unwrap();
                                applet_env.push((
                                    "X_PRIVILEGED_WAYLAND_SOCKET".to_string(),
                                    privileged_socket.as_raw_fd().to_string(),
                                ));

                                fds.push(privileged_socket.into());
                                panel_client.security_ctx = Some(security_context);
                            },
                            Err(why) => {
                                error!(?why, "Failed to create a listener");
                            },
                        }
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
                trace!("child: {}, {:?} {:?}", &exec, args, applet_env);

                info!("Starting: {}", exec);

                let display_handle = display.clone();
                let applet_tx_clone = self.applet_tx.clone();
                let id_clone = panel_client.name.clone();
                let id_clone_info = panel_client.name.clone();
                let id_clone_err = panel_client.name.clone();
                let client_id = panel_client.client.id();
                let client_id_info = panel_client.client.id();
                let client_id_err = panel_client.client.id();
                let security_context_manager_clone = security_context_manager.clone();
                let qh_clone = qh.clone();

                let mut process = Process::new()
                    .with_executable(&exec)
                    .with_args(args)
                    .with_on_stderr(move |_, _, out| {
                        // TODO why is span not included in logs to journald
                        let id_clone = id_clone_err.clone();
                        let client_id = client_id_err.clone();

                        async move {
                            error_span!("stderr", client = ?client_id).in_scope(|| {
                                error!("{}: {}", id_clone, out);
                            });
                        }
                    })
                    .with_on_stdout(move |_, _, out| {
                        let id_clone = id_clone_info.clone();
                        let client_id = client_id_info.clone();
                        // TODO why is span not included in logs to journald
                        async move {
                            info_span!("stdout", client = ?client_id).in_scope(|| {
                                info!("{}: {}", id_clone, out);
                            });
                        }
                    })
                    .with_on_exit(move |mut pman, key, err_code, is_restarting| {
                        if let Some(err_code) = err_code {
                            error!("Exited with error code {}", err_code)
                        }
                        let my_list = my_list.clone();
                        let mut display_handle = display_handle.clone();
                        let id_clone = id_clone.clone();
                        let applet_tx_clone = applet_tx_clone.clone();
                        let (c, client_socket) = get_client_sock(&mut display_handle);
                        let raw_client_socket = client_socket.as_raw_fd();
                        let client_id_clone = client_id.clone();
                        let mut applet_env = Vec::with_capacity(1);
                        let mut fds: Vec<OwnedFd> = Vec::with_capacity(2);
                        let should_restart = is_restarting && err_code.is_some();
                        let security_context = if requests_wayland_display && should_restart {
                            security_context_manager_clone.as_ref().and_then(
                                |security_context_manager| {
                                    security_context_manager
                                        .create_listener::<SpaceContainer>(&qh_clone)
                                        .ok()
                                        .map(|security_context| {
                                            security_context.set_sandbox_engine(NAME.to_string());
                                            security_context.commit();

                                            let data =
                                                security_context.data::<SecurityContext>().unwrap();
                                            let privileged_socket =
                                                data.conn.lock().unwrap().take().unwrap();
                                            applet_env.push((
                                                "X_PRIVILEGED_WAYLAND_SOCKET".to_string(),
                                                privileged_socket.as_raw_fd().to_string(),
                                            ));
                                            fds.push(privileged_socket.into());
                                            security_context
                                        })
                                },
                            )
                        } else {
                            None
                        };

                        async move {
                            if !should_restart {
                                _ = pman.stop_process(key).await;
                                return;
                            }

                            if is_notification_applet {
                                let (tx, rx) = oneshot::channel();
                                _ = applet_tx_clone
                                    .send(AppletMsg::NeedNewNotificationFd(tx))
                                    .await;
                                let Ok(fd) = rx.await else {
                                    error!("Failed to get new fd");
                                    return;
                                };
                                if let Err(err) = pman
                                    .update_process_env(&key, vec![(
                                        "COSMIC_NOTIFICATIONS".to_string(),
                                        fd.as_raw_fd().to_string(),
                                    )])
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

                            if let Some(old_client) = my_list
                                .lock()
                                .unwrap()
                                .iter_mut()
                                .find(|PanelClient { name, .. }| name == &id_clone)
                            {
                                old_client.client = c;
                                old_client.security_ctx = security_context;
                                info!("Replaced the client socket");
                            } else {
                                error!("Failed to find matching client... {}", &id_clone)
                            }
                            let _ = applet_tx_clone
                                .send(AppletMsg::ClientSocketPair(client_id_clone))
                                .await;
                            applet_env.push((
                                "WAYLAND_SOCKET".to_string(),
                                raw_client_socket.to_string(),
                            ));
                            let _ = pman.update_process_env(&key, applet_env.clone()).await;
                        }
                    });

                let msg = if is_notification_applet {
                    AppletMsg::NewNotificationsProcess(self.id(), process, applet_env, fds)
                } else {
                    process = process.with_fds(move || fds);

                    AppletMsg::NewProcess(self.id(), process.with_env(applet_env))
                };
                match self.applet_tx.try_send(msg) {
                    Ok(_) => {},
                    Err(e) => error!("{e}"),
                };
            }

            info!("Done spawning applets");
            Ok(())
        } else {
            anyhow::bail!("Clients have already been spawned!");
        }
    }

    fn destroy(&mut self) {
        unimplemented!()
    }

    fn visibility(&self) -> Visibility {
        self.visibility
    }

    fn raise_window(&mut self, w: &Window, activate: bool) {
        self.space.raise_element(&CosmicMappedInternal::Window(w.clone()), activate);
    }

    fn dirty_window(&mut self, _dh: &DisplayHandle, s: &s_WlSurface) {
        self.is_dirty = true;
        self.last_dirty = Some(Instant::now());
        if let Some(w) = self
            .space
            .elements()
            .filter_map(|w| if let CosmicMappedInternal::Window(w) = w { Some(w) } else { None })
            .find(|w| w.wl_surface().is_some_and(|w| w.as_ref() == s))
        {
            w.on_commit();
            w.refresh();
        }

        // check unmapped and overflow
        if let Some(w) =
            self.unmapped.iter().find(|w| w.wl_surface().is_some_and(|w| w.as_ref() == s))
        {
            w.on_commit();
            w.refresh();
        }

        if let Some(w) = self
            .overflow_left
            .elements()
            .chain(self.overflow_center.elements().chain(self.overflow_right.elements()))
            .find_map(|w| {
                if let PopupMappedInternal::Window(w) = w {
                    w.wl_surface().is_some_and(|w| w.as_ref() == s).then_some(w)
                } else {
                    None
                }
            })
        {
            w.on_commit();
            w.refresh();
        }
    }

    fn dirty_popup(&mut self, _dh: &DisplayHandle, s: &s_WlSurface) {
        self.is_dirty = true;
        self.space.refresh();

        if let Some(p) = self.popups.iter_mut().find(|p| p.s_surface.wl_surface() == s) {
            let p_bbox = bbox_from_surface_tree(p.s_surface.wl_surface(), (0, 0))
                .to_physical(1)
                .to_f64()
                .to_logical(self.scale)
                .to_i32_round();
            let p_geo = PopupKind::Xdg(p.s_surface.clone()).geometry();
            if p_bbox != p.popup.rectangle && p_bbox.size.w > 0 && p_bbox.size.h > 0 {
                p.popup.c_popup.xdg_surface().set_window_geometry(
                    p_geo.loc.x,
                    p_geo.loc.y,
                    p_geo.size.w.max(1),
                    p_geo.size.h.max(1),
                );
                if let Some((input_regions, my_region)) =
                    with_states(p.s_surface.wl_surface(), |states| {
                        states
                            .cached_state
                            .current::<SurfaceAttributes>()
                            .input_region
                            .as_ref()
                            .cloned()
                    })
                    .zip(p.popup.input_region.as_ref())
                {
                    my_region.subtract(p_bbox.loc.x, p_bbox.loc.y, p_bbox.size.w, p_bbox.size.h);
                    for r in input_regions.rects {
                        my_region.add(0, 0, r.1.size.w, r.1.size.h);
                    }
                    p.popup.c_popup.wl_surface().set_input_region(Some(my_region.wl_region()));
                }

                p.popup.state = Some(WrapperPopupState::Rectangle {
                    x: p_bbox.loc.x,
                    y: p_bbox.loc.y,
                    width: p_bbox.size.w.max(1),
                    height: p_bbox.size.h.max(1),
                });
            }
            p.popup.dirty = true;
        }
    }

    // XXX the renderer is provided by the container, not tracked by the PanelSpace
    fn renderer(&mut self) -> Option<&mut GlesRenderer> {
        unimplemented!()
    }

    fn setup(
        &mut self,
        _compositor_state: &CompositorState,
        _fractional_scale_manager: Option<&FractionalScalingManager>,
        _security_context_manager: Option<SecurityContextManager>,
        _viewport: Option<&ViewporterState>,
        _layer_state: &mut LayerShell,
        _conn: &Connection,
        _qh: &QueueHandle<GlobalState>,
    ) {
    }

    /// returns false to forward the button press, and true to intercept
    fn handle_button(&mut self, seat_name: &str, press: bool) -> Option<SpaceTarget> {
        self.generated_ptr_event_count = self.generated_ptr_event_count.saturating_sub(1);

        if let Some(prev_foc) = {
            let c_hovered_surface: &ClientFocus = &self.c_hovered_surface.borrow();

            c_hovered_surface
                .iter()
                .enumerate()
                .find(|(_, f)| f.1 == seat_name)
                .map(|(i, f)| (i, f.0.clone()))
        } {
            // close popups when panel is pressed
            if self.layer.as_ref().map(|s| s.wl_surface()) == Some(&prev_foc.1)
                && !self.popups.is_empty()
                && press
            {
                self.close_popups();
            }
            self.s_hovered_surface.iter().find_map(|h| {
                if h.seat_name.as_str() == seat_name {
                    Some(h.surface.clone().into())
                } else {
                    None
                }
            })
        } else {
            // no hover found
            // if has keyboard focus remove it and close popups
            self.keyboard_leave(seat_name, None);
            None
        }
    }

    ///  update active window based on pointer location
    fn update_pointer(
        &mut self,
        (x, y): (i32, i32),
        seat_name: &str,
        c_wl_surface: c_wl_surface::WlSurface,
    ) -> Option<ServerPointerFocus> {
        let mut prev_hover =
            self.s_hovered_surface.iter_mut().enumerate().find(|(_, f)| f.seat_name == seat_name);
        let prev_foc = self.s_focused_surface.iter_mut().find(|f| f.1 == seat_name);
        // first check if the motion is on a popup's client surface

        let mut cur_client_hover_id = None;
        let mut hover_relative_loc = None;
        let mut hover_geo = None;

        let ret = if let Some(p) =
            self.popups.iter().find(|p| p.popup.c_popup.wl_surface() == &c_wl_surface)
        {
            let geo = smithay::desktop::PopupKind::Xdg(p.s_surface.clone()).geometry();
            // special handling for popup bc they exist on their own client surface

            if let Some(prev_foc) = prev_foc {
                prev_foc.0 = p.s_surface.wl_surface().clone().into();
            } else {
                self.s_focused_surface
                    .push((p.s_surface.wl_surface().clone().into(), seat_name.to_string()));
            }
            if let Some((_, prev_foc)) = prev_hover.as_mut() {
                prev_foc.c_pos = p.popup.rectangle.loc;
                prev_foc.s_pos = (p.popup.rectangle.loc - geo.loc).to_f64();

                prev_foc.surface = p.s_surface.wl_surface().clone().into();
                Some(prev_foc.clone())
            } else {
                self.s_hovered_surface.push(ServerPointerFocus {
                    surface: p.s_surface.wl_surface().clone().into(),
                    seat_name: seat_name.to_string(),
                    c_pos: p.popup.rectangle.loc,
                    s_pos: (p.popup.rectangle.loc - geo.loc).to_f64(),
                });
                self.s_hovered_surface.last().cloned()
            }
        } else if self.layer.as_ref().is_some_and(|s| *s.wl_surface() == c_wl_surface) {
            // if not on this panel's client surface return None

            // FIXME
            // There has to be a way to avoid messing with the scaling like this...
            let space_focus = self.space.elements().rev().find_map(|e| {
                let Some(render_location) = self.space.element_location(e) else {
                    self.generated_ptr_event_count =
                        self.generated_ptr_event_count.saturating_sub(1);
                    return None;
                };

                let mut bbox = e.bbox().to_f64();
                bbox.size.w /= self.scale;
                bbox.size.h /= self.scale;
                bbox.loc.x = render_location.x as f64;
                bbox.loc.y = render_location.y as f64;
                if bbox.contains((x as f64, y as f64)) {
                    Some((e.clone(), render_location))
                } else {
                    None
                }
            });

            if let Some((target, relative_loc)) = space_focus {
                let geo =
                    target.bbox().to_f64().to_physical(1.0).to_logical(self.scale).to_i32_round();
                if let Some(prev_kbd) = prev_foc {
                    prev_kbd.0 = target.clone().into();
                } else {
                    self.s_focused_surface.push((target.clone().into(), seat_name.to_string()));
                }

                hover_geo = Some(geo);
                hover_relative_loc = Some(relative_loc);
                cur_client_hover_id = target.wl_surface().and_then(|t| t.client().map(|c| c.id()));

                if let Some((_, prev_foc)) = prev_hover.as_mut() {
                    prev_foc.s_pos = relative_loc.to_f64();
                    prev_foc.c_pos = geo.loc;
                    prev_foc.surface = target.into();
                    Some(prev_foc.clone())
                } else {
                    self.s_hovered_surface.push(ServerPointerFocus {
                        surface: target.into(),
                        seat_name: seat_name.to_string(),
                        c_pos: geo.loc,
                        s_pos: relative_loc.to_f64(),
                    });
                    self.s_hovered_surface.last().cloned()
                }
            } else {
                if let Some((prev_i, _)) = prev_hover {
                    self.s_hovered_surface.swap_remove(prev_i);
                }
                None
            }
        } else if self
            .overflow_popup
            .as_ref()
            .is_some_and(|p| p.0.c_popup.wl_surface() == &c_wl_surface)
        {
            let (popup, section) = self.overflow_popup.as_ref().unwrap();
            let space = match section {
                OverflowSection::Left => &self.overflow_left,
                OverflowSection::Center => &self.overflow_center,
                OverflowSection::Right => &self.overflow_right,
            };

            let space_focus = space.elements().rev().find_map(|e| {
                let Some(w) = (match e {
                    PopupMappedInternal::Window(w) => w.wl_surface(),
                    _ => return None,
                }) else {
                    return None;
                };
                let Some(render_location) = space.element_location(e) else {
                    self.generated_ptr_event_count =
                        self.generated_ptr_event_count.saturating_sub(1);
                    return None;
                };

                let mut bbox = e.bbox().to_f64();
                bbox.size.w /= popup.scale;
                bbox.size.h /= popup.scale;
                bbox.loc.x = render_location.x as f64;
                bbox.loc.y = render_location.y as f64;
                if bbox.contains((x as f64, y as f64)) {
                    Some((e.bbox().to_f64(), w.into_owned(), render_location))
                } else {
                    None
                }
            });

            if let Some((bbox, target, relative_loc)) = space_focus {
                let geo = bbox.to_physical(1.0).to_logical(self.scale).to_i32_round();
                if let Some(prev_kbd) = prev_foc {
                    prev_kbd.0 = SpaceTarget::Surface(target.clone());
                } else {
                    self.s_focused_surface.push((target.clone().into(), seat_name.to_string()));
                }

                hover_geo = Some(geo);
                hover_relative_loc = Some(relative_loc);
                cur_client_hover_id = target.wl_surface().and_then(|t| t.client().map(|c| c.id()));

                if let Some((_, prev_foc)) = prev_hover.as_mut() {
                    prev_foc.s_pos = relative_loc.to_f64();
                    prev_foc.c_pos = geo.loc;
                    prev_foc.surface = target.into();
                    Some(prev_foc.clone())
                } else {
                    self.s_hovered_surface.push(ServerPointerFocus {
                        surface: target.into(),
                        seat_name: seat_name.to_string(),
                        c_pos: geo.loc,
                        s_pos: relative_loc.to_f64(),
                    });
                    self.s_hovered_surface.last().cloned()
                }
            } else {
                if let Some((prev_i, _)) = prev_hover {
                    self.s_hovered_surface.swap_remove(prev_i);
                }
                None
            }
        } else {
            if self
                .space
                .elements()
                .filter_map(|e| e.wl_surface())
                .chain(self.overflow_left.elements().filter_map(|e| e.wl_surface()))
                .any(|e| {
                    e.wl_surface()
                        .zip(prev_hover.as_ref().map(|s| s.1.surface.wl_surface()))
                        .is_some_and(|(s, prev_hover)| Some(s) == prev_hover)
                })
            {
                let (pos, _) = prev_hover.unwrap();
                self.s_hovered_surface.remove(pos);
            }
            self.generated_ptr_event_count = self.generated_ptr_event_count.saturating_sub(1);
            return None;
        };

        let prev_popup_client = self
            .popups
            .iter()
            .next()
            .and_then(|p| p.s_surface.wl_surface().client())
            .map(|c| c.id());

        if prev_popup_client.is_some()
            && prev_popup_client != cur_client_hover_id
            && self.generated_ptr_event_count == 0
        {
            // send press to new client if it hover flag is set
            let left_guard = self.clients_left.lock().unwrap();
            let center_guard = self.clients_center.lock().unwrap();
            let right_guard = self.clients_right.lock().unwrap();

            if let Some(((c, relative_loc), geo)) = left_guard
                .iter()
                .chain(center_guard.iter())
                .chain(right_guard.iter())
                .find(|c| {
                    c.auto_popup_hover_press.is_some() && Some(c.client.id()) == cur_client_hover_id
                })
                .zip(hover_relative_loc)
                .zip(hover_geo)
            {
                let mut p = (x, y);
                let effective_anchor =
                    match (c.auto_popup_hover_press.unwrap(), self.config.is_horizontal()) {
                        (AppletAutoClickAnchor::Start, true) => AppletAutoClickAnchor::Left,
                        (AppletAutoClickAnchor::Start, false) => AppletAutoClickAnchor::Top,
                        (AppletAutoClickAnchor::End, true) => AppletAutoClickAnchor::Right,
                        (AppletAutoClickAnchor::End, false) => AppletAutoClickAnchor::Bottom,
                        (anchor, _) => anchor,
                    };
                match effective_anchor {
                    AppletAutoClickAnchor::Top => {
                        // centered on the top edge
                        p.0 = relative_loc.x + geo.size.w as i32 / 2;
                        p.1 = relative_loc.y + 4;
                    },
                    AppletAutoClickAnchor::Bottom => {
                        // centered on the bottom edge
                        p.0 = relative_loc.x + geo.size.w as i32 / 2;
                        p.1 = relative_loc.y + geo.size.h as i32 - 4;
                    },
                    AppletAutoClickAnchor::Left => {
                        // centered on the left edge
                        p.0 = relative_loc.x + 4;
                        p.1 = relative_loc.y + geo.size.h as i32 / 2;
                    },
                    AppletAutoClickAnchor::Right => {
                        // centered on the right edge
                        p.0 = relative_loc.x + geo.size.w as i32 - 4;
                        p.1 = relative_loc.y + geo.size.h as i32 / 2;
                    },
                    AppletAutoClickAnchor::Center => {
                        // centered on the center
                        p.0 = relative_loc.x + geo.size.w as i32 / 2;
                        p.1 = relative_loc.y + geo.size.h as i32 / 2;
                    },
                    AppletAutoClickAnchor::Auto => {
                        let relative_x = x - relative_loc.x;
                        let relative_y = y - relative_loc.y;
                        if relative_x.abs() < 4 {
                            p.0 += 4;
                        } else if (relative_x - geo.size.w as i32).abs() < 4 {
                            p.0 -= 4;
                        }
                        if relative_y.abs() < 4 {
                            p.1 += 4;
                        } else if (relative_y - geo.size.h as i32).abs() < 4 {
                            p.1 -= 4;
                        }
                    },
                    AppletAutoClickAnchor::Start | AppletAutoClickAnchor::End => {
                        tracing::warn!("Invalid anchor for auto click");
                        // should be handled above
                    },
                }

                self.generated_pointer_events = vec![
                    PointerEvent {
                        surface: self.layer.as_ref().unwrap().wl_surface().clone(),
                        position: (p.0 as f64, p.1 as f64),
                        kind: sctk::seat::pointer::PointerEventKind::Motion { time: 0 },
                    },
                    PointerEvent {
                        surface: self.layer.as_ref().unwrap().wl_surface().clone(),
                        position: (p.0 as f64, p.1 as f64),
                        kind: sctk::seat::pointer::PointerEventKind::Press {
                            time: 0,
                            button: BTN_LEFT,
                            serial: 0,
                        },
                    },
                    PointerEvent {
                        surface: self.layer.as_ref().unwrap().wl_surface().clone(),
                        position: (p.0 as f64, p.1 as f64),
                        kind: sctk::seat::pointer::PointerEventKind::Release {
                            time: 0,
                            button: BTN_LEFT,
                            serial: 0,
                        },
                    },
                ];
            }
        } else if prev_popup_client.is_some() && self.generated_ptr_event_count == 0 {
            // TODO simulate button press on the overflow button...
        }
        self.generated_ptr_event_count = self.generated_ptr_event_count.saturating_sub(1);
        ret
    }

    fn keyboard_leave(&mut self, seat_name: &str, _: Option<c_wl_surface::WlSurface>) {
        let prev_len = self.s_focused_surface.len();
        self.s_focused_surface.retain(|(_, name)| name != seat_name);

        if prev_len != self.s_focused_surface.len() {
            self.close_popups();
        }
    }

    fn keyboard_enter(&mut self, _: &str, _: c_wl_surface::WlSurface) -> Option<s_WlSurface> {
        None
    }

    fn pointer_leave(&mut self, seat_name: &str, _s: Option<c_wl_surface::WlSurface>) {
        self.generated_ptr_event_count = self.generated_ptr_event_count.saturating_sub(1);

        self.s_hovered_surface.retain(|focus| focus.seat_name != seat_name);
    }

    fn pointer_enter(
        &mut self,
        dim: (i32, i32),
        seat_name: &str,
        c_wl_surface: c_wl_surface::WlSurface,
    ) -> Option<ServerPointerFocus> {
        self.update_pointer(dim, seat_name, c_wl_surface)
    }

    // TODO
    fn configure_popup(
        &mut self,
        _popup: &sctk::shell::xdg::popup::Popup,
        _config: sctk::shell::xdg::popup::PopupConfigure,
    ) {
    }

    fn close_popup(&mut self, popup: &sctk::shell::xdg::popup::Popup) {
        self.popups.retain(|p| {
            if p.popup.c_popup.wl_surface() == popup.wl_surface() {
                if p.s_surface.alive() {
                    p.s_surface.send_popup_done();
                }
                false
            } else {
                true
            }
        });
    }

    // handled by custom method with access to renderer instead
    fn configure_layer(&mut self, _: &LayerSurface, _: LayerSurfaceConfigure) {}

    // handled by the container
    fn close_layer(&mut self, _: &LayerSurface) {}

    // handled by the container
    fn output_leave(
        &mut self,
        _c_output: c_wl_output::WlOutput,
        _s_output: Output,
    ) -> anyhow::Result<()> {
        anyhow::bail!("Output leaving should be handled by the container")
    }

    fn update_output(
        &mut self,
        c_output: c_wl_output::WlOutput,
        s_output: Output,
        info: OutputInfo,
    ) -> anyhow::Result<bool> {
        self.output.replace((c_output, s_output, info));
        self.dimensions = self.constrain_dim(self.dimensions.clone(), Some(self.gap() as u32));
        self.is_dirty = true;
        Ok(true)
    }

    fn new_output(
        &mut self,
        compositor_state: &sctk::compositor::CompositorState,
        fractional_scale_manager: Option<&FractionalScalingManager>,
        viewport: Option<&ViewporterState>,
        layer_state: &mut LayerShell,
        _conn: &sctk::reexports::client::Connection,
        qh: &QueueHandle<GlobalState>,
        c_output: Option<c_wl_output::WlOutput>,
        s_output: Option<Output>,
        output_info: Option<OutputInfo>,
    ) -> anyhow::Result<()> {
        if self.output.is_some() {
            bail!("output already setup for this panel");
        }
        if let (Some(_c_output), Some(s_output), Some(output_info)) =
            (c_output.as_ref(), s_output.as_ref(), output_info.as_ref())
        {
            self.space.map_output(s_output, output_info.location);
            match &self.config.output {
                CosmicPanelOuput::Active => {
                    bail!("output does not match config")
                },
                CosmicPanelOuput::Name(config_name)
                    if output_info.name != Some(config_name.to_string()) =>
                {
                    bail!("output does not match config")
                },
                _ => {},
            };
            if matches!(self.config.output, CosmicPanelOuput::Active) && self.layer.is_some() {
                return Ok(());
            }
        } else if !matches!(self.config.output, CosmicPanelOuput::Active) {
            bail!("output does not match config");
        }
        let dimensions: Size<i32, Logical> =
            self.constrain_dim((0, 0).into(), Some(self.gap() as u32));

        let layer = match self.config().layer() {
            zwlr_layer_shell_v1::Layer::Background => Layer::Background,
            zwlr_layer_shell_v1::Layer::Bottom => Layer::Bottom,
            zwlr_layer_shell_v1::Layer::Top => Layer::Top,
            zwlr_layer_shell_v1::Layer::Overlay => Layer::Overlay,
            _ => bail!("Invalid layer"),
        };

        let surface = compositor_state.create_surface(&qh);
        let client_surface =
            layer_state.create_layer_surface(&qh, surface, layer, Some("Panel"), c_output.as_ref());
        // client_surface.set_margin(margin.top, margin.right, margin.bottom,
        // margin.left);
        client_surface.set_keyboard_interactivity(match self.config.keyboard_interactivity {
            xdg_shell_wrapper_config::KeyboardInteractivity::None => KeyboardInteractivity::None,
            xdg_shell_wrapper_config::KeyboardInteractivity::Exclusive => {
                KeyboardInteractivity::Exclusive
            },
            xdg_shell_wrapper_config::KeyboardInteractivity::OnDemand => {
                KeyboardInteractivity::OnDemand
            },
        });
        client_surface.set_size(dimensions.w.try_into().unwrap(), dimensions.h.try_into().unwrap());

        client_surface.set_anchor(self.config.anchor.into());

        let input_region = Region::new(compositor_state)?;
        client_surface.wl_surface().set_input_region(Some(input_region.wl_region()));
        self.input_region.replace(input_region);

        let fractional_scale = fractional_scale_manager
            .map(|f| f.fractional_scaling(client_surface.wl_surface(), &qh));

        let viewport = viewport.map(|v| v.get_viewport(client_surface.wl_surface(), &qh));

        client_surface.commit();

        let next_render_event = Rc::new(Cell::new(Some(SpaceEvent::WaitConfigure {
            first: true,
            width: dimensions.w,
            height: dimensions.h,
        })));

        self.output =
            izip!(c_output.into_iter(), s_output.into_iter(), output_info.as_ref().cloned()).next();
        self.layer = Some(client_surface);
        self.layer_fractional_scale = fractional_scale;
        self.layer_viewport = viewport;
        self.dimensions = dimensions;
        self.space_event = next_render_event;
        self.is_dirty = true;
        self.left_overflow_button_id = id::Id::new(format!("left_overflow_button_{}", self.id()));
        self.right_overflow_button_id = id::Id::new(format!("right_overflow_button_{}", self.id()));
        self.center_overflow_button_id =
            id::Id::new(format!("center_overflow_button_{}", self.id()));
        self.left_overflow_popup_id = id::Id::new(format!("left_overflow_popup_{}", self.id()));
        self.right_overflow_popup_id = id::Id::new(format!("right_overflow_popup_{}", self.id()));
        self.center_overflow_popup_id = id::Id::new(format!("center_overflow_popup_{}", self.id()));

        if let Err(err) = self.spawn_clients(
            self.s_display.clone().unwrap(),
            &qh,
            self.security_context_manager.clone(),
        ) {
            error!(?err, "Failed to spawn clients");
        }
        Ok(())
    }

    fn handle_events(
        &mut self,
        _dh: &DisplayHandle,
        _qh: &QueueHandle<GlobalState>,
        _popup_manager: &mut PopupManager,
        _time: u32,
    ) -> Instant {
        unimplemented!()
    }

    fn frame(&mut self, surface: &c_wl_surface::WlSurface, _time: u32) {
        if Some(surface) == self.layer.as_ref().map(|l| l.wl_surface()) {
            self.has_frame = true;
        } else if let Some(p) =
            self.popups.iter_mut().find(|p| surface == p.popup.c_popup.wl_surface())
        {
            p.popup.has_frame = true;
        }
    }

    fn get_scale_factor(&self, surface: &s_WlSurface) -> std::option::Option<f64> {
        let client = surface.client();
        if self
            .clients_center
            .lock()
            .unwrap()
            .iter()
            .chain(self.clients_left.lock().unwrap().iter())
            .chain(self.clients_right.lock().unwrap().iter())
            .any(|c| Some(&c.client) == client.as_ref())
        {
            Some(self.scale)
        } else {
            None
        }
    }

    fn scale_factor_changed(
        &mut self,
        surface: &c_wl_surface::WlSurface,
        scale: f64,
        legacy: bool,
    ) {
        info!(
            "Scale factor changed {scale} for as surface in space \"{}\" on {}",
            self.config.name,
            self.output
                .as_ref()
                .and_then(|o| o.2.name.clone())
                .unwrap_or_else(|| "None".to_string())
        );
        if Some(surface) == self.layer.as_ref().map(|l| l.wl_surface()) {
            self.scale_change_retries = 10;
            self.scale = scale;
            self.is_dirty = true;
            if legacy && self.layer_fractional_scale.is_none() {
                surface.set_buffer_scale(scale as i32);
            } else {
                surface.set_buffer_scale(1);
                if let Some(viewport) = self.layer_viewport.as_ref() {
                    viewport.set_destination(self.actual_size.w.max(1), self.actual_size.h.max(1));
                }

                for surface in self.space.elements().filter_map(|e| e.wl_surface().clone()) {
                    with_states(&surface, |states| {
                        with_fractional_scale(states, |fractional_scale| {
                            fractional_scale.set_preferred_scale(scale);
                        });
                    });
                }
            }
        }
        for popup in &mut self.popups {
            if popup.popup.c_popup.wl_surface() != surface {
                continue;
            }
            popup.popup.scale = scale;
            let Rectangle { loc, size } = popup.popup.rectangle;
            if popup.popup.state.is_none() {
                popup.popup.state = Some(WrapperPopupState::Rectangle {
                    x: loc.x,
                    y: loc.y,
                    width: size.w,
                    height: size.h,
                });
            }

            with_states(&popup.s_surface.wl_surface(), |states| {
                with_fractional_scale(states, |fractional_scale| {
                    fractional_scale.set_preferred_scale(scale);
                });
            });
        }
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _surface: &c_wl_surface::WlSurface,
        _new_transform: cctk::sctk::reexports::client::protocol::wl_output::Transform,
    ) {
        // TODO handle the preferred transform
    }

    fn generate_pointer_events(&mut self) -> Vec<PointerEvent> {
        self.generated_ptr_event_count = self.generated_pointer_events.len();
        std::mem::take(&mut self.generated_pointer_events)
    }
}
