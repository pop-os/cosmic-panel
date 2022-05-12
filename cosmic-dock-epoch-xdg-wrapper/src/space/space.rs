// SPDX-License-Identifier: MPL-2.0-only

use std::{
    cell::{Cell, RefCell},
    cmp::Ordering,
    process::Child,
    rc::Rc,
    time::Instant,
};

use itertools::Itertools;
use libc::c_int;

use super::{ClientEglSurface, Popup, PopupRenderEvent, ServerSurface, TopLevelSurface};
use cosmic_dock_epoch_config::config::{Anchor, CosmicDockConfig};
use sctk::{
    output::OutputInfo,
    reexports::{
        client::protocol::{wl_output as c_wl_output, wl_surface as c_wl_surface},
        client::{self, Attached, Main},
    },
    shm::AutoMemPool,
};
use slog::{info, trace, warn, Logger};
use smithay::{
    backend::{
        egl::{
            context::{EGLContext, GlAttributes},
            display::EGLDisplay,
            ffi::{
                self,
                egl::{GetConfigAttrib, SwapInterval, WaitClient},
            },
            surface::EGLSurface,
        },
        renderer::{
            gles2::Gles2Renderer, utils::draw_surface_tree, Bind, Frame, ImportEgl, Renderer,
            Unbind,
        },
    },
    desktop::{
        utils::{damage_from_surface_tree, send_frames_surface_tree},
        Kind, PopupKind, PopupManager, Window,
    },
    reexports::{
        wayland_protocols::{
            wlr::unstable::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1},
            xdg_shell::client::{
                xdg_popup::{self, XdgPopup},
                xdg_surface::{self, XdgSurface},
            },
        },
        wayland_server::{
            protocol::wl_surface::WlSurface as s_WlSurface, Client, Display as s_Display,
        },
    },
    utils::{Logical, Point, Rectangle},
    wayland::shell::xdg::PopupSurface,
};

#[derive(PartialEq, Copy, Clone, Debug)]
pub enum RenderEvent {
    WaitConfigure {
        width: u32,
        height: u32,
    },
    Configure {
        width: u32,
        height: u32,
        serial: u32,
    },
    Closed,
}

#[derive(Debug)]
pub struct Space {
    pub clients_left: Vec<(u32, Client)>,
    pub clients_center: Vec<(u32, Client)>,
    pub clients_right: Vec<(u32, Client)>,
    pub client_top_levels_left: Vec<TopLevelSurface>,
    pub client_top_levels_center: Vec<TopLevelSurface>,
    pub client_top_levels_right: Vec<TopLevelSurface>,
    pub pool: AutoMemPool,
    pub layer_shell: Attached<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    pub output: (c_wl_output::WlOutput, OutputInfo),
    pub c_display: client::Display,
    pub config: CosmicDockConfig,
    pub log: Logger,
    pub needs_update: bool,
    /// indicates whether the surface should be fully cleared and redrawn on the next render
    pub full_clear: bool,
    pub egl_display: EGLDisplay,
    pub renderer: Gles2Renderer,
    pub last_dirty: Instant,
    // layer surface which all client surfaces are composited onto
    pub layer_surface: Main<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
    pub egl_surface: Rc<EGLSurface>,
    pub next_render_event: Rc<Cell<Option<RenderEvent>>>,
    pub layer_shell_wl_surface: Attached<c_wl_surface::WlSurface>,
    // adjusts to fit all client surfaces
    pub dimensions: (u32, u32),
    pub pending_dimensions: Option<(u32, u32)>,
    // focused surface so it can be changed when a window is removed
    focused_surface: Rc<RefCell<Option<s_WlSurface>>>,
}

