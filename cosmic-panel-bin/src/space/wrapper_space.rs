// SPDX-License-Identifier: MPL-2.0-only

use std::{
    cell::{Cell, RefCell},
    ffi::OsString,
    fs,
    os::unix::{net::UnixStream, prelude::AsRawFd},
    rc::Rc,
    time::Instant,
};

use anyhow::bail;
use freedesktop_desktop_entry::{self, DesktopEntry, Iter};
use itertools::Itertools;
use libc::c_int;
use sctk::{
    environment::Environment,
    output::OutputInfo,
    reexports::{
        client::protocol::{wl_output as c_wl_output, wl_surface as c_wl_surface},
        client::{self, Attached, Main},
        protocols::{
            wlr::unstable::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1},
            xdg_shell::client::{
                xdg_popup,
                xdg_positioner::{Anchor, Gravity, XdgPositioner},
                xdg_surface,
                xdg_wm_base::XdgWmBase,
            },
        },
    },
};
use slog::{info, trace, Logger};
use smithay::{
    backend::{
        egl::{
            context::{EGLContext, GlAttributes},
            display::EGLDisplay,
            ffi::{
                self,
                egl::{GetConfigAttrib, SwapInterval},
            },
            surface::EGLSurface,
        },
        renderer::gles2::Gles2Renderer,
    },
    desktop::{Kind, PopupKind, PopupManager, Space, Window, WindowSurfaceType},
    nix::libc,
    reexports::wayland_server::{protocol::wl_surface::WlSurface as s_WlSurface, DisplayHandle},
    utils::{Logical, Size},
    wayland::shell::xdg::{PopupSurface, PositionerState},
};
use wayland_egl::WlEglSurface;
use xdg_shell_wrapper::{
    client_state::{Env, Focus},
    config::WrapperConfig,
    space::{ClientEglSurface, Popup, PopupState, SpaceEvent, Visibility, WrapperSpace},
    util::{exec_child, get_client_sock},
};

use cosmic_panel_config::{CosmicPanelConfig, PanelAnchor};

use super::PanelSpace;

impl WrapperSpace for PanelSpace {
    type Config = CosmicPanelConfig;

