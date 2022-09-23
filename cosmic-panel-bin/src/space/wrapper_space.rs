// SPDX-License-Identifier: MPL-2.0-only

use std::{
    cell::{Cell, RefCell},
    ffi::OsString,
    fs,
    os::unix::prelude::AsRawFd,
    rc::Rc,
    time::Instant,
};

use anyhow::bail;
use freedesktop_desktop_entry::{self, DesktopEntry, Iter};
use itertools::{izip, Itertools};
use launch_pad::process::Process;
use sctk::{
    compositor::{CompositorState, Region},
    output::OutputInfo,
    reexports::{
        client::{
            protocol::{wl_output as c_wl_output, wl_surface as c_wl_surface},
            Connection, Proxy, QueueHandle,
        },
        protocols::xdg::shell::client::xdg_positioner::{Anchor, Gravity},
    },
    shell::{
        layer::{self, KeyboardInteractivity, Layer, LayerState, LayerSurface},
        xdg::popup,
    },
};
use shlex::Shlex;
use slog::{trace, Logger};
use smithay::{
    backend::renderer::gles2::Gles2Renderer,
    desktop::{
        utils::bbox_from_surface_tree, Kind, PopupKind, PopupManager, Window, WindowSurfaceType,
    },
    output::Output,
    reexports::wayland_server::{
        self, protocol::wl_surface::WlSurface as s_WlSurface, DisplayHandle,
    },
    utils::{Logical, Rectangle, Size},
    wayland::{
        compositor::{with_states, SurfaceAttributes},
        shell::xdg::{PopupSurface, PositionerState, SurfaceCachedState},
    },
};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1;
use xdg_shell_wrapper::{
    client_state::ClientFocus,
    server_state::ServerPointerFocus,
    shared_state::GlobalState,
    space::{SpaceEvent, Visibility, WrapperPopup, WrapperPopupState, WrapperSpace},
    util::get_client_sock,
};

use cosmic_panel_config::{CosmicPanelConfig, CosmicPanelOuput};

use crate::space::AppletMsg;