impl Space {
    pub(crate) fn new(
        clients_left: &Vec<(u32, Client)>,
        clients_center: &Vec<(u32, Client)>,
        clients_right: &Vec<(u32, Client)>,
        output: c_wl_output::WlOutput,
        output_info: &OutputInfo,
        pool: AutoMemPool,
        config: CosmicDockConfig,
        c_display: client::Display,
        layer_shell: Attached<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
        log: Logger,
        c_surface: Attached<c_wl_surface::WlSurface>,
        focused_surface: Rc<RefCell<Option<s_WlSurface>>>,
    ) -> Self {
        let dimensions = Self::constrain_dim(&config, (0, 0), output_info.modes[0].dimensions);

        let (w, h) = dimensions;
        let (layer_surface, next_render_event) = Self::get_layer_shell(
            &layer_shell,
            &config,
            c_surface.clone(),
            dimensions,
            Some(&output),
            log.clone(),
        );

        let client_egl_surface = ClientEglSurface {
            wl_egl_surface: wayland_egl::WlEglSurface::new(&c_surface, w as i32, h as i32),
            display: c_display.clone(),
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
            Gles2Renderer::new(egl_context, log.clone()).expect("Failed to initialize EGL Surface")
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

        let next_render_event_handle = next_render_event.clone();
        let logger = log.clone();
        layer_surface.quick_assign(move |layer_surface, event, _| {
            match (event, next_render_event_handle.get()) {
                (zwlr_layer_surface_v1::Event::Closed, _) => {
                    info!(logger, "Received close event. closing.");
                    next_render_event_handle.set(Some(RenderEvent::Closed));
                }
                (
                    zwlr_layer_surface_v1::Event::Configure {
                        serial,
                        width,
                        height,
                    },
                    next,
                ) if next != Some(RenderEvent::Closed) => {
                    trace!(
                        logger,
                        "received configure event {:?} {:?} {:?}",
                        serial,
                        width,
                        height
                    );
                    layer_surface.ack_configure(serial);
                    next_render_event_handle.set(Some(RenderEvent::Configure {
                        width,
                        height,
                        serial,
                    }));
                }
                (_, _) => {}
            }
        });

        Self {
            clients_left: clients_left.clone(),
            clients_center: clients_center.clone(),
            clients_right: clients_right.clone(),
            egl_display,
            renderer,
            client_top_levels_left: Default::default(),
            client_top_levels_center: Default::default(),
            client_top_levels_right: Default::default(),
            layer_shell,
            output: (output, output_info.clone()),
            c_display,
            pool,
            config,
            log,
            needs_update: true,
            full_clear: true,
            last_dirty: Instant::now(),
            dimensions,
            pending_dimensions: None,
            layer_surface,
            egl_surface,
            next_render_event,
            layer_shell_wl_surface: c_surface,
            focused_surface,
        }
    }

    fn client_top_levels_mut(&mut self) -> impl Iterator<Item = &mut TopLevelSurface> + '_ {
        self.client_top_levels_left
            .iter_mut()
            .chain(self.client_top_levels_center.iter_mut())
            .chain(self.client_top_levels_right.iter_mut())
            .into_iter()
    }