    fn handle_events(&mut self, dh: &DisplayHandle, time: u32, f: &Focus) -> Instant {
        if self
            .children
            .iter_mut()
            .map(|c| c.try_wait())
            .all(|r| matches!(r, Ok(Some(_))))
        {
            info!(
                self.log.as_ref().unwrap().clone(),
                "Child processes exited. Now exiting..."
            );
            std::process::exit(0);
        }
        self.handle_focus(f);
        let mut should_render = false;
        match self.next_render_event.take() {
            Some(SpaceEvent::Quit) => {
                trace!(
                    self.log.as_ref().unwrap(),
                    "root layer shell surface removed, exiting..."
                );
                for child in &mut self.children {
                    let _ = child.kill();
                }
                std::process::exit(0);
            }
            Some(SpaceEvent::Configure {
                first,
                width,
                height,
                serial: _serial,
            }) => {
                if first {
                    let log = self.log.clone().unwrap();
                    let client_egl_surface = ClientEglSurface {
                        wl_egl_surface: WlEglSurface::new(
                            self.layer_shell_wl_surface.as_ref().unwrap(),
                            width,
                            height,
                        ),
                        display: self.c_display.as_ref().unwrap().clone(),
                    };
                    let egl_display = EGLDisplay::new(&client_egl_surface, log.clone())
                        .expect("Failed to initialize EGL display");

                    let egl_context = EGLContext::new_with_config(
                        &egl_display,
                        GlAttributes {
                            version: (3, 0),
                            profile: None,
                            debug: cfg!(debug_assertions),
                            vsync: false,
                        },
                        Default::default(),
                        log.clone(),
                    )
                    .expect("Failed to initialize EGL context");

                    let mut min_interval_attr = 23239;
                    unsafe {
                        GetConfigAttrib(
                            egl_display.get_display_handle().handle,
                            egl_context.config_id(),
                            ffi::egl::MIN_SWAP_INTERVAL as c_int,
                            &mut min_interval_attr,
                        );
                    }

                    let renderer = unsafe {
                        Gles2Renderer::new(egl_context, log.clone())
                            .expect("Failed to initialize EGL Surface")
                    };
                    trace!(log, "{:?}", unsafe {
                        SwapInterval(egl_display.get_display_handle().handle, 0)
                    });

                    let egl_surface = Rc::new(
                        EGLSurface::new(
                            &egl_display,
                            renderer
                                .egl_context()
                                .pixel_format()
                                .expect("Failed to get pixel format from EGL context "),
                            renderer.egl_context().config_id(),
                            client_egl_surface,
                            log.clone(),
                        )
                        .expect("Failed to initialize EGL Surface"),
                    );

                    self.renderer.replace(renderer);
                    self.egl_surface.replace(egl_surface);
                    self.egl_display.replace(egl_display);
                } else if self.dimensions != (width as i32, height as i32).into()
                    && self.pending_dimensions.is_none()
                {
                    self.w_accumulated_damage.drain(..);
                    self.egl_surface
                        .as_ref()
                        .unwrap()
                        .resize(width as i32, height as i32, 0, 0);
                }
                self.full_clear = 4;
                self.layer_shell_wl_surface.as_ref().unwrap().commit();
                self.dimensions = (width as i32, height as i32).into();
            }
            Some(SpaceEvent::WaitConfigure {
                first,
                width,
                height,
            }) => {
                self.next_render_event
                    .replace(Some(SpaceEvent::WaitConfigure {
                        first,
                        width,
                        height,
                    }));
            }
            None => {
                if let Some(size) = self.pending_dimensions.take() {
                    let width = size.w.try_into().unwrap();
                    let height = size.h.try_into().unwrap();

                    self.layer_surface.as_ref().unwrap().set_size(width, height);
                    if let Visibility::Hidden = self.visibility {
                        if self.config.exclusive_zone() {
                            self.layer_surface
                                .as_ref()
                                .unwrap()
                                .set_exclusive_zone(self.config.get_hide_handle().unwrap() as i32);
                        }
                        let target = match self.config.anchor() {
                            PanelAnchor::Left | PanelAnchor::Right => -(self.dimensions.w),
                            PanelAnchor::Top | PanelAnchor::Bottom => -(self.dimensions.h),
                        } + self.config.get_hide_handle().unwrap() as i32;
                        self.layer_surface
                            .as_ref()
                            .unwrap()
                            .set_margin(target, target, target, target);
                    } else if self.config.exclusive_zone() {
                        let list_thickness = match self.config.anchor() {
                            PanelAnchor::Left | PanelAnchor::Right => width,
                            PanelAnchor::Top | PanelAnchor::Bottom => height,
                        };
                        self.layer_surface
                            .as_ref()
                            .unwrap()
                            .set_exclusive_zone(list_thickness as i32);
                    }
                    self.layer_shell_wl_surface.as_ref().unwrap().commit();
                    self.next_render_event
                        .replace(Some(SpaceEvent::WaitConfigure {
                            first: false,
                            width: size.w,
                            height: size.h,
                        }));
                } else {
                    if self.full_clear == 4 {
                        self.update_window_locations();
                        self.space.refresh(&dh);
                    }
                    should_render = true;
                }
            }
        }

        self.popups.retain_mut(|p: &mut Popup| {
            p.handle_events(
                &mut self.popup_manager,
                self.renderer.as_ref().unwrap().egl_context(),
                self.egl_display.as_ref().unwrap(),
                self.c_display.as_ref().unwrap(),
            )
        });

        if should_render {
            let _ = self.render(time);
        }
        if let Some(egl_surface) = self.egl_surface.as_ref() {
            if egl_surface.get_size() != Some(self.dimensions.to_physical(1)) {
                self.full_clear = 4;
            }
        }

        self.last_dirty.unwrap_or_else(|| Instant::now())
    }

    fn popups(&self) -> Vec<&Popup> {
        self.popups.iter().collect_vec()
    }

    /// returns false to forward the button press, and true to intercept
    fn handle_button(&mut self, c_focused_surface: &c_wl_surface::WlSurface) -> bool {
        if **self.layer_shell_wl_surface.as_ref().unwrap() == *c_focused_surface
            && !self.popups.is_empty()
        {
            self.close_popups();
            true
        } else {
            false
        }
    }

    fn add_window(&mut self, w: Window) {
        self.full_clear = 4;
        self.space.commit(&w.toplevel().wl_surface());
        self.space
            .map_window(&w, (0, 0), self.z_index().map(|z| z as u8), true);
        for w in self.space.windows() {
            w.configure();
        }
    }

