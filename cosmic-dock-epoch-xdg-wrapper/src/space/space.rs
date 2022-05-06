// SPDX-License-Identifier: MPL-2.0-only

use std::{
    cell::{Cell, RefCell},
    process::Child,
    rc::Rc,
    time::Instant,
};

use itertools::Itertools;
use libc::c_int;

use sctk::{
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
                xdg_popup::{self, XdgPopup},
                xdg_surface::{self, XdgSurface},
            },
        },
        wayland_server::{protocol::wl_surface::WlSurface as s_WlSurface, Display as s_Display},
    },
    utils::{Logical, Rectangle},
    wayland::shell::xdg::PopupSurface,
};

use cosmic_dock_epoch_config::config::CosmicDockConfig;
use crate::space::RenderEvent;

use super::{
    ActiveState, ClientEglSurface, Popup, PopupRenderEvent, ServerSurface, TopLevelSurface,
};

#[derive(Debug)]
pub struct Space {
    pub cliient_top_levels: Vec<TopLevelSurface>,
    pub pool: AutoMemPool,
    pub layer_shell: Attached<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    pub output: Option<(c_wl_output::WlOutput, String)>,
    pub c_display: client::Display,
    pub config: CosmicDockConfig,
    pub log: Logger,
    pub needs_update: bool,
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
    // focused surface so it can be changed when a window is removed
    focused_surface: Rc<RefCell<Option<s_WlSurface>>>,
}

