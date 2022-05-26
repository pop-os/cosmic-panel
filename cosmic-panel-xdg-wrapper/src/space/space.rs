// SPDX-License-Identifier: MPL-2.0-only

use std::{
    cell::{Cell, RefCell},
    cmp::Ordering,
    process::Child,
    rc::Rc,
    time::{Duration, Instant},
};

use itertools::Itertools;
use libc::c_int;

use crate::{shared_state::Focus, util::smootherstep};

use super::{ClientEglSurface, Popup, PopupRenderEvent, ServerSurface, TopLevelSurface};
use cosmic_panel_config::config::{self, CosmicPanelConfig};
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
                egl::{GetConfigAttrib, SwapInterval},
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
                xdg_popup,
                xdg_positioner::{self, Anchor, Gravity, XdgPositioner},
                xdg_surface::{self, XdgSurface},
            },
        },
        wayland_server::{
            protocol::wl_surface::WlSurface as s_WlSurface, Client, Display as s_Display,
        },
    },
    utils::{Logical, Point, Rectangle},
    wayland::{shell::xdg::{PopupSurface, PositionerState}, SERIAL_COUNTER},
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Visibility {
    Hidden,
    Visible,
    TransitionToHidden {
        last_instant: Instant,
        progress: Duration,
        prev_margin: i32,
    },
    TransitionToVisible {
        last_instant: Instant,
        progress: Duration,
        prev_margin: i32,
    },
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
    pub config: CosmicPanelConfig,
    pub log: Logger,
    pub needs_update: bool,
    /// indicates whether the surface should be fully cleared and redrawn on the next render
    pub full_clear: bool,
    pub egl_display: EGLDisplay,
    pub renderer: Gles2Renderer,
    pub last_dirty: Instant,
    /// layer surface which all client surfaces are composited onto
    pub layer_surface: Main<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
    pub egl_surface: Rc<EGLSurface>,
    pub next_render_event: Rc<Cell<Option<RenderEvent>>>,
    pub layer_shell_wl_surface: Attached<c_wl_surface::WlSurface>,
    /// adjusts to fit all client surfaces
    pub dimensions: (u32, u32),
    pub pending_dimensions: Option<(u32, u32)>,
    /// focused surface so it can be changed when a window is removed
    focused_surface: Rc<RefCell<Option<s_WlSurface>>>,
    /// visibility state of the panel / panel
    pub visibility: Visibility,
}