    fn add_popup(
        &mut self,
        env: &Environment<Env>,
        xdg_wm_base: &Attached<XdgWmBase>,
        s_surface: PopupSurface,
        positioner: Main<XdgPositioner>,
        PositionerState {
            rect_size,
            anchor_rect,
            anchor_edges,
            gravity,
            constraint_adjustment,
            offset,
            reactive,
            parent_size,
            parent_configure: _,
        }: PositionerState,
    ) {
        // TODO handle popups not on main surface
        if !self.popups.is_empty() {
            self.popups.clear();
            return;
        }

        let parent_window = if let Some(s) = self.space.windows().find(|w| match w.toplevel() {
            Kind::Xdg(wl_s) => Some(wl_s.wl_surface()) == s_surface.get_parent_surface().as_ref(),
        }) {
            s
        } else {
            return;
        };

        let c_wl_surface = env.create_surface().detach();
        let c_xdg_surface = xdg_wm_base.get_xdg_surface(&c_wl_surface);

        let wl_surface = s_surface.wl_surface().clone();
        let s_popup_surface = s_surface.clone();
        self.popup_manager
            .track_popup(PopupKind::Xdg(s_surface.clone()))
            .unwrap();
        self.popup_manager.commit(&wl_surface);

        let p_offset = self
            .space
            .window_location(parent_window)
            .unwrap_or_else(|| (0, 0).into());
        // dbg!(s.bbox().loc);
        positioner.set_size(rect_size.w, rect_size.h);
        positioner.set_anchor_rect(
            anchor_rect.loc.x + p_offset.x,
            anchor_rect.loc.y + p_offset.y,
            anchor_rect.size.w,
            anchor_rect.size.h,
        );
        positioner.set_anchor(Anchor::from_raw(anchor_edges as u32).unwrap_or(Anchor::None));
        positioner.set_gravity(Gravity::from_raw(gravity as u32).unwrap_or(Gravity::None));

        positioner.set_constraint_adjustment(u32::from(constraint_adjustment));
        positioner.set_offset(offset.x, offset.y);
        if positioner.as_ref().version() >= 3 {
            if reactive {
                positioner.set_reactive();
            }
            if let Some(parent_size) = parent_size {
                positioner.set_parent_size(parent_size.w, parent_size.h);
            }
        }
        let c_popup = c_xdg_surface.get_popup(None, &positioner);
        self.layer_surface.as_ref().unwrap().get_popup(&c_popup);

        //must be done after role is assigned as popup
        c_wl_surface.commit();

        let cur_popup_state = Rc::new(Cell::new(Some(PopupState::WaitConfigure(true))));
        c_xdg_surface.quick_assign(move |c_xdg_surface, e, _| {
            if let xdg_surface::Event::Configure { serial, .. } = e {
                c_xdg_surface.ack_configure(serial);
            }
        });

        let popup_state = cur_popup_state.clone();

        c_popup.quick_assign(move |_c_popup, e, _| {
            if let Some(PopupState::Closed) = popup_state.get().as_ref() {
                return;
            }

            match e {
                xdg_popup::Event::Configure {
                    x,
                    y,
                    width,
                    height,
                } => {
                    if popup_state.get() != Some(PopupState::Closed) {
                        let _ = s_popup_surface.send_configure();

                        let first = match popup_state.get() {
                            Some(PopupState::Configure { first, .. }) => first,
                            Some(PopupState::WaitConfigure(first)) => first,
                            _ => false,
                        };
                        popup_state.set(Some(PopupState::Configure {
                            first,
                            x,
                            y,
                            width,
                            height,
                        }));
                    }
                }
                xdg_popup::Event::PopupDone => {
                    popup_state.set(Some(PopupState::Closed));
                }
                xdg_popup::Event::Repositioned { token } => {
                    popup_state.set(Some(PopupState::Repositioned(token)));
                }
                _ => {}
            };
        });

        self.popups.push(Popup {
            c_popup,
            c_xdg_surface,
            c_wl_surface,
            s_surface,
            egl_surface: None,
            dirty: false,
            popup_state: cur_popup_state,
            position: (0, 0).into(),
            accumulated_damage: Default::default(),
            full_clear: 4,
        });
    }