impl Space {
    pub(crate) fn new(
        output: Option<(c_wl_output::WlOutput, String)>,
        pool: AutoMemPool,
        config: CosmicDockConfig,
        c_display: client::Display,
        layer_shell: Attached<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
        log: Logger,
        c_surface: Attached<c_wl_surface::WlSurface>,
        focused_surface: Rc<RefCell<Option<s_WlSurface>>>,
    ) -> Self {
        let dimensions = Self::constrain_dim(&config, (0, 0));
        let (w, h) = dimensions;
        let (layer_surface, next_render_event) = Self::get_layer_shell(
            &layer_shell,
            &config,
            c_surface.clone(),
            dimensions,
            output.as_ref().map(|(o, _)| o.clone()).as_ref(),
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
                    next_render_event_handle.set(Some(RenderEvent::Configure { width, height }));
                }
                (_, _) => {}
            }
        });

        Self {
            egl_display,
            renderer,
            cliient_top_levels: Default::default(),
            layer_shell,
            output,
            c_display,
            pool,
            config,
            log,
            needs_update: true,
            last_dirty: Instant::now(),
            dimensions,
            layer_surface,
            egl_surface,
            next_render_event,
            layer_shell_wl_surface: c_surface,
            focused_surface,
        }
    }

    pub fn handle_events(&mut self, time: u32, children: &mut Vec<Child>) -> Instant {
        match self.next_render_event.take() {
            Some(RenderEvent::Closed) => {
                trace!(self.log, "root window removed, exiting...");
                for child in children {
                    let _ = child.kill();
                }
            }
            Some(RenderEvent::Configure { width, height }) => {
                if self.dimensions != (width, height) {
                    self.dimensions = (width, height);
                    self.egl_surface.resize(width as i32, height as i32, 0, 0);
                    self.needs_update = true;
                }
            }
            Some(RenderEvent::WaitConfigure) => {
                self.next_render_event
                    .replace(Some(RenderEvent::WaitConfigure));
            }
            None => (),
        }

        // collect and remove windows that aren't needed
        let mut needs_new_active = false;
        let mut surfaces = self
            .cliient_top_levels
            .drain(..)
            .filter_map(|mut s| {
                let remove = s.handle_events();
                if remove {
                    if let ActiveState::ActiveFullyRendered(_) = s.active {
                        s.active = ActiveState::InactiveCleared(false);
                        needs_new_active = true;
                    }
                }
                if remove {
                    None
                } else {
                    Some(s)
                }
            })
            .collect();
        self.cliient_top_levels.append(&mut surfaces);

        if needs_new_active {
            self.cycle_active();
        }

        if self.next_render_event.get() != Some(RenderEvent::WaitConfigure) {
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

    pub fn add_top_level(&mut self, s_top_level: Rc<RefCell<Window>>) {
        for top_level in &mut self.cliient_top_levels {
            top_level.active = ActiveState::InactiveCleared(false);
            top_level.dirty = true;
        }

        let is_root = self.cliient_top_levels.len() == 0;
        let top_level = TopLevelSurface {
            dimensions: (0, 0).into(),
            is_root,
            s_top_level,
            popups: Default::default(),
            log: self.log.clone(),
            dirty: true,
            active: ActiveState::ActiveFullyRendered(false),
            loc_offset: (0, 0).into(),
        };
        self.cliient_top_levels.push(top_level);
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
        let s = match self.cliient_top_levels.iter_mut().find(|s| {
            let top_level: &Window = &s.s_top_level.borrow();
            let wl_s = match top_level.toplevel() {
                Kind::Xdg(wl_s) => wl_s.get_surface(),
            };
            wl_s == Some(&parent)
        }) {
            Some(s) => s,
            None => return,
        };

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

    pub fn dirty(&mut self, dirty_top_level_surface: &s_WlSurface, (w, h): (u32, u32)) {
        self.last_dirty = Instant::now();

        let mut max_w = w;
        let mut max_h = h;
        if let Some((max_old_w, max_old_h)) = self
            .cliient_top_levels
            .iter()
            .filter_map(|s| {
                let top_level = s.s_top_level.borrow();
                let wl_s = match top_level.toplevel() {
                    Kind::Xdg(wl_s) => wl_s.get_surface(),
                };
                if wl_s == Some(dirty_top_level_surface) {
                    None
                } else {
                    Some(s.dimensions)
                }
            })
            .reduce(|accum, s| (s.0.max(accum.0), s.1.max(accum.1)))
        {
            max_w = max_old_w.max(w);
            max_h = max_old_h.max(h);
        }
        if self.dimensions != (max_w, max_h) {
            self.dimensions = (max_w, max_h);
            self.egl_surface.resize(max_w as i32, max_h as i32, 0, 0);
            self.layer_surface.set_size(max_w, max_h);
            self.layer_shell_wl_surface.commit();
        }
        if let Some(s) = self.cliient_top_levels.iter_mut().find(|s| {
            let top_level = s.s_top_level.borrow();
            let wl_s = match top_level.toplevel() {
                Kind::Xdg(wl_s) => wl_s.get_surface(),
            };
            wl_s == Some(dirty_top_level_surface)
        }) {
            if s.dimensions != (w, h) {
                let x_offset = (max_w - w) as i32 / 2;
                let y_offset = (max_h - h) as i32 / 2;

                s.loc_offset = (x_offset as i32, y_offset as i32).into();
                s.dimensions = (w, h);
            }
            s.dirty = true;
        }
    }

    pub fn dirty_popup(
        &mut self,
        other_top_level_surface: &s_WlSurface,
        other_popup: PopupSurface,
        dim: Rectangle<i32, Logical>,
    ) {
        self.last_dirty = Instant::now();
        if let Some(s) = self.cliient_top_levels.iter_mut().find(|s| {
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

    pub fn find_server_surface(
        &self,
        active_surface: &c_wl_surface::WlSurface,
    ) -> Option<ServerSurface> {
        if active_surface == &*self.layer_shell_wl_surface {
            return self.cliient_top_levels.iter().find_map(|s| match s.active {
                ActiveState::ActiveFullyRendered(_) => Some(ServerSurface::TopLevel(
                    s.loc_offset.clone(),
                    s.s_top_level.clone(),
                )),
                _ => None,
            });
        }

        for s in &self.cliient_top_levels {
            for popup in &s.popups {
                if popup.c_surface == active_surface.clone() {
                    return Some(ServerSurface::Popup(
                        s.loc_offset.clone(),
                        s.s_top_level.clone(),
                        popup.s_surface.clone(),
                    ));
                }
            }
        }
        None
    }

    pub fn find_server_window(&self, active_surface: &s_WlSurface) -> Option<ServerSurface> {
        for s in &self.cliient_top_levels {
            if s.s_top_level.borrow().toplevel().get_surface() == Some(active_surface) {
                return Some(ServerSurface::TopLevel(
                    s.loc_offset.clone(),
                    s.s_top_level.clone(),
                ));
            } else {
                for popup in &s.popups {
                    if popup.s_surface.get_surface() == Some(active_surface) {
                        return Some(ServerSurface::Popup(
                            s.loc_offset.clone(),
                            s.s_top_level.clone(),
                            popup.s_surface.clone(),
                        ));
                    }
                }
            }
        }
        None
    }

    fn constrain_dim(config: &CosmicDockConfig, (mut w, mut h): (u32, u32)) -> (u32, u32) {
        let (min_w, min_h) = (1, 1);
        w = min_w.max(w);
        h = min_h.max(h);
        // TODO get monitor dimensions
        if let (Some(max_w), _) = config.get_dimensions() {
            w = max_w.min(w);
        }
        if let (_, Some(max_h)) = config.get_dimensions() {
            h = max_h.min(h);
        }
        (w, h)
    }

    // TODO cleanup & test thouroughly
    pub fn set_output(&mut self, output: Option<(c_wl_output::WlOutput, String)>) {
        self.output = output;
        self.layer_surface.destroy();
        let (layer_surface, next_render_event) = Self::get_layer_shell(
            &self.layer_shell,
            &self.config,
            self.layer_shell_wl_surface.clone(),
            self.dimensions,
            self.output.as_ref().map(|(o, _)| o.clone()).as_ref(),
            self.log.clone(),
        );

        self.next_render_event = next_render_event;
        self.layer_surface = layer_surface;
        self.needs_update = true;
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

        let next_render_event = Rc::new(Cell::new(Some(RenderEvent::WaitConfigure)));

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
                    next_render_event_handle.set(Some(RenderEvent::Configure { width, height }));
                }
                (_, _) => {}
            }
        });
        (layer_surface, next_render_event)
    }

    pub fn cycle_active(&mut self) {
        let cur_active =
            self.cliient_top_levels
                .iter()
                .position(|top_level| match top_level.active {
                    ActiveState::ActiveFullyRendered(_) => true,
                    _ => false,
                });
        if let Some(cur_active) = cur_active {
            let next_active = (cur_active + 1) % self.cliient_top_levels.len();
            for (i, top_level) in &mut self.cliient_top_levels.iter_mut().enumerate() {
                if i == next_active {
                    top_level.active = ActiveState::ActiveFullyRendered(false);
                    let mut focused_surface = self.focused_surface.borrow_mut();
                    *focused_surface = top_level
                        .s_top_level
                        .borrow()
                        .toplevel()
                        .get_surface()
                        .map(|s| s.clone());
                } else {
                    top_level.active = ActiveState::InactiveCleared(false);
                }
                top_level.dirty = true;
            }
        } else if self.cliient_top_levels.len() > 0 {
            let top_level = &mut self.cliient_top_levels[0];
            top_level.active = ActiveState::ActiveFullyRendered(false);
            top_level.dirty = true;
            let mut focused_surface = self.focused_surface.borrow_mut();
            *focused_surface = top_level
                .s_top_level
                .borrow()
                .toplevel()
                .get_surface()
                .map(|s| s.clone());
        }
    }

    // TODO cleanup
    fn render(&mut self, time: u32) {
        let logger = self.log.clone();
        let width = self.dimensions.0 as i32;
        let height = self.dimensions.1 as i32;

        let full_clear = self.cliient_top_levels.iter().map(| top_level | match top_level.active {
            ActiveState::ActiveFullyRendered(b) if b == false => true,
            _ => false,
        }).reduce(|a, b| a || b ).unwrap_or_default();
        // reorder top levels so active window is last
        let mut _top_levels: Vec<&mut TopLevelSurface> = self
            .cliient_top_levels
            .iter_mut()
            .sorted_by(|a, b| Ord::cmp(&b.active, &a.active).reverse())
            .collect();

        // aggregate damage of all top levels
        // clear once with aggregated damage
        // redraw each top level using the aggregated damage
        let mut l_damage = Vec::new();
        let mut p_damage = Vec::new();
        let mut p_damage_f64 = Vec::new();
        let clear_color = [0.0, 0.0, 0.0, 0.0];
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
                        l_damage = vec![(Rectangle::from_loc_and_size((0,0), (self.dimensions.0 as i32, self.dimensions.1 as i32)), (0,0).into())];
                    }
                    else {
                        for top_level in &mut _top_levels.iter().filter(|t| t.dirty) {
                            let s_top_level = top_level.s_top_level.borrow();
                            let server_surface = match s_top_level.toplevel() {
                                Kind::Xdg(xdg_surface) => match xdg_surface.get_surface() {
                                    Some(s) => s,
                                    _ => continue,
                                },
                            };
                            let mut loc = s_top_level.bbox().loc - top_level.loc_offset;
                            loc = (-loc.x, -loc.y).into();
                            let full_clear = match top_level.active {
                                ActiveState::ActiveFullyRendered(b) if b == false => true,
                                ActiveState::InactiveCleared(b) if b == false => false,
                                _ => false,
                            };
    
                            let surface_tree_damage =
                                damage_from_surface_tree(server_surface, (0, 0), None);
                            l_damage.extend( 
                                if surface_tree_damage.len() == 0 || full_clear {
                                    vec![Rectangle::from_loc_and_size(
                                        loc,
                                        (top_level.dimensions.0 as i32, top_level.dimensions.1 as i32),
                                    )]
                                } else {
                                    surface_tree_damage
                                }
                                .into_iter()
                                .map(|d| (d, top_level.loc_offset)),
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
                    for top_level in &mut _top_levels {
                        // render top level surface
                        let s_top_level = top_level.s_top_level.borrow();
                        let server_surface = match s_top_level.toplevel() {
                            Kind::Xdg(xdg_surface) => match xdg_surface.get_surface() {
                                Some(s) => s,
                                _ => continue,
                            },
                        };
                        if top_level.dirty || l_damage.len() > 0 {
                            let mut loc = s_top_level.bbox().loc - top_level.loc_offset;
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
                                        d.loc += o - top_level.loc_offset;
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

        if _top_levels.iter().find(|t| t.dirty).is_some() {
            self.egl_surface
                .swap_buffers(Some(&mut p_damage))
                .expect("Failed to swap buffers.");
        }

        // render popups
        for top_level in &mut _top_levels {
            for p in &mut top_level.popups {
                if !p.dirty || !p.s_surface.alive() || p.next_render_event.get() != None {
                    continue;
                }
                p.dirty = false;
                let wl_surface = match p.s_surface.get_surface() {
                    Some(s) => s,
                    _ => return,
                };

                let (width, height) = p.bbox.size.into();
                let loc = p.bbox.loc + top_level.loc_offset;
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
                            if let ActiveState::ActiveFullyRendered(_) = top_level.active {
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
                            }
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

        for top_level in &mut _top_levels.iter_mut().filter(|t| t.dirty) {
            top_level.dirty = false;

            let s_top_level = top_level.s_top_level.borrow();
            let server_surface = match s_top_level.toplevel() {
                Kind::Xdg(xdg_surface) => match xdg_surface.get_surface() {
                    Some(s) => s,
                    _ => continue,
                },
            };
            send_frames_surface_tree(server_surface, time);

            match top_level.active {
                ActiveState::ActiveFullyRendered(b) if !b => {
                    top_level.active = ActiveState::ActiveFullyRendered(true);
                }
                ActiveState::InactiveCleared(b) if !b => {
                    top_level.active = ActiveState::InactiveCleared(true);
                }
                _ => {}
            }
        }
    }
}

impl Drop for Space {
    fn drop(&mut self) {
        self.layer_surface.destroy();
        self.layer_shell_wl_surface.destroy();
    }
}