    fn client_top_levels(&self) -> impl Iterator<Item = &TopLevelSurface> + '_ {
        self.client_top_levels_left
            .iter()
            .chain(self.client_top_levels_center.iter())
            .chain(self.client_top_levels_right.iter())
            .into_iter()
    }

    fn filter_top_levels(mut s: TopLevelSurface) -> Option<TopLevelSurface> {
        let remove = s.handle_events();
        if remove {
            None
        } else {
            Some(s)
        }
    }

    pub fn handle_events(&mut self, time: u32, children: &mut Vec<Child>) -> Instant {
        let mut should_render = false;
        match self.next_render_event.take() {
            Some(RenderEvent::Closed) => {
                trace!(self.log, "root window removed, exiting...");
                for child in children {
                    let _ = child.kill();
                }
            }
            Some(RenderEvent::Configure {
                width,
                height,
                serial: _serial,
            }) => {
                if self.dimensions != (width, height) && self.pending_dimensions.is_none() {
                    self.dimensions = (width, height);
                    // FIXME sometimes it seems that the egl_surface resize is successful but does not take effect right away
                    self.egl_surface.resize(width as i32, height  as i32, 0, 0);
                    self.needs_update = true;
                    self.full_clear = true;
                    self.update_offsets();
                }
            }
            Some(RenderEvent::WaitConfigure { width, height }) => {
                self.next_render_event
                    .replace(Some(RenderEvent::WaitConfigure { width, height }));
            }
            None => {
                if let Some((width, height)) = self.pending_dimensions.take() {
                    self.layer_surface.set_size(width, height);
                    self.layer_shell_wl_surface.commit();
                    self.next_render_event
                        .replace(Some(RenderEvent::WaitConfigure { width, height }));
                } else {
                    should_render = true;
                }
            }
        }

        // collect and remove windows that aren't needed
        let mut surfaces = self
            .client_top_levels_left
            .drain(..)
            .filter_map(Self::filter_top_levels)
            .collect();
        self.client_top_levels_left.append(&mut surfaces);

        let mut surfaces = self
            .client_top_levels_center
            .drain(..)
            .filter_map(Self::filter_top_levels)
            .collect();
        self.client_top_levels_center.append(&mut surfaces);

        let mut surfaces = self
            .client_top_levels_right
            .drain(..)
            .filter_map(Self::filter_top_levels)
            .collect();
        self.client_top_levels_right.append(&mut surfaces);

        if should_render {
            self.render(time);
        }

        self.last_dirty
    }

    pub fn apply_display(&mut self, s_display: &s_Display) {
        if !self.needs_update {
            return;
        };

        if let Err(_err) = self.renderer.bind_wl_display(s_display) {
            warn!(
                self.log.clone(),
                "Failed to bind display to Egl renderer. Hardware acceleration will not be used."
            );
        }
        self.needs_update = false;
    }

    // TODO: adjust offset of top level
    pub fn add_top_level(&mut self, s_top_level: Rc<RefCell<Window>>) {
        self.full_clear = true;

        let surface_client = s_top_level
            .borrow()
            .toplevel()
            .get_surface()
            .and_then(|s| s.as_ref().client().clone());
        if let Some(surface_client) = surface_client {
            let mut top_level = TopLevelSurface {
                s_top_level,
                popups: Default::default(),
                log: self.log.clone(),
                dirty: true,
                rectangle: Rectangle {
                    loc: (0, 0).into(),
                    size: (0, 0).into(),
                },
                priority: 0,
                hidden: false,
            };
            // determine index position of top level in its list
            if let Some((p, _)) = self.clients_left.iter().find(|(_, c)| *c == surface_client) {
                top_level.set_priority(*p);
                self.client_top_levels_left.push(top_level);
                self.client_top_levels_left.sort_by(|a, b| {
                    let a_client = a
                        .s_top_level
                        .borrow()
                        .toplevel()
                        .get_surface()
                        .and_then(|s| s.as_ref().client().clone());

                    let b_client = b
                        .s_top_level
                        .borrow()
                        .toplevel()
                        .get_surface()
                        .and_then(|s| s.as_ref().client().clone());
                    if let (Some(a_client), Some(b_client)) = (a_client, b_client) {
                        match (
                            self.clients_left.iter().position(|(_, e)| e == &a_client),
                            self.clients_left.iter().position(|(_, e)| e == &b_client),
                        ) {
                            (Some(s_client), Some(t_client)) => s_client.cmp(&t_client),
                            _ => Ordering::Equal,
                        }
                    } else {
                        Ordering::Equal
                    }
                });
            } else if let Some((p, _)) = self
                .clients_center
                .iter()
                .find(|(_, c)| *c == surface_client)
            {
                top_level.set_priority(*p);
                self.client_top_levels_center.push(top_level);
                self.client_top_levels_center.sort_by(|a, b| {
                    let a_client = a
                        .s_top_level
                        .borrow()
                        .toplevel()
                        .get_surface()
                        .and_then(|s| s.as_ref().client().clone());

                    let b_client = b
                        .s_top_level
                        .borrow()
                        .toplevel()
                        .get_surface()
                        .and_then(|s| s.as_ref().client().clone());
                    if let (Some(a_client), Some(b_client)) = (a_client, b_client) {
                        match (
                            self.clients_center.iter().position(|(_, e)| e == &a_client),
                            self.clients_center.iter().position(|(_, e)| e == &b_client),
                        ) {
                            (Some(s_client), Some(t_client)) => s_client.cmp(&t_client),
                            _ => Ordering::Equal,
                        }
                    } else {
                        Ordering::Equal
                    }
                });
            } else if let Some((p, _)) = self
                .clients_right
                .iter()
                .find(|(_, c)| *c == surface_client)
            {
                top_level.set_priority(*p);
                self.client_top_levels_right.push(top_level);
                self.client_top_levels_right.sort_by(|a, b| {
                    let a_client = a
                        .s_top_level
                        .borrow()
                        .toplevel()
                        .get_surface()
                        .and_then(|s| s.as_ref().client().clone());

                    let b_client = b
                        .s_top_level
                        .borrow()
                        .toplevel()
                        .get_surface()
                        .and_then(|s| s.as_ref().client().clone());
                    if let (Some(a_client), Some(b_client)) = (a_client, b_client) {
                        match (
                            self.clients_right.iter().position(|(_, e)| e == &a_client),
                            self.clients_right.iter().position(|(_, e)| e == &b_client),
                        ) {
                            (Some(s_client), Some(t_client)) => s_client.cmp(&t_client),
                            _ => Ordering::Equal,
                        }
                    } else {
                        Ordering::Equal
                    }
                });
            } else {
                return;
            }
        }
    }

    pub fn add_popup(
        &mut self,
        c_surface: c_wl_surface::WlSurface,
        c_xdg_surface: Main<XdgSurface>,
        c_popup: Main<XdgPopup>,
        s_surface: PopupSurface,
        parent: s_WlSurface,
        w: i32,
        h: i32,
        popup_manager: Rc<RefCell<PopupManager>>,
    ) {
        // XXX: closing all popups when adding a new popup will be an issue for nexted popups
        self.close_popups();
        let mut s = match self.client_top_levels_mut().find(|s| {
            let top_level: &Window = &s.s_top_level.borrow();
            let wl_s = match top_level.toplevel() {
                Kind::Xdg(wl_s) => wl_s.get_surface(),
            };
            wl_s == Some(&parent)
        }) {
            Some(s) => s,
            None => return,
        }
        .clone();

        self.layer_surface.get_popup(&c_popup);
        //must be done after role is assigned as popup
        c_surface.commit();
        let next_render_event = Rc::new(Cell::new(Some(PopupRenderEvent::WaitConfigure)));
        c_xdg_surface.quick_assign(move |c_xdg_surface, e, _| {
            if let xdg_surface::Event::Configure { serial, .. } = e {
                c_xdg_surface.ack_configure(serial);
            }
        });

        let next_render_event_handle = next_render_event.clone();
        let s_popup_surface = s_surface.clone();
        c_popup.quick_assign(move |_c_popup, e, _| {
            if let Some(PopupRenderEvent::Closed) = next_render_event_handle.get().as_ref() {
                return;
            }

            match e {
                xdg_popup::Event::Configure {
                    x,
                    y,
                    width,
                    height,
                } => {
                    let kind = PopupKind::Xdg(s_popup_surface.clone());
                    let _ = s_popup_surface.with_pending_state(|popup_state| {
                        popup_state.geometry.loc = (x, y).into();
                        popup_state.geometry.size = (width, height).into();
                    });

                    let _ = s_popup_surface.send_configure();
                    let _ = popup_manager.borrow_mut().track_popup(kind.clone());
                    next_render_event_handle.set(Some(PopupRenderEvent::Configure {
                        x,
                        y,
                        width,
                        height,
                    }));
                }
                xdg_popup::Event::PopupDone => {
                    next_render_event_handle.set(Some(PopupRenderEvent::Closed));
                }
                _ => {}
            };
        });
        let client_egl_surface = ClientEglSurface {
            wl_egl_surface: wayland_egl::WlEglSurface::new(&c_surface, w, h),
            display: self.c_display.clone(),
        };

        let egl_context = self.renderer.egl_context();
        let egl_surface = Rc::new(
            EGLSurface::new(
                &self.egl_display,
                egl_context
                    .pixel_format()
                    .expect("Failed to get pixel format from EGL context "),
                egl_context.config_id(),
                client_egl_surface,
                self.log.clone(),
            )
            .expect("Failed to initialize EGL Surface"),
        );

        s.popups.push(Popup {
            c_popup,
            c_xdg_surface,
            c_surface,
            s_surface,
            egl_surface,
            dirty: false,
            next_render_event,
            bbox: Rectangle::from_loc_and_size((0, 0), (0, 0)),
        });
    }

    pub fn close_popups(&mut self) {
        for top_level in self.client_top_levels_mut() {
            for popup in top_level.popups.drain(..) {
                popup.s_surface.send_popup_done();
            }
        }
    }

    pub fn dirty(&mut self, dirty_top_level_surface: &s_WlSurface, (w, h): (u32, u32)) {
        // TODO constrain window size based on max dock sizes
        // let (w, h) = Self::constrain_dim(&self.config, (w, h), self.output.1.modes[0].dimensions);
        self.last_dirty = Instant::now();
        let mut full_clear = false;

        if let Some(s) = self.client_top_levels_mut().find(|s| {
            let top_level = s.s_top_level.borrow();
            let wl_s = match top_level.toplevel() {
                Kind::Xdg(wl_s) => wl_s.get_surface(),
            };
            wl_s == Some(dirty_top_level_surface)
        }) {
            if s.rectangle.size != (w as i32, h as i32).into() {
                s.rectangle.size = (w as i32, h as i32).into();
                full_clear = true;
            }
            s.dirty = true;
        }

        // TODO improve this for when there are changes to the lists of plugins while running
        let pending_dimensions = self.pending_dimensions.unwrap_or_default();
        let wait_configure_dim = self
            .next_render_event
            .get()
            .map(|e| match e {
                RenderEvent::Configure {
                    width,
                    height,
                    serial,
                } => (width, height),
                RenderEvent::WaitConfigure { width, height } => (width, height),
                _ => (0, 0),
            })
            .unwrap_or_default();
        if self.dimensions.0 < w && pending_dimensions.0 < w && wait_configure_dim.0 < w {
            self.pending_dimensions = Some((w + 2 * self.config.padding, self.dimensions.1));
        }
        if self.dimensions.1 < h && pending_dimensions.1 < h && wait_configure_dim.1 < h {
            self.pending_dimensions = Some((self.dimensions.0, h + 2 * self.config.padding));
        }

        if full_clear {
            self.full_clear = true;
            self.update_offsets();
        }
    }

    pub fn dirty_popup(
        &mut self,
        other_top_level_surface: &s_WlSurface,
        other_popup: PopupSurface,
        dim: Rectangle<i32, Logical>,
    ) {
        self.last_dirty = Instant::now();
        if let Some(s) = self.client_top_levels_mut().find(|s| {
            let top_level = s.s_top_level.borrow();
            let wl_s = match top_level.toplevel() {
                Kind::Xdg(wl_s) => wl_s.get_surface(),
            };
            wl_s == Some(other_top_level_surface)
        }) {
            for popup in &mut s.popups {
                if popup.s_surface.get_surface() == other_popup.get_surface() {
                    if popup.bbox != dim {
                        popup.bbox = dim;
                        popup.egl_surface.resize(dim.size.w, dim.size.h, 0, 0);
                    }
                    popup.dirty = true;
                    break;
                }
            }
        }
    }

    ///  update active window based on pointer location
    pub fn update_pointer(&mut self, (x, y): (i32, i32)) {
        let point = (x, y);
        // set new focused
        if let Some(s) = self
            .client_top_levels()
            .filter(|t| !t.hidden)
            .find(|t| t.rectangle.contains(point))
            .and_then(|t| {
                t.s_top_level
                    .borrow()
                    .toplevel()
                    .get_surface()
                    .map(|s| s.clone())
            })
        {
            self.focused_surface.borrow_mut().replace(s.clone());
        }
    }

    pub fn find_server_surface(
        &self,
        active_surface: &c_wl_surface::WlSurface,
    ) -> Option<ServerSurface> {
        if active_surface == &*self.layer_shell_wl_surface {
            return self.client_top_levels().find_map(|t| {
                t.s_top_level
                    .borrow()
                    .toplevel()
                    .get_surface()
                    .and_then(|s| {
                        if Some(s.clone()) == *self.focused_surface.borrow() {
                            Some(ServerSurface::TopLevel(
                                t.rectangle.loc.clone(),
                                t.s_top_level.clone(),
                            ))
                        } else {
                            None
                        }
                    })
            });
        }

        for s in self.client_top_levels() {
            for popup in &s.popups {
                if popup.c_surface == active_surface.clone() {
                    return Some(ServerSurface::Popup(
                        s.rectangle.loc.clone(),
                        s.s_top_level.clone(),
                        popup.s_surface.clone(),
                    ));
                }
            }
        }
        None
    }

    pub fn find_server_window(&self, active_surface: &s_WlSurface) -> Option<ServerSurface> {
        for s in self.client_top_levels() {
            if s.s_top_level.borrow().toplevel().get_surface() == Some(active_surface) {
                return Some(ServerSurface::TopLevel(
                    s.rectangle.loc.clone(),
                    s.s_top_level.clone(),
                ));
            } else {
                for popup in &s.popups {
                    if popup.s_surface.get_surface() == Some(active_surface) {
                        return Some(ServerSurface::Popup(
                            s.rectangle.loc.clone(),
                            s.s_top_level.clone(),
                            popup.s_surface.clone(),
                        ));
                    }
                }
            }
        }
        None
    }

    fn constrain_dim(
        config: &CosmicDockConfig,
        (mut w, mut h): (u32, u32),
        (o_w, o_h): (i32, i32),
    ) -> (u32, u32) {
        let (min_w, min_h) = (1, 1);
        w = min_w.max(w);
        h = min_h.max(h);
        if let (Some(w_range), _) = config.get_dimensions((o_w as u32, o_h as u32)) {
            if w < w_range.start {
                w = w_range.start;
            } else if w > w_range.end {
                w = w_range.end;
            }
        }
        if let (_, Some(h_range)) = config.get_dimensions((o_w as u32, o_h as u32)) {
            if h < h_range.start {
                h = h_range.start;
            } else if h > h_range.end {
                h = h_range.end;
            }
        }
        (w, h)
    }

    fn get_layer_shell(
        layer_shell: &Attached<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
        config: &CosmicDockConfig,
        c_surface: Attached<c_wl_surface::WlSurface>,
        dimensions: (u32, u32),
        output: Option<&c_wl_output::WlOutput>,
        log: Logger,
    ) -> (
        Main<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
        Rc<Cell<Option<RenderEvent>>>,
    ) {
        let layer_surface = layer_shell.get_layer_surface(
            &c_surface.clone(),
            output,
            config.layer.into(),
            "".to_owned(),
        );

        layer_surface.set_anchor(config.anchor.into());
        layer_surface.set_keyboard_interactivity(config.keyboard_interactivity.into());
        let (x, y) = dimensions;
        layer_surface.set_size(x, y);

        // Commit so that the server will send a configure event
        c_surface.commit();

        let next_render_event = Rc::new(Cell::new(Some(RenderEvent::WaitConfigure {
            width: x,
            height: y,
        })));

        //let egl_surface_clone = egl_surface.clone();
        let next_render_event_handle = next_render_event.clone();
        let logger = log.clone();
        layer_surface.quick_assign(move |layer_surface, event, _| {
            match (event, next_render_event_handle.get()) {
                (zwlr_layer_surface_v1::Event::Closed, _) => {
                    info!(logger, "Received close event. closing.");
                    next_render_event_handle.set(Some(RenderEvent::Closed));
                }
                (
                    zwlr_layer_surface_v1::Event::Configure {
                        serial,
                        width,
                        height,
                    },
                    next,
                ) if next != Some(RenderEvent::Closed) => {
                    trace!(
                        logger,
                        "received configure event {:?} {:?} {:?}",
                        serial,
                        width,
                        height
                    );
                    layer_surface.ack_configure(serial);
                    next_render_event_handle.set(Some(RenderEvent::Configure {
                        width,
                        height,
                        serial,
                    }));
                }
                (_, _) => {}
            }
        });
        (layer_surface, next_render_event)
    }

    // TODO cleanup
    fn render(&mut self, time: u32) {
        let logger = self.log.clone();
        let width = self.dimensions.0 as i32;
        let height = self.dimensions.1 as i32;

        let full_clear = self.full_clear;
        self.full_clear = false;

        // reorder top levels so active window is last
        let mut _top_levels: Vec<_> = self.client_top_levels().map(|t| t.clone()).collect_vec();

        // aggregate damage of all top levels
        // clear once with aggregated damage
        // redraw each top level using the aggregated damage
        let mut l_damage = Vec::new();
        let mut p_damage = Vec::new();
        let mut p_damage_f64 = Vec::new();
        let clear_color = [0.5, 0.5, 0.5, 0.5];
        let _ = self.renderer.unbind();
        self.renderer
            .bind(self.egl_surface.clone())
            .expect("Failed to bind surface to GL");
        self.renderer
            .render(
                (width, height).into(),
                smithay::utils::Transform::Flipped180,
                |self_: &mut Gles2Renderer, frame| {
                    // clear frame with total damage
                    if full_clear {
                        l_damage = vec![(
                            Rectangle::from_loc_and_size(
                                (0, 0),
                                (self.dimensions.0 as i32, self.dimensions.1 as i32),
                            ),
                            (0, 0).into(),
                        )];
                    } else {
                        for top_level in &mut _top_levels.iter().filter(|t| t.dirty && !t.hidden) {
                            let s_top_level = top_level.s_top_level.borrow();
                            let server_surface = match s_top_level.toplevel() {
                                Kind::Xdg(xdg_surface) => match xdg_surface.get_surface() {
                                    Some(s) => s,
                                    _ => continue,
                                },
                            };
                            let mut loc = s_top_level.bbox().loc - top_level.rectangle.loc;
                            loc = (-loc.x, -loc.y).into();
                            // full clear if size changed or if top level added
                            let full_clear = self.full_clear;

                            let surface_tree_damage =
                                damage_from_surface_tree(server_surface, (0, 0), None);
                            l_damage.extend(
                                if surface_tree_damage.len() == 0 || full_clear {
                                    vec![Rectangle::from_loc_and_size(
                                        loc,
                                        (
                                            top_level.rectangle.size.w as i32,
                                            top_level.rectangle.size.h as i32,
                                        ),
                                    )]
                                } else {
                                    surface_tree_damage
                                }
                                .into_iter()
                                .map(|d| (d, top_level.rectangle.loc)),
                            );
                        }
                    }

                    let (mut cur_p_damage, mut cur_p_damage_f64) = (
                        l_damage
                            .iter()
                            .map(|(d, o)| {
                                let mut d = d.clone();
                                d.loc += *o;
                                d.to_physical(1)
                            })
                            .collect::<Vec<_>>(),
                        l_damage
                            .iter()
                            .map(|(d, o)| {
                                let mut d = d.clone();
                                d.loc += *o;
                                d.to_physical(1).to_f64()
                            })
                            .collect::<Vec<_>>(),
                    );
                    p_damage.append(&mut cur_p_damage);
                    p_damage_f64.append(&mut cur_p_damage_f64);
                    frame
                        .clear(clear_color, &p_damage_f64)
                        .expect("Failed to clear frame.");

                    // draw each surface which needs to be drawn
                    for top_level in &mut _top_levels.iter_mut().filter(|t| !t.hidden) {
                        // render top level surface
                        let s_top_level = top_level.s_top_level.borrow();
                        let server_surface = match s_top_level.toplevel() {
                            Kind::Xdg(xdg_surface) => match xdg_surface.get_surface() {
                                Some(s) => s,
                                _ => continue,
                            },
                        };
                        if top_level.dirty || l_damage.len() > 0 {
                            let mut loc = s_top_level.bbox().loc - top_level.rectangle.loc;
                            loc = (-loc.x, -loc.y).into();

                            draw_surface_tree(
                                self_,
                                frame,
                                server_surface,
                                1.0,
                                loc,
                                &l_damage
                                    .clone()
                                    .into_iter()
                                    .map(|(d, o)| {
                                        let mut d = d.clone();
                                        d.loc += o - top_level.rectangle.loc;
                                        d
                                    })
                                    .collect::<Vec<_>>(),
                                &logger,
                            )
                            .expect("Failed to draw surface tree");
                        }
                    }
                },
            )
            .expect("Failed to render to layer shell surface.");

        if _top_levels.iter().find(|t| t.dirty && !t.hidden).is_some() {
            self.egl_surface
                .swap_buffers(Some(&mut p_damage))
                .expect("Failed to swap buffers.");
        }

        // render popups
        for top_level in &mut _top_levels.iter_mut().filter(|t| !t.hidden) {
            for p in &mut top_level
                .popups
                .iter_mut()
                .filter(|p| p.dirty && p.s_surface.alive() && p.next_render_event.get() != None)
            {
                p.dirty = false;
                let wl_surface = match p.s_surface.get_surface() {
                    Some(s) => s,
                    _ => return,
                };

                let (width, height) = p.bbox.size.into();
                let loc = p.bbox.loc + top_level.rectangle.loc;
                let logger = top_level.log.clone();
                let _ = self.renderer.unbind();
                self.renderer
                    .bind(p.egl_surface.clone())
                    .expect("Failed to bind surface to GL");
                self.renderer
                    .render(
                        (width, height).into(),
                        smithay::utils::Transform::Flipped180,
                        |self_: &mut Gles2Renderer, frame| {
                            let damage = smithay::utils::Rectangle::<i32, smithay::utils::Logical> {
                                loc: loc.clone().into(),
                                size: (width, height).into(),
                            };

                            frame
                                .clear(
                                    clear_color,
                                    &[smithay::utils::Rectangle::<f64, smithay::utils::Logical> {
                                        loc: (loc.x as f64, loc.y as f64).clone().into(),
                                        size: (width as f64, height as f64).into(),
                                    }
                                    .to_physical(1.0)],
                                )
                                .expect("Failed to clear frame.");
                            let loc = (-loc.x, -loc.y);
                            draw_surface_tree(
                                self_,
                                frame,
                                wl_surface,
                                1.0,
                                loc.into(),
                                &[damage],
                                &logger,
                            )
                            .expect("Failed to draw surface tree");
                        },
                    )
                    .expect("Failed to render to layer shell surface.");

                let mut damage = [smithay::utils::Rectangle {
                    loc: (0, 0).into(),
                    size: (width, height).into(),
                }];

                p.egl_surface
                    .swap_buffers(Some(&mut damage))
                    .expect("Failed to swap buffers.");

                send_frames_surface_tree(wl_surface, time);
            }
        }

        for top_level in &mut self.client_top_levels_mut().filter(|t| t.dirty) {
            top_level.dirty = false;

            let s_top_level = top_level.s_top_level.borrow();
            let server_surface = match s_top_level.toplevel() {
                Kind::Xdg(xdg_surface) => match xdg_surface.get_surface() {
                    Some(s) => s,
                    _ => continue,
                },
            };
            send_frames_surface_tree(server_surface, time);
        }
    }

    // TODO update top level offsets based on client list and top level list
    // DO THIS NEXT
    fn update_offsets(&mut self) {
        let CosmicDockConfig {
            padding,
            anchor,
            spacing,
            ..
        } = self.config;
        // First try partitioning the dock evenly into N spaces.
        // If all windows fit into each space, then set their offsets and return.
        let (list_length, list_thickness) = match anchor {
            Anchor::Left | Anchor::Right => (self.dimensions.1, self.dimensions.0),
            Anchor::Top | Anchor::Bottom => (self.dimensions.0, self.dimensions.1),
        };

        let mut num_lists = 0;
        if self.client_top_levels_left.len() > 0 {
            num_lists += 1;
        }
        if self.client_top_levels_center.len() > 0 {
            num_lists += 1;
        }
        if self.client_top_levels_right.len() > 0 {
            num_lists += 1;
        }

        for t in self.client_top_levels_mut() {
            t.hidden = false;
        }

        fn map_fn(
            (i, t): (usize, &TopLevelSurface),
            anchor: Anchor,
            alignment: Alignment,
        ) -> (Alignment, usize, u32, i32) {
            match anchor {
                Anchor::Left | Anchor::Right => (alignment, i, t.priority, t.rectangle.size.h),
                Anchor::Top | Anchor::Bottom => (alignment, i, t.priority, t.rectangle.size.w),
            }
        }

        let left = self
            .client_top_levels_left
            .iter()
            .enumerate()
            .map(|e| map_fn(e, anchor, Alignment::Left));
        let mut left_sum = left.clone().map(|(_, _, _, d)| d).sum::<i32>();

        let center = self
            .client_top_levels_center
            .iter()
            .enumerate()
            .map(|e| map_fn(e, anchor, Alignment::Center));
        let mut center_sum = center.clone().map(|(_, _, _, d)| d).sum::<i32>();

        let right = self
            .client_top_levels_right
            .iter()
            .enumerate()
            .map(|e| map_fn(e, anchor, Alignment::Right));
        let mut right_sum = right.clone().map(|(_, _, _, d)| d).sum::<i32>();

        let mut all_sorted_priority = left
            .chain(center)
            .chain(right)
            .sorted_by(|(_, _, p_a, _), (_, _, p_b, _)| Ord::cmp(p_a, p_b).reverse())
            .collect_vec();
        let mut total_sum = left_sum + center_sum + right_sum;
        while total_sum as u32 + padding * 2 + spacing * (num_lists - 1) > list_length {
            // hide lowest priority element from dock
            let (hidden_a, hidden_i, _, hidden_l) = all_sorted_priority.pop().unwrap();
            match hidden_a {
                Alignment::Left => {
                    self.client_top_levels_left[hidden_i].set_hidden(true);
                    left_sum -= hidden_l;
                }
                Alignment::Center => {
                    self.client_top_levels_center[hidden_i].set_hidden(true);
                    center_sum -= hidden_l;
                }
                Alignment::Right => {
                    self.client_top_levels_right[hidden_i].set_hidden(true);
                    right_sum -= hidden_l;
                }
            };
            total_sum -= hidden_l;
        }

        fn center_in_bar(thickness: u32, dim: u32) -> i32 {
            (thickness as i32 - dim as i32) / 2
        }

        let requested_eq_length: i32 = (list_length / num_lists).try_into().unwrap();
        let (right_sum, center_offset,) = if left_sum < requested_eq_length
            && center_sum < requested_eq_length
            && right_sum < requested_eq_length
        {
            let center_padding = (requested_eq_length - center_sum) / 2;
            (right_sum, requested_eq_length + padding as i32 + spacing as i32 + center_padding)
        } else {
            let center_padding = (list_length as i32 - total_sum) / 2;

            (right_sum, left_sum + padding as i32 + spacing as i32 + center_padding)
        };

        let mut prev: u32 = padding;

        for (i, top_level) in &mut self
            .client_top_levels_left
            .iter_mut()
            .filter(|t| !t.hidden)
            .enumerate()
        {
            let size: Point<_, Logical> =
                (top_level.rectangle.size.w, top_level.rectangle.size.h).into();
            let cur = prev + spacing * i as u32;
            match anchor {
                Anchor::Left | Anchor::Right => {
                    let cur = (center_in_bar(list_thickness, size.x as u32), cur);
                    prev += size.y as u32;
                    top_level.rectangle.loc = (cur.0 as i32, cur.1 as i32).into();
                }
                Anchor::Top | Anchor::Bottom => {
                    let cur = (cur, center_in_bar(list_thickness, size.y as u32));
                    prev += size.x as u32;
                    top_level.rectangle.loc = (cur.0 as i32, cur.1 as i32).into();
                }
            };
        }

        let mut prev: u32 = center_offset as u32;

        for (i, top_level) in &mut self
            .client_top_levels_center
            .iter_mut()
            .filter(|t| !t.hidden)
            .enumerate()
        {
            let size: Point<_, Logical> =
                (top_level.rectangle.size.w, top_level.rectangle.size.h).into();
            let cur = prev + spacing * i as u32;
            match anchor {
                Anchor::Left | Anchor::Right => {
                    let cur = (center_in_bar(list_thickness, size.x as u32), cur);
                    prev += size.y as u32;
                    top_level.rectangle.loc = (cur.0 as i32, cur.1 as i32).into();
                }
                Anchor::Top | Anchor::Bottom => {
                    let cur = (cur, center_in_bar(list_thickness, size.y as u32));
                    prev += size.x as u32;
                    top_level.rectangle.loc = (cur.0 as i32, cur.1 as i32).into();
                }
            };
        }

        let mut prev: u32 = list_length - padding - right_sum as u32;

        for (i, top_level) in &mut self.client_top_levels_right.iter_mut().enumerate() {
            let size: Point<_, Logical> =
                (top_level.rectangle.size.w, top_level.rectangle.size.h).into();
            let cur = prev + spacing * i as u32;
            match anchor {
                Anchor::Left | Anchor::Right => {
                    let cur = (center_in_bar(list_thickness, size.x as u32), cur);
                    prev += size.y as u32;
                    top_level.rectangle.loc = (cur.0 as i32, cur.1 as i32).into();
                }
                Anchor::Top | Anchor::Bottom => {
                    let cur = (cur, center_in_bar(list_thickness, size.y as u32));
                    prev += size.x as u32;
                    top_level.rectangle.loc = (cur.0 as i32, cur.1 as i32).into();
                }
            };
        }
    }
}

impl Drop for Space {
    fn drop(&mut self) {
        self.layer_surface.destroy();
        self.layer_shell_wl_surface.destroy();
    }
}

enum Alignment {
    Left,
    Center,
    Right,
}