    ///  update active window based on pointer location
    fn update_pointer(&mut self, (x, y): (i32, i32)) {
        // set new focused
        if let Some((_, s, _)) = self
            .space
            .surface_under((x as f64, y as f64), WindowSurfaceType::ALL)
        {
            self.focused_surface.borrow_mut().replace(s);
            return;
        }
        self.focused_surface.borrow_mut().take();
    }

    fn reposition_popup(
        &mut self,
        s_popup: PopupSurface,
        _: Main<XdgPositioner>,
        _: PositionerState,
        token: u32,
    ) -> anyhow::Result<()> {
        s_popup.send_repositioned(token);
        s_popup.send_configure()?;
        self.popup_manager.commit(s_popup.wl_surface());

        Ok(())
    }

    fn next_space_event(&self) -> Rc<Cell<Option<SpaceEvent>>> {
        Rc::clone(&self.next_render_event)
    }

    fn config(&self) -> Self::Config {
        self.config.clone()
    }

    fn spawn_clients(
        &mut self,
        display: &mut DisplayHandle,
    ) -> Result<Vec<UnixStream>, anyhow::Error> {
        if self.children.is_empty() {
            let (clients_left, sockets_left): (Vec<_>, Vec<_>) = (0..self
                .config
                .plugins_left
                .as_ref()
                .map(|v| v.len())
                .unwrap_or(0))
                .map(|_p| {
                    let (c, s) = get_client_sock(display);
                    (c, s)
                })
                .unzip();
            self.clients_left = clients_left;
            let (clients_center, sockets_center): (Vec<_>, Vec<_>) = (0..self
                .config
                .plugins_center
                .as_ref()
                .map(|v| v.len())
                .unwrap_or(0))
                .map(|_p| {
                    let (c, s) = get_client_sock(display);
                    (c, s)
                })
                .unzip();
            self.clients_center = clients_center;
            let (clients_right, sockets_right): (Vec<_>, Vec<_>) = (0..self
                .config
                .plugins_right
                .as_ref()
                .map(|v| v.len())
                .unwrap_or(0))
                .map(|_p| {
                    let (c, s) = get_client_sock(display);
                    (c, s)
                })
                .unzip();
            self.clients_right = clients_right;

            let mut desktop_ids = self
                .config
                .plugins_left
                .iter()
                .chain(self.config.plugins_center.iter())
                .chain(self.config.plugins_right.iter())
                .flatten()
                .zip(
                    sockets_left
                        .into_iter()
                        .chain(sockets_center.into_iter())
                        .chain(sockets_right.into_iter()),
                )
                .collect_vec();

            // TODO how slow is this? Would it be worth using a faster method of comparing strings?
            self.children = Iter::new(freedesktop_desktop_entry::default_paths())
                .filter_map(|path| {
                    if let Some(position) = desktop_ids.iter().position(|(app_file_name, _)| {
                        Some(OsString::from(app_file_name).as_os_str()) == path.file_stem()
                    }) {
                        // This way each applet is at most started once,
                        // even if multiple desktop files in different directories match
                        let (_, client_socket) = desktop_ids.remove(position);
                        fs::read_to_string(&path).ok().and_then(|bytes| {
                            if let Ok(entry) = DesktopEntry::decode(&path, &bytes) {
                                if let Some(exec) = entry.exec() {
                                    let requests_host_wayland_display =
                                        entry.desktop_entry("HostWaylandDisplay").is_some();
                                    return Some(exec_child(
                                        exec,
                                        Some(self.config.name()),
                                        self.log.as_ref().unwrap().clone(),
                                        client_socket.as_raw_fd(),
                                        requests_host_wayland_display,
                                    ));
                                }
                            }
                            None
                        })
                    } else {
                        None
                    }
                })
                .collect_vec();

            Ok(desktop_ids.into_iter().map(|(_, socket)| socket).collect())
        } else {
            bail!("Clients have already been spawned!");
        }
    }

    fn log(&self) -> Option<Logger> {
        self.log.clone()
    }

    fn destroy(&mut self) {
        self.layer_surface.as_mut().map(|ls| ls.destroy());
        self.layer_shell_wl_surface
            .as_mut()
            .map(|wls| wls.destroy());
    }

    fn space(&mut self) -> &mut Space {
        &mut self.space
    }