impl Space {
    pub(crate) fn new(
        clients_left: &Vec<(u32, Client)>,
        clients_center: &Vec<(u32, Client)>,
        clients_right: &Vec<(u32, Client)>,
        output: c_wl_output::WlOutput,
        output_info: &OutputInfo,
        pool: AutoMemPool,
        config: CosmicPanelConfig,
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
            visibility: Visibility::Visible,
        }
    }

    fn client_top_levels_mut(&mut self) -> impl Iterator<Item = &mut TopLevelSurface> + '_ {
        self.client_top_levels_left
            .iter_mut()
            .chain(self.client_top_levels_center.iter_mut())
            .chain(self.client_top_levels_right.iter_mut())
    }

    fn client_top_levels(&self) -> impl Iterator<Item = &TopLevelSurface> + '_ {
        self.client_top_levels_left
            .iter()
            .chain(self.client_top_levels_center.iter())
            .chain(self.client_top_levels_right.iter())
    }

    fn filter_top_levels(mut s: TopLevelSurface) -> Option<TopLevelSurface> {
        let remove = s.handle_events();
        if remove {
            None
        } else {
            Some(s)
        }
    }

    fn handle_focus(&mut self, focus: &Focus) {
        // always visible if not configured for autohide
        if self.config.autohide.is_none() {
            return;
        }

        match self.visibility {
            Visibility::Hidden => {
                if let Focus::Current(_) = focus {
                    // start transition to visible
                    let margin = match self.config.anchor {
                        config::Anchor::Left | config::Anchor::Right => -(self.dimensions.0 as i32),
                        config::Anchor::Top | config::Anchor::Bottom => -(self.dimensions.1 as i32),
                    } + self.config.get_hide_handle().unwrap() as i32;
                    self.visibility = Visibility::TransitionToVisible {
                        last_instant: Instant::now(),
                        progress: Duration::new(0, 0),
                        prev_margin: margin,
                    }
                }
            }
            Visibility::Visible => {
                if let Focus::LastFocus(t) = focus {
                    // start transition to hidden
                    let duration_since_last_focus = match Instant::now().checked_duration_since(*t)
                    {
                        Some(d) => d,
                        None => return,
                    };
                    if duration_since_last_focus > self.config.get_hide_wait().unwrap() {
                        self.visibility = Visibility::TransitionToHidden {
                            last_instant: Instant::now(),
                            progress: Duration::new(0, 0),
                            prev_margin: 0,
                        }
                    }
                }
            }
            Visibility::TransitionToHidden {
                last_instant,
                progress,
                prev_margin,
            } => {
                let now = Instant::now();
                let total_t = self.config.get_hide_transition().unwrap();
                let delta_t = match now.checked_duration_since(last_instant) {
                    Some(d) => d,
                    None => return,
                };
                let prev_progress = progress;
                let progress = match prev_progress.checked_add(delta_t) {
                    Some(d) => d,
                    None => return,
                };
                let progress_norm =
                    smootherstep(progress.as_millis() as f32 / total_t.as_millis() as f32);
                let handle = self.config.get_hide_handle().unwrap() as i32;

                if let Focus::Current(_) = focus {
                    // start transition to visible
                    self.visibility = Visibility::TransitionToVisible {
                        last_instant: now,
                        progress: total_t.checked_sub(progress).unwrap_or_default(),
                        prev_margin,
                    }
                } else {
                    let panel_size = match self.config.anchor {
                        config::Anchor::Left | config::Anchor::Right => self.dimensions.0 as i32,
                        config::Anchor::Top | config::Anchor::Bottom => self.dimensions.1 as i32,
                    };
                    let target = -panel_size + handle;

                    let cur_pix = (progress_norm * target as f32) as i32;

                    if progress > total_t {
                        // XXX needs testing, but docs say that the margin value is only applied to anchored edge
                        if self.config.exclusive_zone {
                            self.layer_surface.set_exclusive_zone(handle);
                        }
                        self.layer_surface
                            .set_margin(target, target, target, target);
                        self.layer_shell_wl_surface.commit();
                        self.visibility = Visibility::Hidden;
                    } else {
                        if prev_margin != cur_pix {
                            if self.config.exclusive_zone {
                                self.layer_surface.set_exclusive_zone(panel_size - cur_pix);
                            }
                            self.layer_surface
                                .set_margin(cur_pix, cur_pix, cur_pix, cur_pix);
                            self.layer_shell_wl_surface.commit();
                        }
                        self.close_popups();
                        self.visibility = Visibility::TransitionToHidden {
                            last_instant: now,
                            progress,
                            prev_margin: cur_pix,
                        };
                    }
                }
            }
            Visibility::TransitionToVisible {
                last_instant,
                progress,
                prev_margin,
            } => {
                let now = Instant::now();
                let total_t = self.config.get_hide_transition().unwrap();
                let delta_t = match now.checked_duration_since(last_instant) {
                    Some(d) => d,
                    None => return,
                };
                let prev_progress = progress;
                let progress = match prev_progress.checked_add(delta_t) {
                    Some(d) => d,
                    None => return,
                };
                let progress_norm =
                    smootherstep(progress.as_millis() as f32 / total_t.as_millis() as f32);
                let handle = self.config.get_hide_handle().unwrap() as i32;

                if let Focus::LastFocus(_) = focus {
                    // start transition to visible
                    self.close_popups();
                    self.visibility = Visibility::TransitionToHidden {
                        last_instant: now,
                        progress: total_t.checked_sub(progress).unwrap_or_default(),
                        prev_margin,
                    }
                } else {
                    let panel_size = match self.config.anchor {
                        config::Anchor::Left | config::Anchor::Right => self.dimensions.0 as i32,
                        config::Anchor::Top | config::Anchor::Bottom => self.dimensions.1 as i32,
                    };
                    let start = -panel_size + handle;

                    let cur_pix = ((1.0 - progress_norm) * start as f32) as i32;

                    if progress > total_t {
                        // XXX needs thorough testing, but docs say that the margin value is only applied to anchored edge
                        if self.config.exclusive_zone {
                            self.layer_surface.set_exclusive_zone(panel_size);
                        }
                        self.layer_surface.set_margin(0, 0, 0, 0);
                        self.layer_shell_wl_surface.commit();
                        self.visibility = Visibility::Visible;
                    } else {
                        if prev_margin != cur_pix {
                            if self.config.exclusive_zone {
                                self.layer_surface.set_exclusive_zone(panel_size - cur_pix);
                            }
                            self.layer_surface
                                .set_margin(cur_pix, cur_pix, cur_pix, cur_pix);
                            self.layer_shell_wl_surface.commit();
                        }
                        self.visibility = Visibility::TransitionToVisible {
                            last_instant: now,
                            progress,
                            prev_margin: cur_pix,
                        };
                    }
                }
            }
        }
    }

    pub fn handle_events(
        &mut self,
        time: u32,
        children: &mut Vec<Child>,
        focus: &Focus,
    ) -> Instant {
        self.handle_focus(focus);
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
                    self.layer_shell_wl_surface.commit();
                    self.egl_surface.resize(width as i32, height as i32, 0, 0);
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
                    if let Visibility::Hidden = self.visibility {
                        if self.config.exclusive_zone {
                            self.layer_surface
                                .set_exclusive_zone(self.config.get_hide_handle().unwrap() as i32);
                        }
                        let target = match self.config.anchor {
                            config::Anchor::Left | config::Anchor::Right => {
                                -(self.dimensions.0 as i32)
                            }
                            config::Anchor::Top | config::Anchor::Bottom => {
                                -(self.dimensions.1 as i32)
                            }
                        } + self.config.get_hide_handle().unwrap() as i32;
                        self.layer_surface
                            .set_margin(target, target, target, target);
                    } else if self.config.exclusive_zone {
                        let list_thickness = match self.config.anchor {
                            config::Anchor::Left | config::Anchor::Right => width,
                            config::Anchor::Top | config::Anchor::Bottom => height,
                        };
                        self.layer_surface.set_exclusive_zone(list_thickness as i32);
                    }
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
        if self.egl_surface.get_size()
            != Some((self.dimensions.0 as i32, self.dimensions.1 as i32).into())
        {
            self.full_clear = true;
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
            .and_then(|s| s.as_ref().client());
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
                        .and_then(|s| s.as_ref().client());

                    let b_client = b
                        .s_top_level
                        .borrow()
                        .toplevel()
                        .get_surface()
                        .and_then(|s| s.as_ref().client());
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
                        .and_then(|s| s.as_ref().client());

                    let b_client = b
                        .s_top_level
                        .borrow()
                        .toplevel()
                        .get_surface()
                        .and_then(|s| s.as_ref().client());
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
                        .and_then(|s| s.as_ref().client());

                    let b_client = b
                        .s_top_level
                        .borrow()
                        .toplevel()
                        .get_surface()
                        .and_then(|s| s.as_ref().client());
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
        s_surface: PopupSurface,
        parent: s_WlSurface,
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
            ..
        }: PositionerState,
        popup_manager: Rc<RefCell<PopupManager>>,
    ) {
        self.close_popups();

        let s = if let Some(s) = self.client_top_levels_left.iter_mut().find(|s| {
            let top_level: &Window = &s.s_top_level.borrow();
            match top_level.toplevel() {
                Kind::Xdg(wl_s) => wl_s.get_surface() == Some(&parent),
            }
        }) {
            s
        } else if let Some(s) = self.client_top_levels_center.iter_mut().find(|s| {
            let top_level: &Window = &s.s_top_level.borrow();
            match top_level.toplevel() {
                Kind::Xdg(wl_s) => wl_s.get_surface() == Some(&parent),
            }
        }) {
            s
        } else if let Some(s) = self.client_top_levels_right.iter_mut().find(|s| {
            let top_level: &Window = &s.s_top_level.borrow();
            match top_level.toplevel() {
                Kind::Xdg(wl_s) => wl_s.get_surface() == Some(&parent),
            }
        }) {
            s
        } else {
            return;
        };

        positioner.set_size(rect_size.w, rect_size.h);
        positioner.set_anchor_rect(
            anchor_rect.loc.x + s.rectangle.loc.x,
            anchor_rect.loc.y + s.rectangle.loc.y,
            anchor_rect.size.w,
            anchor_rect.size.h,
        );
        positioner.set_anchor(Anchor::from_raw(anchor_edges.to_raw()).unwrap_or(Anchor::None));
        positioner.set_gravity(Gravity::from_raw(gravity.to_raw()).unwrap_or(Gravity::None));

        positioner.set_constraint_adjustment(constraint_adjustment.to_raw());
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
                    if next_render_event_handle.get() != Some(PopupRenderEvent::Closed) {
                        let kind = PopupKind::Xdg(s_popup_surface.clone());

                        let _ = s_popup_surface.send_configure();
                        let _ = popup_manager.borrow_mut().track_popup(kind);
                        next_render_event_handle.set(Some(PopupRenderEvent::Configure {
                            x,
                            y,
                            width,
                            height,
                        }));
                    }
                }
                xdg_popup::Event::PopupDone => {
                    next_render_event_handle.set(Some(PopupRenderEvent::Closed));
                }
                xdg_popup::Event::Repositioned { token } => {
                    next_render_event_handle.set(Some(PopupRenderEvent::Repositioned(token)));
                }
                _ => {}
            };
        });
        let client_egl_surface = ClientEglSurface {
            wl_egl_surface: wayland_egl::WlEglSurface::new(&c_surface, rect_size.w, rect_size.h),
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
            should_render: false,
        });
    }

    pub fn close_popups(&mut self) {
        for top_level in self.client_top_levels_mut() {
            drop(top_level.popups.drain(..));
        }
    }

    pub fn dirty(&mut self, dirty_top_level_surface: &s_WlSurface, (w, h): (u32, u32)) {
        // TODO constrain window size based on max panel sizes
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

        let new_w = w + 2 * self.config.padding;
        let new_h = h + 2 * self.config.padding;

        // TODO improve this for when there are changes to the lists of plugins while running
        let (new_w, new_h) = Self::constrain_dim(
            &self.config,
            (new_w, new_h),
            self.output.1.modes[0].dimensions,
        );
        let pending_dimensions = self.pending_dimensions.unwrap_or(self.dimensions);
        let mut wait_configure_dim = self
            .next_render_event
            .get()
            .map(|e| match e {
                RenderEvent::Configure {
                    width,
                    height,
                    serial: _serial,
                } => (width, height),
                RenderEvent::WaitConfigure { width, height } => (width, height),
                _ => self.dimensions,
            })
            .unwrap_or(pending_dimensions);
        if self.dimensions.0 < new_w && pending_dimensions.0 < new_w && wait_configure_dim.0 < new_w
        {
            self.pending_dimensions = Some((new_w, wait_configure_dim.1));
            wait_configure_dim.0 = new_w;
        }
        if self.dimensions.1 < new_h && pending_dimensions.1 < new_h && wait_configure_dim.1 < new_h
        {
            self.pending_dimensions = Some((wait_configure_dim.0, new_h));
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
            .and_then(|t| t.s_top_level.borrow().toplevel().get_surface().cloned())
        {
            self.focused_surface.borrow_mut().replace(s);
            return;
        }
        self.focused_surface.borrow_mut().take();
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
                                t.rectangle.loc,
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
                        s.rectangle.loc,
                        s.s_top_level.clone(),
                        popup.s_surface.clone(),
                    ));
                }
            }
        }
        None
    }

    pub fn reposition_popup(
        &mut self,
        popup: PopupSurface,
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
            ..
        }: PositionerState,        token: u32,
    ) -> anyhow::Result<()> {
        if let Some((top_level_popup, top_level_rectangle)) = self
            .client_top_levels_mut()
            .find_map(|s| s.popups.iter_mut().find_map(|p| if p.s_surface == popup
            {
                Some((p, s.rectangle))
            } else {
                None
            }))
        {
            if positioner.as_ref().version() >= 3 {

                positioner.set_size(rect_size.w, rect_size.h);
                positioner.set_anchor_rect(
                    anchor_rect.loc.x + top_level_rectangle.loc.x,
                    anchor_rect.loc.y + top_level_rectangle.loc.y,
                    anchor_rect.size.w,
                    anchor_rect.size.h,
                );
        
                positioner.set_anchor(Anchor::from_raw(anchor_edges.to_raw()).unwrap_or(Anchor::None));
                positioner.set_gravity(Gravity::from_raw(gravity.to_raw()).unwrap_or(Gravity::None));

                
                positioner.set_constraint_adjustment(constraint_adjustment.to_raw());
                positioner.set_offset(offset.x, offset.y);
                if reactive {
                    positioner.set_reactive();
                }
                if let Some(parent_size) = parent_size {
                    positioner.set_parent_size(parent_size.w, parent_size.h);
                }
                top_level_popup.c_popup.reposition(&positioner, u32::from(SERIAL_COUNTER.next_serial()));
                Ok(())
            } else {
                top_level_popup.s_surface.send_repositioned(token);
                top_level_popup.s_surface.send_configure()?;
                anyhow::bail!("popup doesn't support repositioning");
            }
        } else {
            anyhow::bail!("failed to find repositioned popup")
        }
    }

    pub fn find_server_window(&self, active_surface: &s_WlSurface) -> Option<ServerSurface> {
        for s in self.client_top_levels() {
            if s.s_top_level.borrow().toplevel().get_surface() == Some(active_surface) {
                return Some(ServerSurface::TopLevel(
                    s.rectangle.loc,
                    s.s_top_level.clone(),
                ));
            } else {
                for popup in &s.popups {
                    if popup.s_surface.get_surface() == Some(active_surface) {
                        return Some(ServerSurface::Popup(
                            s.rectangle.loc,
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
        config: &CosmicPanelConfig,
        (mut w, mut h): (u32, u32),
        (o_w, o_h): (i32, i32),
    ) -> (u32, u32) {
        let (min_w, min_h) = (1.max(config.padding * 2), 1.max(config.padding * 2));
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
        config: &CosmicPanelConfig,
        c_surface: Attached<c_wl_surface::WlSurface>,
        dimensions: (u32, u32),
        output: Option<&c_wl_output::WlOutput>,
        log: Logger,
    ) -> (
        Main<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
        Rc<Cell<Option<RenderEvent>>>,
    ) {
        let layer_surface =
            layer_shell.get_layer_surface(&c_surface, output, config.layer.into(), "".to_owned());

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

        // aggregate damage of all top levels
        // clear once with aggregated damage
        // redraw each top level using the aggregated damage
        let mut l_damage = Vec::new();
        let mut p_damage = Vec::new();
        let mut p_damage_f64 = Vec::new();
        let clear_color = match self.config.background {
            cosmic_panel_config::config::CosmicPanelBackground::ThemeDefault => {
                [0.5, 0.5, 0.5, 0.2]
            }
            cosmic_panel_config::config::CosmicPanelBackground::Color(c) => c,
        };
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
                        for top_level in &mut self
                            .client_top_levels_left
                            .iter_mut()
                            .chain(self.client_top_levels_center.iter_mut())
                            .chain(self.client_top_levels_right.iter_mut())
                            .into_iter()
                            .filter(|t| t.dirty && !t.hidden)
                        {
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
                                if surface_tree_damage.is_empty() || full_clear {
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
                                let mut d = *d;
                                d.loc += *o;
                                d.to_physical(1)
                            })
                            .collect::<Vec<_>>(),
                        l_damage
                            .iter()
                            .map(|(d, o)| {
                                let mut d = *d;
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
                    for top_level in &mut self
                        .client_top_levels_left
                        .iter_mut()
                        .chain(self.client_top_levels_center.iter_mut())
                        .chain(self.client_top_levels_right.iter_mut())
                        .into_iter()
                        .filter(|t| !t.hidden)
                    {
                        // render top level surface
                        let s_top_level = top_level.s_top_level.borrow();
                        let server_surface = match s_top_level.toplevel() {
                            Kind::Xdg(xdg_surface) => match xdg_surface.get_surface() {
                                Some(s) => s,
                                _ => continue,
                            },
                        };

                        if top_level.dirty || !l_damage.is_empty() {
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
                                    .filter_map(|(d, o)| {
                                        let mut d = d;
                                        d.loc += o;
                                        let mut intersect = d.intersection(top_level.rectangle);
                                        if let Some(r) = intersect.as_mut() {
                                            r.loc = (0, 0).into()
                                        };
                                        intersect
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

        if self.client_top_levels().any(|t| t.dirty && !t.hidden) || full_clear {
            self.egl_surface
                .swap_buffers(Some(&mut p_damage))
                .expect("Failed to swap buffers.");
        }
        let clear_color = [0.0, 0.0, 0.0, 0.0];
        // render popups
        for top_level in &mut self
            .client_top_levels_left
            .iter_mut()
            .chain(self.client_top_levels_center.iter_mut())
            .chain(self.client_top_levels_right.iter_mut())
            .into_iter()
            .filter(|t| !t.hidden)
        {
            for p in &mut top_level.popups.iter_mut().filter(|p| p.should_render) {
                p.dirty = false;
                let wl_surface = match p.s_surface.get_surface() {
                    Some(s) => s,
                    _ => continue,
                };
                let pgeo = PopupKind::Xdg(p.s_surface.clone()).geometry();

                let (width, height) = pgeo.size.into();
                let loc = pgeo.loc;

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
                                loc,
                                size: (width, height).into(),
                            };

                            frame
                                .clear(
                                    clear_color,
                                    &[smithay::utils::Rectangle::<f64, smithay::utils::Logical> {
                                        loc: (loc.x as f64, loc.y as f64).into(),
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
                    loc: loc.to_physical(1),
                    size: (width, height).into(),
                }];

                p.egl_surface
                    .swap_buffers(Some(&mut damage))
                    .expect("Failed to swap buffers.");

                send_frames_surface_tree(wl_surface, time);
            }
        }

        for top_level in &mut self
            .client_top_levels_left
            .iter_mut()
            .chain(self.client_top_levels_center.iter_mut())
            .chain(self.client_top_levels_right.iter_mut())
            .into_iter()
            .filter(|t| t.dirty)
        {
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

    fn update_offsets(&mut self) {
        let CosmicPanelConfig {
            padding,
            anchor,
            spacing,
            ..
        } = self.config;
        // First try partitioning the panel evenly into N spaces.
        // If all windows fit into each space, then set their offsets and return.
        let (list_length, list_thickness) = match anchor {
            config::Anchor::Left | config::Anchor::Right => (self.dimensions.1, self.dimensions.0),
            config::Anchor::Top | config::Anchor::Bottom => (self.dimensions.0, self.dimensions.1),
        };

        let mut num_lists = 0;
        if self.config.plugins_left.is_some() {
            num_lists += 1;
        }
        if self.config.plugins_right.is_some() {
            num_lists += 1;
        }
        let mut is_dock = false;
        if self.config.plugins_center.is_some() {
            if num_lists == 0 {
                is_dock = true;
            }
            num_lists += 1;
        }

        for t in self.client_top_levels_mut() {
            t.hidden = false;
        }

        fn map_fn(
            (i, t): (usize, &TopLevelSurface),
            anchor: config::Anchor,
            alignment: Alignment,
        ) -> (Alignment, usize, u32, i32) {
            match anchor {
                config::Anchor::Left | config::Anchor::Right => {
                    (alignment, i, t.priority, t.rectangle.size.h)
                }
                config::Anchor::Top | config::Anchor::Bottom => {
                    (alignment, i, t.priority, t.rectangle.size.w)
                }
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
            // hide lowest priority element from panel
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
        let (right_sum, center_offset) = if is_dock {
            (0, padding as i32)
        } else if left_sum < requested_eq_length
            && center_sum < requested_eq_length
            && right_sum < requested_eq_length
        {
            let center_padding = (requested_eq_length - center_sum) / 2;
            (
                right_sum,
                requested_eq_length + padding as i32 + spacing as i32 + center_padding,
            )
        } else {
            let center_padding = (list_length as i32 - total_sum) / 2;

            (
                right_sum,
                left_sum + padding as i32 + spacing as i32 + center_padding,
            )
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
                config::Anchor::Left | config::Anchor::Right => {
                    let cur = (center_in_bar(list_thickness, size.x as u32), cur);
                    prev += size.y as u32;
                    top_level.rectangle.loc = (cur.0 as i32, cur.1 as i32).into();
                }
                config::Anchor::Top | config::Anchor::Bottom => {
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
                config::Anchor::Left | config::Anchor::Right => {
                    let cur = (center_in_bar(list_thickness, size.x as u32), cur);
                    prev += size.y as u32;
                    top_level.rectangle.loc = (cur.0 as i32, cur.1 as i32).into();
                }
                config::Anchor::Top | config::Anchor::Bottom => {
                    let cur = (cur, center_in_bar(list_thickness, size.y as u32));
                    prev += size.x as u32;
                    top_level.rectangle.loc = (cur.0 as i32, cur.1 as i32).into();
                }
            };
        }

        // twice padding is subtracted
        let mut prev: u32 = list_length - padding - right_sum as u32 - spacing * (self.client_top_levels_right.len().max(1) as u32 - 1);

        for (i, top_level) in &mut self.client_top_levels_right.iter_mut().filter(|t| !t.hidden).enumerate() {
            let size: Point<_, Logical> =
                (top_level.rectangle.size.w, top_level.rectangle.size.h).into();
            let cur = prev + spacing * i as u32;
            match anchor {
                config::Anchor::Left | config::Anchor::Right => {
                    let cur = (center_in_bar(list_thickness, size.x as u32), cur);
                    prev += size.y as u32;
                    top_level.rectangle.loc = (cur.0 as i32, cur.1 as i32).into();
                }
                config::Anchor::Top | config::Anchor::Bottom => {
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