use super::PanelSpace;

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

    fn handle_events(&mut self, _: &DisplayHandle, _: &mut PopupManager, _: u32) -> Instant {
        panic!("this should not be called");
    }

    fn add_window(&mut self, w: Window) {
        self.full_clear = 4;
        self.space.commit(w.toplevel().wl_surface());
        self.space
            .map_window(&w, (0, 0), self.z_index().map(|z| z as u8), true);
        for w in self.space.windows() {
            w.configure();
        }
    }

    fn add_popup<W: WrapperSpace>(
        &mut self,
        compositor_state: &sctk::compositor::CompositorState,
        _conn: &sctk::reexports::client::Connection,
        qh: &QueueHandle<GlobalState<W>>,
        xdg_shell_state: &mut sctk::shell::xdg::XdgShellState,
        s_surface: PopupSurface,
        positioner: &sctk::shell::xdg::XdgPositioner,
        positioner_state: PositionerState,
    ) -> anyhow::Result<()> {
        let PositionerState {
            rect_size,
            anchor_rect,
            anchor_edges,
            gravity,
            constraint_adjustment,
            offset,
            reactive,
            parent_size,
            parent_configure: _,
        } = positioner_state;
        // TODO handle popups not on main surface
        if !self.popups.is_empty() {
            self.popups.clear();
            return Ok(());
        }

        let parent_window = if let Some(s) = self.space.windows().find(|w| match w.toplevel() {
            Kind::Xdg(wl_s) => Some(wl_s.wl_surface()) == s_surface.get_parent_surface().as_ref(),
        }) {
            s
        } else {
            bail!("Could not find parent window");
        };

        let c_wl_surface = compositor_state.create_surface(qh)?;

        let p_offset = self
            .space
            .window_location(parent_window)
            .unwrap_or_else(|| (0, 0).into());

        positioner.set_size(rect_size.w, rect_size.h);
        positioner.set_anchor_rect(
            anchor_rect.loc.x + p_offset.x,
            anchor_rect.loc.y + p_offset.y,
            anchor_rect.size.w,
            anchor_rect.size.h,
        );
        positioner.set_anchor(Anchor::try_from(anchor_edges as u32).unwrap_or(Anchor::None));
        positioner.set_gravity(Gravity::try_from(gravity as u32).unwrap_or(Gravity::None));

        positioner.set_constraint_adjustment(u32::from(constraint_adjustment));
        positioner.set_offset(offset.x, offset.y);
        if positioner.version() >= 3 {
            if reactive {
                positioner.set_reactive();
            }
            if let Some(parent_size) = parent_size {
                positioner.set_parent_size(parent_size.w, parent_size.h);
            }
        }
        let c_popup = popup::Popup::from_surface(
            None,
            positioner,
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
                s_window_geometry.size.w,
                s_window_geometry.size.h,
            );
            for r in input_regions.rects {
                input_region.add(0, 0, r.1.size.w, r.1.size.h);
            }
            c_wl_surface.set_input_region(Some(input_region.wl_region()));
        }

        // get_popup is not implemented yet in sctk 0.30
        self.layer
            .as_ref()
            .unwrap()
            .get_popup(c_popup.xdg_popup());

        // //must be done after role is assigned as popup
        c_wl_surface.commit();

        let cur_popup_state = Some(WrapperPopupState::WaitConfigure);

        self.popups.push(WrapperPopup {
            c_popup,
            c_wl_surface,
            s_surface,
            egl_surface: None,
            dirty: false,
            rectangle: Rectangle::from_loc_and_size((0, 0), (0, 0)),
            accumulated_damage: Default::default(),
            full_clear: 4,
            state: cur_popup_state,
            input_region,
        });

        Ok(())
    }

    fn reposition_popup(
        &mut self,
        popup: PopupSurface,
        _positioner: &sctk::shell::xdg::XdgPositioner,
        _positioner_state: PositionerState,
        token: u32,
    ) -> anyhow::Result<()> {
        popup.send_repositioned(token);
        popup.send_configure()?;
        Ok(())
    }

    fn config(&self) -> Self::Config {
        self.config.clone()
    }

    fn spawn_clients(&mut self, mut display: DisplayHandle) -> anyhow::Result<()> {
        if self.clients_left.is_empty()
            && self.clients_center.is_empty()
            && self.clients_right.is_empty()
        {
            self.clients_left = self
                .config
                .plugins_left()
                .as_ref()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|id| {
                    let (c, s) = get_client_sock(&mut display);
                    (id, c, s)
                })
                .collect();

            self.clients_center = self
                .config
                .plugins_center()
                .as_ref()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|id| {
                    let (c, s) = get_client_sock(&mut display);
                    (id, c, s)
                })
                .collect();

            self.clients_right = self
                .config
                .plugins_right()
                .as_ref()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|id| {
                    let (c, s) = get_client_sock(&mut display);
                    (id, c, s)
                })
                .collect();

            let mut desktop_ids = self
                .clients_left
                .iter()
                .chain(self.clients_center.iter())
                .chain(self.clients_right.iter())
                .collect_vec();

            let config_size = self.config.size.to_string();
            let config_output = self.config.output.to_string();
            let config_anchor = self.config.anchor.to_string();
            let env_vars = vec![
                ("COSMIC_PANEL_SIZE", config_size.as_str()),
                ("COSMIC_PANEL_OUTPUT", config_output.as_str()),
                ("COSMIC_PANEL_ANCHOR", config_anchor.as_str()),
            ];

            for path in Iter::new(freedesktop_desktop_entry::default_paths()) {
                if let Some(position) = desktop_ids.iter().position(|(app_file_name, _, _)| {
                    Some(OsString::from(app_file_name).as_os_str()) == path.file_stem()
                }) {
                    // This way each applet is at most started once,
                    // even if multiple desktop files in different directories match
                    let (id, client, socket) = desktop_ids.remove(position);
                    if let Ok(bytes) = fs::read_to_string(&path) {
                        if let Ok(entry) = DesktopEntry::decode(&path, &bytes) {
                            if let Some(exec) = entry.exec() {
                                let requests_wayland_display =
                                    entry.desktop_entry("X-HostWaylandDisplay").is_some();
                                let mut exec_iter = Shlex::new(exec);
                                let exec = exec_iter
                                    .next()
                                    .expect("exec parameter must contain at least on word");

                                let mut args = Vec::new();
                                for arg in exec_iter {
                                    trace!(self.log.clone(), "child argument: {}", &arg);
                                    args.push(arg);
                                }
                                let mut applet_env = Vec::new();
                                for (key, val) in &env_vars {
                                    if !requests_wayland_display && *key == "WAYLAND_DISPLAY" {
                                        continue;
                                    }
                                    applet_env.push((*key, *val));
                                }
                                let fd = socket.as_raw_fd().to_string();
                                applet_env.push(("WAYLAND_SOCKET", fd.as_str()));
                                trace!(
                                    self.log.clone(),
                                    "child: {}, {:?} {:?}",
                                    &exec,
                                    args,
                                    applet_env
                                );

                                let log_clone = self.log.clone();
                                let display_handle = display.clone();
                                let applet_tx_clone = self.applet_tx.clone();
                                let id_clone = id.clone();
                                let client_id = client.id();
                                let process = Process::new()
                                .with_executable(&exec)
                                .with_args(args)
                                .with_env(applet_env)
                                .with_on_exit(move |mut pman, key, err_code, is_restarting| {
                                    let log_clone = log_clone.clone();
                                    let mut display_handle = display_handle.clone();
                                    let id_clone = id_clone.clone();
                                    let client_id_clone = client_id.clone();
                                    let applet_tx_clone = applet_tx_clone.clone();

                                    let (c, s) = get_client_sock(&mut display_handle);
                                    async move {
                                        if !is_restarting {
                                            if let Some(err_code) = err_code {
                                                slog::error!(log_clone, "Exited with error code and will not restart! {}", err_code);
                                            }
                                            return;
                                        }
                                        let fd = s.as_raw_fd().to_string();
                                        let _ = applet_tx_clone.send(AppletMsg::ClientSocketPair(id_clone, client_id_clone, c, s)).await;
                                        let _ = pman.update_process_env(&key, vec![("WAYLAND_SOCKET", fd.as_str())]).await;
                                    }
                                });

                                // TODO error handling
                                match self.applet_tx.try_send(AppletMsg::NewProcess(process)) {
                                    Ok(_) => {}
                                    Err(e) => eprintln!("{e}"),
                                };
                            }
                        }
                    }
                }
            }
            Ok(())
        } else {
            bail!("Clients have already been spawned!");
        }
    }

    fn log(&self) -> Option<Logger> {
        Some(self.log.clone())
    }

    fn destroy(&mut self) {
        // self.layer_shell_wl_surface
        //     .as_mut()
        //     .map(|wls| wls.destroy());
    }

    fn visibility(&self) -> Visibility {
        Visibility::Visible
    }

    fn raise_window(&mut self, w: &Window, activate: bool) {
        self.space.raise_window(w, activate);
    }

    fn dirty_window(&mut self, _dh: &DisplayHandle, s: &s_WlSurface) {
        self.last_dirty = Some(Instant::now());

        if let Some(w) = self.space.window_for_surface(s, WindowSurfaceType::ALL) {
            let old_bbox = w.bbox();
            self.space.commit(s);
            w.refresh();
            let new_bbox = w.bbox();
            if old_bbox.size != new_bbox.size {
                self.full_clear = 4;
            }

            // TODO improve this for when there are changes to the lists of plugins while running
            let padding: Size<i32, Logical> = (
                (2 * self.config.padding()).try_into().unwrap(),
                (2 * self.config.padding()).try_into().unwrap(),
            )
                .into();
            let size = self.constrain_dim(padding + w.bbox().size);
            let pending_dimensions = self.pending_dimensions.unwrap_or(self.dimensions);
            let mut wait_configure_dim = self
                .space_event
                .get()
                .map(|e| match e {
                    SpaceEvent::WaitConfigure { width, height, .. } => (width, height),
                    _ => self.dimensions.into(),
                })
                .unwrap_or_else(|| pending_dimensions.into());
            if self.dimensions.w < size.w
                && pending_dimensions.w < size.w
                && wait_configure_dim.0 < size.w
            {
                self.pending_dimensions = Some((size.w, wait_configure_dim.1).into());
                wait_configure_dim.0 = size.w;
            }
            if self.dimensions.h < size.h
                && pending_dimensions.h < size.h
                && wait_configure_dim.1 < size.h
            {
                self.pending_dimensions = Some((wait_configure_dim.0, size.h).into());
            }
        }
    }

    fn dirty_popup(&mut self, dh: &DisplayHandle, s: &s_WlSurface) {
        self.space.commit(s);
        self.space.refresh(dh);

        if let Some(p) = self
            .popups
            .iter_mut()
            .find(|p| p.s_surface.wl_surface() == s)
        {
            let p_bbox = bbox_from_surface_tree(p.s_surface.wl_surface(), (0, 0));
            let p_geo = PopupKind::Xdg(p.s_surface.clone()).geometry();
            if p_bbox != p.rectangle {
                p.c_popup.xdg_surface().set_window_geometry(
                    p_geo.loc.x,
                    p_geo.loc.y,
                    p_geo.size.w,
                    p_geo.size.h,
                );
                if let Some(input_regions) = with_states(p.s_surface.wl_surface(), |states| {
                    states
                        .cached_state
                        .current::<SurfaceAttributes>()
                        .input_region
                        .as_ref()
                        .cloned()
                }) {
                    p.input_region.subtract(
                        p_bbox.loc.x,
                        p_bbox.loc.y,
                        p_bbox.size.w,
                        p_bbox.size.h,
                    );
                    for r in input_regions.rects {
                        p.input_region.add(0, 0, r.1.size.w, r.1.size.h);
                    }
                    p.c_wl_surface
                        .set_input_region(Some(p.input_region.wl_region()));
                }
                p.state.replace(WrapperPopupState::Rectangle {
                    x: p_bbox.loc.x,
                    y: p_bbox.loc.y,
                    width: p_bbox.size.w,
                    height: p_bbox.size.h,
                });
            }
            p.dirty = true;
        }
    }

    // XXX the renderer is provided by the container, not tracked by the PanelSpace
    fn renderer(&mut self) -> Option<&mut Gles2Renderer> {
        None
    }

    fn setup<W: WrapperSpace>(
        &mut self,
        _compositor_state: &CompositorState,
        _layer_state: &mut LayerState,
        conn: &Connection,
        _qh: &QueueHandle<GlobalState<W>>,
    ) {
        self.c_display.replace(conn.display());
    }

    fn new_output<W: WrapperSpace>(
        &mut self,
        compositor_state: &sctk::compositor::CompositorState,
        layer_state: &mut LayerState,
        _conn: &sctk::reexports::client::Connection,
        qh: &QueueHandle<GlobalState<W>>,
        c_output: Option<c_wl_output::WlOutput>,
        s_output: Option<Output>,
        output_info: Option<OutputInfo>,
    ) -> anyhow::Result<()> {
        if let (Some(c_output), Some(s_output), Some(output_info)) =
            (c_output.as_ref(), s_output.as_ref(), output_info.as_ref())
        {
            self.space.map_output(s_output, output_info.location);
            match &self.config.output {
                CosmicPanelOuput::All | CosmicPanelOuput::Active => {
                    bail!("output does not match config")
                }
                CosmicPanelOuput::Name(config_name)
                    if output_info.name != Some(config_name.to_string()) =>
                {
                    bail!("output does not match config")
                }
                _ => {}
            };
            if matches!(self.config.output, CosmicPanelOuput::Active)
                && self.layer.is_some()
            {
                return Ok(());
            }
        } else if !matches!(self.config.output, CosmicPanelOuput::Active) {
            bail!("output does not match config");
        }

        let c_surface = compositor_state.create_surface(qh)?;
        let dimensions = self.constrain_dim((1, 1).into());

        let layer = match self.config().layer() {
            zwlr_layer_shell_v1::Layer::Background => Layer::Background,
            zwlr_layer_shell_v1::Layer::Bottom => Layer::Bottom,
            zwlr_layer_shell_v1::Layer::Top => Layer::Top,
            zwlr_layer_shell_v1::Layer::Overlay => Layer::Overlay,
            _ => bail!("Invalid layer"),
        };

        let mut layer_surface_builder = LayerSurface::builder()
            .keyboard_interactivity(match self.config.keyboard_interactivity {
                xdg_shell_wrapper_config::KeyboardInteractivity::None => {
                    KeyboardInteractivity::None
                }
                xdg_shell_wrapper_config::KeyboardInteractivity::Exclusive => {
                    KeyboardInteractivity::Exclusive
                }
                xdg_shell_wrapper_config::KeyboardInteractivity::OnDemand => {
                    KeyboardInteractivity::OnDemand
                }
            })
            .size((
                dimensions.w.try_into().unwrap(),
                dimensions.h.try_into().unwrap(),
            ));
        if let Some(output) = c_output.as_ref() {
            layer_surface_builder = layer_surface_builder.output(output);
        }
        let layer_surface = layer_surface_builder.map(qh, layer_state, c_surface.clone(), layer)?;
        layer_surface.set_anchor(match self.config.anchor {
            cosmic_panel_config::PanelAnchor::Left => layer::Anchor::LEFT,
            cosmic_panel_config::PanelAnchor::Right => layer::Anchor::RIGHT,
            cosmic_panel_config::PanelAnchor::Top => layer::Anchor::TOP,
            cosmic_panel_config::PanelAnchor::Bottom => layer::Anchor::BOTTOM,
        });

        c_surface.commit();
        let next_render_event = Rc::new(Cell::new(Some(SpaceEvent::WaitConfigure {
            first: true,
            width: dimensions.w,
            height: dimensions.h,
        })));

        self.output = izip!(
            c_output.into_iter(),
            s_output.into_iter(),
            output_info.as_ref().cloned()
        )
        .next();
        self.layer.replace(layer_surface);
        self.dimensions = dimensions;
        self.space_event = next_render_event;
        self.full_clear = 4;
        Ok(())
    }

    /// returns false to forward the button press, and true to intercept
    fn handle_press(&mut self, seat_name: &str) -> Option<s_WlSurface> {
        if let Some(prev_foc) = {
            let c_hovered_surface: &ClientFocus = &self.c_hovered_surface.borrow();

            c_hovered_surface
                .iter()
                .enumerate()
                .find(|(_, f)| f.1 == seat_name)
                .map(|(i, f)| (i, f.0.clone()))
        } {
            // close popups when panel is pressed
            if self.layer.as_ref().map(|s| s.wl_surface())== Some(&prev_foc.1) && !self.popups.is_empty()
            {
                self.close_popups();
            }
            self.s_hovered_surface.iter().find_map(|h| {
                if h.seat_name.as_str() == seat_name {
                    Some(h.surface.clone())
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
        let mut prev_hover = self
            .s_hovered_surface
            .iter_mut()
            .enumerate()
            .find(|(_, f)| f.seat_name == seat_name);
        let prev_foc = self.s_focused_surface.iter_mut().find(|f| f.1 == seat_name);
        // first check if the motion is on a popup's client surface
        if let Some(p) = self.popups.iter().find(|p| p.c_wl_surface == c_wl_surface) {
            let geo = smithay::desktop::PopupKind::Xdg(p.s_surface.clone()).geometry();
            // special handling for popup bc they exist on their own client surface

            if let Some(prev_foc) = prev_foc {
                prev_foc.0 = p.s_surface.wl_surface().clone();
            } else {
                self.s_focused_surface
                    .push((p.s_surface.wl_surface().clone(), seat_name.to_string()));
            }
            if let Some((_, prev_foc)) = prev_hover.as_mut() {
                prev_foc.c_pos = p.rectangle.loc;
                prev_foc.s_pos = p.rectangle.loc - geo.loc;

                prev_foc.surface = p.s_surface.wl_surface().clone();

                Some(prev_foc.clone())
            } else {
                self.s_hovered_surface.push(ServerPointerFocus {
                    surface: p.s_surface.wl_surface().clone(),
                    seat_name: seat_name.to_string(),
                    c_pos: p.rectangle.loc,
                    s_pos: p.rectangle.loc - geo.loc,
                });
                self.s_hovered_surface.last().cloned()
            }
        } else {
            // if not on this panel's client surface retun None
            if self
                .layer
                .as_ref()
                .map(|s| *s.wl_surface() != c_wl_surface)
                .unwrap_or(true)
            {
                return None;
            }
            if let Some((w, s, p)) = self
                .space
                .surface_under((x as f64, y as f64), WindowSurfaceType::ALL)
            {
                if let Some(prev_kbd) = prev_foc {
                    prev_kbd.0 = w.toplevel().wl_surface().clone();
                } else {
                    self.s_focused_surface
                        .push((w.toplevel().wl_surface().clone(), seat_name.to_string()));
                }
                if let Some((_, prev_foc)) = prev_hover.as_mut() {
                    prev_foc.s_pos = p;
                    prev_foc.c_pos = w.geometry().loc;
                    prev_foc.surface = s;
                    Some(prev_foc.clone())
                } else {
                    self.s_hovered_surface.push(ServerPointerFocus {
                        surface: s,
                        seat_name: seat_name.to_string(),
                        c_pos: w.geometry().loc,
                        s_pos: (x, y).into(),
                    });
                    self.s_hovered_surface.last().cloned()
                }
            } else {
                if let Some((prev_i, _)) = prev_hover {
                    self.s_hovered_surface.swap_remove(prev_i);
                }
                None
            }
        }
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

    fn pointer_leave(&mut self, seat_name: &str, _: Option<c_wl_surface::WlSurface>) {
        self.s_hovered_surface
            .retain(|focus| focus.seat_name != seat_name);
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
            if p.c_popup.wl_surface() == popup.wl_surface() {
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
    fn configure_layer(&mut self, _: &LayerSurface, _: sctk::shell::layer::LayerSurfaceConfigure) {}

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
    ) -> anyhow::Result<()> {
        self.output.replace((c_output, s_output, info));
        self.dimensions = self.constrain_dim(self.dimensions.clone());
        self.full_clear = 4;
        Ok(())
    }
}