    fn popup_manager(&mut self) -> &mut PopupManager {
        &mut self.popup_manager
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
            self.space.commit(&s);
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
                .next_render_event
                .get()
                .map(|e| match e {
                    SpaceEvent::Configure {
                        width,
                        height,
                        serial: _serial,
                        ..
                    } => (width, height),
                    SpaceEvent::WaitConfigure { width, height, .. } => (width, height),
                    _ => self.dimensions.into(),
                })
                .unwrap_or(pending_dimensions.into());
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
        self.space.commit(&s);
        self.space.refresh(&dh);
        if let Some(p) = self
            .popups
            .iter_mut()
            .find(|p| p.s_surface.wl_surface() == s)
        {
            p.dirty = true;
            self.popup_manager.commit(s);
        }
    }

    fn renderer(&mut self) -> Option<&mut Gles2Renderer> {
        self.renderer.as_mut()
    }

    fn keyboard_focus_lost(&mut self) {
        self.close_popups();
    }

    fn setup(
        &mut self,
        env: &Environment<Env>,
        c_display: client::Display,
        log: Logger,
        focused_surface: Rc<RefCell<Option<s_WlSurface>>>,
    ) {
        let layer_shell = env.require_global::<zwlr_layer_shell_v1::ZwlrLayerShellV1>();
        let pool = env
            .create_auto_pool()
            .expect("Failed to create a memory pool!");

        self.log.replace(log);
        self.layer_shell.replace(layer_shell);
        self.pool.replace(pool);
        self.focused_surface = focused_surface;
        self.c_display.replace(c_display);
    }

    fn handle_output(
        &mut self,
        env: &Environment<Env>,
        output: Option<&c_wl_output::WlOutput>,
        output_info: Option<&OutputInfo>,
    ) -> anyhow::Result<()> {
        if let Some(info) = output_info {
            if info.obsolete {
                todo!()
            }
        }
        self.output = output.cloned().zip(output_info.cloned());
        let log = self.log.as_ref().unwrap();
        let c_surface = env.create_surface();
        let dimensions = self.constrain_dim((1, 1).into());
        let layer_surface = self.layer_shell.as_ref().unwrap().get_layer_surface(
            &c_surface,
            output,
            self.config.layer(),
            "".to_owned(),
        );

        layer_surface.set_anchor(self.config.anchor.into());
        layer_surface.set_keyboard_interactivity(self.config.keyboard_interactivity());
        layer_surface.set_size(
            dimensions.w.try_into().unwrap(),
            dimensions.h.try_into().unwrap(),
        );

        // Commit so that the server will send a configure event
        c_surface.commit();

        let next_render_event = Rc::new(Cell::new(Some(SpaceEvent::WaitConfigure {
            first: true,
            width: dimensions.w,
            height: dimensions.h,
        })));

        let next_render_event_handle = next_render_event.clone();
        let logger = log.clone();
        layer_surface.quick_assign(move |layer_surface, event, _| {
            match (event, next_render_event_handle.get()) {
                (zwlr_layer_surface_v1::Event::Closed, _) => {
                    info!(logger, "Received close event. closing.");
                    next_render_event_handle.set(Some(SpaceEvent::Quit));
                }
                (
                    zwlr_layer_surface_v1::Event::Configure {
                        serial,
                        width,
                        height,
                    },
                    next,
                ) if next != Some(SpaceEvent::Quit) => {
                    trace!(
                        logger,
                        "received configure event {:?} {:?} {:?}",
                        serial,
                        width,
                        height
                    );
                    layer_surface.ack_configure(serial);

                    let first = match next {
                        Some(SpaceEvent::Configure { first, .. }) => first,
                        Some(SpaceEvent::WaitConfigure { first, .. }) => first,
                        _ => false,
                    };
                    next_render_event_handle.set(Some(SpaceEvent::Configure {
                        first,
                        width: if width == 0 {
                            dimensions.w
                        } else {
                            width.try_into().unwrap()
                        },
                        height: if height == 0 {
                            dimensions.h
                        } else {
                            height.try_into().unwrap()
                        },
                        serial: serial.try_into().unwrap(),
                    }));
                }
                (_, _) => {}
            }
        });

        self.layer_surface.replace(layer_surface);
        self.dimensions = dimensions;
        self.next_render_event = next_render_event;
        self.full_clear = 4;
        self.layer_shell_wl_surface = Some(c_surface);
        Ok(())
    }
}
