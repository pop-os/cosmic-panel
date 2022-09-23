// SPDX-License-Identifier: MPL-2.0-only

use std::{
    cell::{Cell, RefCell},
    fs::File,
    io::{BufRead, BufReader},
    os::{raw::c_int, unix::net::UnixStream},
    rc::Rc,
    time::{Duration, Instant},
};

use itertools::{chain, Itertools};
use launch_pad::process::Process;
use sctk::{
    output::OutputInfo,
    reexports::client::{
        protocol::{wl_display::WlDisplay, wl_output as c_wl_output},
        Proxy,
    },
    shell::{layer::LayerSurface, xdg::popup},
};
use slog::{info, Logger};
use smithay::{
    backend::{
        egl::{
            context::GlAttributes,
            ffi::{self, egl::GetConfigAttrib},
            EGLContext,
        },
        renderer::{Bind, Frame, Renderer, Unbind},
    },
    desktop::{space::RenderZindex, utils::bbox_from_surface_tree},
    output::Output,
    reexports::wayland_server::{backend::ClientId, DisplayHandle},
};
use smithay::{
    backend::{
        egl::{display::EGLDisplay, surface::EGLSurface},
        renderer::{gles2::Gles2Renderer, utils::draw_surface_tree},
    },
    desktop::{
        draw_window, utils::damage_from_surface_tree, PopupKind, PopupManager, Space, Window,
    },
    reexports::wayland_server::{Client, Resource},
    utils::{Logical, Physical, Point, Rectangle, Size},
};
use tokio::sync::mpsc;
use wayland_egl::WlEglSurface;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer;
use xdg_shell_wrapper::{
    client_state::{ClientFocus, FocusStatus},
    server_state::{ServerFocus, ServerPtrFocus},
    space::{ClientEglSurface, SpaceEvent, Visibility, WrapperPopup},
    util::smootherstep,
};

use cosmic_panel_config::{CosmicPanelBackground, CosmicPanelConfig, PanelAnchor};

use crate::space::Alignment;

pub enum AppletMsg {
    NewProcess(Process),
    ClientSocketPair(String, ClientId, Client, UnixStream),
}
/// space for the cosmic panel
#[derive(Debug)]
pub(crate) struct PanelSpace {
    // XXX implicitly drops egl_surface first to avoid segfault
    pub(crate) egl_surface: Option<Rc<EGLSurface>>,
    pub(crate) c_display: Option<WlDisplay>,

    pub config: CosmicPanelConfig,
    pub log: Logger,
    pub(crate) space: Space,
    pub(crate) clients_left: Vec<(String, Client, UnixStream)>,
    pub(crate) clients_center: Vec<(String, Client, UnixStream)>,
    pub(crate) clients_right: Vec<(String, Client, UnixStream)>,
    pub(crate) last_dirty: Option<Instant>,
    pub(crate) pending_dimensions: Option<Size<i32, Logical>>,
    pub(crate) full_clear: u8,
    pub(crate) space_event: Rc<Cell<Option<SpaceEvent>>>,
    pub(crate) dimensions: Size<i32, Logical>,
    pub(crate) c_focused_surface: Rc<RefCell<ClientFocus>>,
    pub(crate) c_hovered_surface: Rc<RefCell<ClientFocus>>,
    pub(crate) s_focused_surface: ServerFocus,
    pub(crate) s_hovered_surface: ServerPtrFocus,
    pub(crate) visibility: Visibility,
    pub(crate) output: Option<(c_wl_output::WlOutput, Output, OutputInfo)>,
    pub(crate) s_display: Option<DisplayHandle>,
    pub(crate) layer: Option<LayerSurface>,
    pub(crate) popups: Vec<WrapperPopup>,
    pub(crate) w_accumulated_damage: Vec<Vec<Rectangle<i32, Physical>>>,
    pub(crate) start_instant: Instant,
    pub(crate) bg_color: [f32; 4],
    pub applet_tx: mpsc::Sender<AppletMsg>,
}

impl PanelSpace {
    /// create a new space for the cosmic panel
    pub fn new(
        config: CosmicPanelConfig,
        log: Logger,
        c_focused_surface: Rc<RefCell<ClientFocus>>,
        c_hovered_surface: Rc<RefCell<ClientFocus>>,
        applet_tx: mpsc::Sender<AppletMsg>,
    ) -> Self {
        let bg_color = match config.background {
            CosmicPanelBackground::ThemeDefault(alpha) => {
                let path = xdg::BaseDirectories::with_prefix("gtk-4.0")
                    .ok()
                    .and_then(|xdg_dirs| xdg_dirs.find_config_file("cosmic.css"))
                    .unwrap_or_else(|| "~/.config/gtk-4.0/cosmic.css".into());
                let window_bg_color_pattern = "@define-color window_bg_color";
                if let Some(color) = File::open(path).ok().and_then(|file| {
                    BufReader::new(file)
                        .lines()
                        .filter_map(|l| l.ok())
                        .find_map(|line| {
                            line.rfind(window_bg_color_pattern)
                                .and_then(|i| line.get(i + window_bg_color_pattern.len()..))
                                .and_then(|color_str| {
                                    csscolorparser::parse(&color_str.trim().replace(";", "")).ok()
                                })
                        })
                }) {
                    [
                        color.r as f32,
                        color.g as f32,
                        color.b as f32,
                        alpha.unwrap_or(color.a as f32),
                    ]
                } else {
                    [0.0, 0.0, 0.0, alpha.unwrap_or(1.0)]
                }
            }
            CosmicPanelBackground::Color(c) => c,
        };

        Self {
            config,
            space: Space::new(log.clone()),
            log,
            full_clear: 0,
            clients_left: Default::default(),
            clients_center: Default::default(),
            clients_right: Default::default(),
            last_dirty: Default::default(),
            pending_dimensions: Default::default(),
            space_event: Default::default(),
            dimensions: Default::default(),
            output: Default::default(),
            s_display: Default::default(),
            c_display: Default::default(),
            layer: Default::default(),
            egl_surface: Default::default(),
            popups: Default::default(),
            w_accumulated_damage: Default::default(),
            visibility: Visibility::Visible,
            start_instant: Instant::now(),
            c_focused_surface,
            c_hovered_surface,
            s_focused_surface: Default::default(),
            s_hovered_surface: Default::default(),
            bg_color,
            applet_tx,
        }
    }

    pub(crate) fn z_index(&self) -> Option<RenderZindex> {
        match self.config.layer() {
            Layer::Background => Some(RenderZindex::Background),
            Layer::Bottom => Some(RenderZindex::Bottom),
            Layer::Top => Some(RenderZindex::Top),
            Layer::Overlay => Some(RenderZindex::Overlay),
            _ => None,
        }
    }

    pub(crate) fn close_popups(&mut self) {
        for w in &mut self.space.windows() {
            for (PopupKind::Xdg(p), _) in
                PopupManager::popups_for_surface(w.toplevel().wl_surface())
            {
                if !self
                    .s_hovered_surface
                    .iter()
                    .any(|hs| &hs.surface == w.toplevel().wl_surface())
                {
                    p.send_popup_done();
                }
            }
        }
    }

    pub(crate) fn handle_focus(&mut self) {
        let (layer_surface, layer_shell_wl_surface) =
            if let Some(layer_surface) = self.layer.as_ref() {
                (layer_surface, layer_surface.wl_surface())
            } else {
                return;
            };
        let cur_focus = {
            let c_focused_surface = self.c_focused_surface.borrow();
            let c_hovered_surface = self.c_hovered_surface.borrow();
            // always visible if not configured for autohide
            if self.config.autohide().is_none() {
                return;
            }

            c_focused_surface
                .iter()
                .chain(c_hovered_surface.iter())
                .fold(
                    FocusStatus::LastFocused(self.start_instant),
                    |acc, (surface, _, f)| {
                        if self
                            .layer
                            .as_ref()
                            .map(|s| *s.wl_surface() == *surface)
                            .unwrap_or(false)
                            || self.popups.iter().any(|p| {
                                &p.c_wl_surface == surface
                                    || self.popups.iter().any(|p| p.c_wl_surface == *surface)
                            })
                        {
                            match (&acc, &f) {
                                (
                                    FocusStatus::LastFocused(t_acc),
                                    FocusStatus::LastFocused(t_cur),
                                ) => {
                                    if t_cur > t_acc {
                                        *f
                                    } else {
                                        acc
                                    }
                                }
                                (FocusStatus::LastFocused(_), FocusStatus::Focused) => *f,
                                _ => acc,
                            }
                        } else {
                            acc
                        }
                    },
                )
        };
        match self.visibility {
            Visibility::Hidden => {
                if let FocusStatus::Focused = cur_focus {
                    // start transition to visible
                    let margin = match self.config.anchor() {
                        PanelAnchor::Left | PanelAnchor::Right => -(self.dimensions.w),
                        PanelAnchor::Top | PanelAnchor::Bottom => -(self.dimensions.h),
                    } + self.config.get_hide_handle().unwrap() as i32;
                    self.visibility = Visibility::TransitionToVisible {
                        last_instant: Instant::now(),
                        progress: Duration::new(0, 0),
                        prev_margin: margin,
                    }
                }
            }
            Visibility::Visible => {
                if let FocusStatus::LastFocused(t) = cur_focus {
                    // start transition to hidden
                    let duration_since_last_focus = match Instant::now().checked_duration_since(t) {
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

                if let FocusStatus::Focused = cur_focus {
                    // start transition to visible
                    self.visibility = Visibility::TransitionToVisible {
                        last_instant: now,
                        progress: total_t.checked_sub(progress).unwrap_or_default(),
                        prev_margin,
                    }
                } else {
                    let panel_size = match self.config.anchor() {
                        PanelAnchor::Left | PanelAnchor::Right => self.dimensions.w,
                        PanelAnchor::Top | PanelAnchor::Bottom => self.dimensions.h,
                    };
                    let target = -panel_size + handle;

                    let cur_pix = (progress_norm * target as f32) as i32;

                    if progress > total_t {
                        // XXX needs testing, but docs say that the margin value is only applied to anchored edge
                        if self.config.exclusive_zone() {
                            layer_surface.set_exclusive_zone(handle);
                        }
                        layer_surface.set_margin(target, target, target, target);
                        layer_shell_wl_surface.commit();
                        self.visibility = Visibility::Hidden;
                    } else {
                        if prev_margin != cur_pix {
                            if self.config.exclusive_zone() {
                                layer_surface.set_exclusive_zone(panel_size - cur_pix);
                            }
                            layer_surface.set_margin(cur_pix, cur_pix, cur_pix, cur_pix);
                            layer_shell_wl_surface.commit();
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

                if let FocusStatus::LastFocused(_) = cur_focus {
                    // start transition to visible
                    self.close_popups();
                    self.visibility = Visibility::TransitionToHidden {
                        last_instant: now,
                        progress: total_t.checked_sub(progress).unwrap_or_default(),
                        prev_margin,
                    }
                } else {
                    let panel_size = match self.config.anchor() {
                        PanelAnchor::Left | PanelAnchor::Right => self.dimensions.w,
                        PanelAnchor::Top | PanelAnchor::Bottom => self.dimensions.h,
                    };
                    let start = -panel_size + handle;

                    let cur_pix = ((1.0 - progress_norm) * start as f32) as i32;

                    if progress > total_t {
                        // XXX needs thorough testing, but docs say that the margin value is only applied to anchored edge
                        if self.config.exclusive_zone() {
                            layer_surface.set_exclusive_zone(panel_size);
                        }
                        layer_surface.set_margin(0, 0, 0, 0);
                        layer_shell_wl_surface.commit();
                        self.visibility = Visibility::Visible;
                    } else {
                        if prev_margin != cur_pix {
                            if self.config.exclusive_zone() {
                                layer_surface.set_exclusive_zone(panel_size - cur_pix);
                            }
                            layer_surface.set_margin(cur_pix, cur_pix, cur_pix, cur_pix);
                            layer_shell_wl_surface.commit();
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

    pub(crate) fn constrain_dim(&self, size: Size<i32, Logical>) -> Size<i32, Logical> {
        let mut w = size.w.try_into().unwrap();
        let mut h = size.h.try_into().unwrap();

        let output_dims = self
            .output
            .as_ref()
            .map(|(_, _, info)| info.modes[0].dimensions);
        let (min_w, min_h) = (
            1.max(self.config.padding() * 2),
            1.max(self.config.padding() * 2),
        );
        w = min_w.max(w);
        h = min_h.max(h);
        if let Some((o_w, o_h)) = output_dims {
            if let (Some(w_range), _) = self.config.get_dimensions((o_w as u32, o_h as u32)) {
                if w < w_range.start {
                    w = w_range.start;
                } else if w > w_range.end - 1 {
                    w = w_range.end;
                }
            }
            if let (_, Some(h_range)) = self.config.get_dimensions((o_w as u32, o_h as u32)) {
                if h < h_range.start {
                    h = h_range.start;
                } else if h > h_range.end - 1 {
                    h = h_range.end;
                }
            }
        }
        (w.try_into().unwrap(), h.try_into().unwrap()).into()
    }

    pub(crate) fn render(&mut self, renderer: &mut Gles2Renderer, time: u32) -> anyhow::Result<()> {
        if self.space_event.get() != None {
            return Ok(());
        }

        let clear_color = self.bg_color;
        let _ = renderer.unbind();
        renderer.bind(self.egl_surface.as_ref().unwrap().clone())?;

        let log_clone = self.log.clone();
        if let Some((o, _info)) = &self.output.as_ref().map(|(_, o, info)| (o, info)) {
            // TODO handle fractional scaling?
            // let output_scale = o.current_scale().fractional_scale();
            // We explicitly use ceil for the output geometry size to make sure the damage
            // spans at least the output size. Round and floor would result in parts not drawn as the
            // frame size could be bigger than the maximum the output_geo would define.

            let cur_damage = if self.full_clear > 0 {
                None
            } else {
                let mut acc_damage = self.space.windows().fold(vec![], |acc, w| {
                    let w_loc = self
                        .space
                        .window_location(w)
                        .unwrap_or_else(|| (0, 0).into());
                    let mut bbox = w.bbox();
                    bbox.loc += w_loc;

                    acc.into_iter()
                        .chain(w.accumulated_damage(
                            w_loc.to_f64().to_physical(1.0),
                            1.0,
                            Some((&self.space, o)),
                        ))
                        .collect_vec()
                });
                acc_damage.dedup();
                acc_damage.retain(|rect| rect.size.h > 0 && rect.size.w > 0);
                // merge overlapping rectangles
                acc_damage = acc_damage
                    .into_iter()
                    .fold(Vec::new(), |new_damage, mut rect| {
                        // replace with drain_filter, when that becomes stable to reuse the original Vec's memory
                        let (overlapping, mut new_damage): (Vec<_>, Vec<_>) = new_damage
                            .into_iter()
                            .partition(|other| other.overlaps(rect));

                        for overlap in overlapping {
                            rect = rect.merge(overlap);
                        }
                        new_damage.push(rect);
                        new_damage
                    });
                Some(acc_damage)
            };
            let should_render = cur_damage.as_ref().map(|d| !d.is_empty()).unwrap_or(true);

            if should_render {
                let damage = Self::damage_for_buffer(
                    cur_damage,
                    &mut self.w_accumulated_damage,
                    self.egl_surface.as_ref().unwrap(),
                    self.full_clear,
                );
                let damage = damage.unwrap_or_default();
                renderer
                    .render(
                        self.dimensions.to_physical(1),
                        smithay::utils::Transform::Flipped180,
                        |renderer: &mut Gles2Renderer, frame| {
                            if damage.is_empty() {
                                frame
                                    .clear(
                                        clear_color,
                                        &[Rectangle::from_loc_and_size(
                                            (0, 0),
                                            self.dimensions.to_physical(1),
                                        )],
                                    )
                                    .expect("Failed to clear frame.");
                            } else {
                                frame
                                    .clear(
                                        clear_color,
                                        damage.iter().cloned().collect_vec().as_slice(),
                                    )
                                    .expect("Failed to clear frame.");
                            }
                            for w in self.space.windows() {
                                let w_loc = self
                                    .space
                                    .window_location(w)
                                    .unwrap_or_else(|| (0, 0).into());
                                let mut bbox = w.bbox();
                                bbox.loc += w_loc;
                                let w_damage = if damage.is_empty() {
                                    vec![bbox.to_physical(1)]
                                } else {
                                    let mut w_damage = damage
                                        .iter()
                                        .filter_map(|r| r.intersection(bbox.to_physical(1)))
                                        .collect_vec();
                                    w_damage.dedup();
                                    w_damage
                                };

                                if w_damage.is_empty() {
                                    continue;
                                }

                                let _ = draw_window(
                                    renderer,
                                    frame,
                                    w,
                                    1.0,
                                    w_loc.to_physical(1).to_f64(),
                                    w_damage.as_slice(),
                                    &log_clone,
                                );
                            }
                        },
                    )
                    .expect("render error...");

                let mut damage = damage
                    .iter()
                    .map(|rect| {
                        Rectangle::from_loc_and_size(
                            (rect.loc.x, self.dimensions.h - rect.loc.y - rect.size.h),
                            rect.size,
                        )
                    })
                    .collect::<Vec<_>>();

                self.egl_surface
                    .as_ref()
                    .unwrap()
                    .swap_buffers(if damage.is_empty() {
                        None
                    } else {
                        // FIXME: swapping buffers in nvidia seems to be broken
                        None
                        // Some(&mut damage)
                    })?;
            }

            // Popup rendering
            let clear_color = [0.0, 0.0, 0.0, 0.0];
            for p in self
                .popups
                .iter_mut()
                .filter(|p| p.dirty && p.state.is_none())
            {
                let _ = renderer.unbind();
                renderer.bind(p.egl_surface.as_ref().unwrap().clone())?;
                let p_bbox = bbox_from_surface_tree(p.s_surface.wl_surface(), (0, 0));
                let cur_damage = if p.full_clear > 0 {
                    None
                } else {
                    Some(damage_from_surface_tree(
                        p.s_surface.wl_surface(),
                        (0.0, 0.0),
                        1.0,
                        Some((&self.space, o)),
                    ))
                };

                if cur_damage.as_ref().map(|d| d.is_empty()).unwrap_or(false) {
                    continue;
                }

                let damage = match Self::damage_for_buffer(
                    cur_damage,
                    &mut p.accumulated_damage,
                    &p.egl_surface.as_ref().unwrap().clone(),
                    p.full_clear,
                ) {
                    None => vec![],
                    Some(d) if d.is_empty() => continue,
                    Some(d) => d,
                };

                let _ = renderer.render(
                    p_bbox.size.to_physical(1),
                    smithay::utils::Transform::Flipped180,
                    |renderer: &mut Gles2Renderer, frame| {
                        let p_damage = if damage.is_empty() {
                            let mut d = p_bbox.to_physical(1);
                            d.loc = (0, 0).into();
                            vec![d]
                        } else {
                            damage.clone()
                        };

                        frame
                            .clear(
                                clear_color,
                                p_damage.iter().cloned().collect_vec().as_slice(),
                            )
                            .expect("Failed to clear frame.");

                        let _ = draw_surface_tree(
                            renderer,
                            frame,
                            p.s_surface.wl_surface(),
                            1.0,
                            p_bbox.loc.to_f64().to_physical(1.0),
                            &p_damage,
                            &log_clone,
                        );
                    },
                );
                let mut damage = damage
                    .iter()
                    .map(|rect| {
                        Rectangle::from_loc_and_size(
                            (rect.loc.x, p_bbox.size.h - rect.loc.y - rect.size.h),
                            rect.size,
                        )
                    })
                    .collect::<Vec<_>>();
                p.egl_surface
                    .as_ref()
                    .unwrap()
                    .swap_buffers(if damage.is_empty() {
                        None
                    } else {
                        None
                        // Some(&mut damage)
                    })?;
                p.dirty = false;
                p.full_clear = p.full_clear.checked_sub(1).unwrap_or_default();
            }
        }

        let _ = renderer.unbind();
        self.space.send_frames(time);
        self.full_clear = self.full_clear.checked_sub(1).unwrap_or_default();
        Ok(())
    }

    pub(crate) fn damage_for_buffer(
        cur_damage: Option<Vec<Rectangle<i32, Physical>>>,
        acc_damage: &mut Vec<Vec<Rectangle<i32, Physical>>>,
        egl_surface: &Rc<EGLSurface>,
        full_clear: u8,
    ) -> Option<Vec<Rectangle<i32, Physical>>> {
        let mut age: usize = egl_surface
            .buffer_age()
            .unwrap()
            .try_into()
            .unwrap_or_default();

        // reset accumulated damage when applying full clear for the first time
        if full_clear == 4 {
            acc_damage.drain(..);
        }
        let cur_damage = cur_damage.unwrap_or_default();

        let dmg_counts = acc_damage.len();
        // buffer contents undefined, treat as a full clear
        let ret = if age == 0 {
            acc_damage.drain(..);
            None
            // buffer older than we keep track of, full clear, but don't reset accumulated damage, instead add to acc damage
        } else if age >= dmg_counts {
            acc_damage.push(cur_damage);
            None
            // use get the accumulated damage for the last [age] renders, and add to acc damage
        } else {
            acc_damage.push(cur_damage);
            age += 1;
            let mut d = acc_damage.clone();
            d.reverse();
            let d = d[..age + 1]
                .iter()
                .flat_map(|v| v.iter().cloned())
                .collect_vec();
            Some(d)
        };

        // acc damage should only ever be length 4
        if acc_damage.len() > 4 {
            acc_damage.drain(..acc_damage.len() - 4);
        }

        ret
    }

    pub(crate) fn update_window_locations(&mut self) -> anyhow::Result<()> {
        let padding = self.config.padding();
        let anchor = self.config.anchor();
        let spacing = self.config.spacing();
        // First try partitioning the panel evenly into N spaces.
        // If all windows fit into each space, then set their offsets and return.
        let (list_length, list_thickness) = match anchor {
            PanelAnchor::Left | PanelAnchor::Right => (self.dimensions.h, self.dimensions.w),
            PanelAnchor::Top | PanelAnchor::Bottom => (self.dimensions.w, self.dimensions.h),
        };

        let mut num_lists = 0;
        if self.config.plugins_wings.is_some() {
            num_lists += 2;
        }
        let mut is_dock = false;
        if self.config.plugins_center.is_some() {
            if num_lists == 0 {
                is_dock = true;
            }
            num_lists += 1;
        }

        let mut windows_right = self
            .space
            .windows()
            .cloned()
            .filter_map(|w| {
                self.clients_right
                    .iter()
                    .enumerate()
                    .find_map(|(i, (_, c, _))| {
                        if Some(c.id()) == w.toplevel().wl_surface().client_id() {
                            Some((i, w.clone()))
                        } else {
                            None
                        }
                    })
            })
            .collect_vec();
        windows_right.sort_by(|(a_i, _), (b_i, _)| a_i.cmp(b_i));

        let mut windows_center = self
            .space
            .windows()
            .cloned()
            .filter_map(|w| {
                self.clients_center
                    .iter()
                    .enumerate()
                    .find_map(|(i, (_, c, _))| {
                        if Some(c.id()) == w.toplevel().wl_surface().client_id() {
                            Some((i, w.clone()))
                        } else {
                            None
                        }
                    })
            })
            .collect_vec();
        windows_center.sort_by(|(a_i, _), (b_i, _)| a_i.cmp(b_i));

        let mut windows_left = self
            .space
            .windows()
            .cloned()
            .filter_map(|w| {
                self.clients_left
                    .iter()
                    .enumerate()
                    .find_map(|(i, (_, c, _))| {
                        if Some(c.id()) == w.toplevel().wl_surface().client_id() {
                            Some((i, w.clone()))
                        } else {
                            None
                        }
                    })
            })
            .collect_vec();
        windows_left.sort_by(|(a_i, _), (b_i, _)| a_i.cmp(b_i));

        fn map_fn(
            (i, w): &(usize, Window),
            anchor: PanelAnchor,
            alignment: Alignment,
        ) -> (Alignment, usize, i32, i32) {
            match anchor {
                PanelAnchor::Left | PanelAnchor::Right => {
                    (alignment, *i, w.bbox().size.h, w.bbox().size.w)
                }
                PanelAnchor::Top | PanelAnchor::Bottom => {
                    (alignment, *i, w.bbox().size.w, w.bbox().size.h)
                }
            }
        }

        let left = windows_left
            .iter()
            .map(|e| map_fn(e, anchor, Alignment::Left));
        let left_sum = left.clone().map(|(_, _, length, _)| length).sum::<i32>()
            + spacing as i32 * (windows_left.len().max(1) as i32 - 1);

        let center = windows_center
            .iter()
            .map(|e| map_fn(e, anchor, Alignment::Center));
        let center_sum = center.clone().map(|(_, _, length, _)| length).sum::<i32>()
            + spacing as i32 * (windows_center.len().max(1) as i32 - 1);

        let right = windows_right
            .iter()
            .map(|e| map_fn(e, anchor, Alignment::Right));

        let right_sum = right.clone().map(|(_, _, length, _)| length).sum::<i32>()
            + spacing as i32 * (windows_right.len().max(1) as i32 - 1);

        // TODO should the center area in the panel be scrollable? and if there are too many on the sides the rightmost are moved to the center?
        let total_sum = left_sum + center_sum + right_sum;
        let new_list_length =
            total_sum + padding as i32 * 2 + spacing as i32 * (num_lists as i32 - 1);
        let new_list_thickness: i32 = 2 * padding as i32
            + chain!(left.clone(), center.clone(), right.clone())
                .map(|(_, _, _, thickness)| thickness)
                .max()
                .unwrap_or(0);
        let new_dim = self.constrain_dim(
            match anchor {
                PanelAnchor::Left | PanelAnchor::Right => (new_list_thickness, new_list_length),
                PanelAnchor::Top | PanelAnchor::Bottom => (new_list_length, new_list_thickness),
            }
            .into(),
        );
        let (new_list_length, new_list_thickness) = match anchor {
            PanelAnchor::Left | PanelAnchor::Right => (new_dim.h, new_dim.w),
            PanelAnchor::Top | PanelAnchor::Bottom => (new_dim.w, new_dim.h),
        };

        if new_list_length != list_length as i32 || new_list_thickness != list_thickness {
            self.pending_dimensions = Some(new_dim);
            self.full_clear = 4;
            anyhow::bail!("resizing list");
        }

        fn center_in_bar(thickness: u32, dim: u32) -> i32 {
            (thickness as i32 - dim as i32) / 2
        }

        let requested_eq_length: i32 = list_length / num_lists;
        let (right_sum, center_offset) = if is_dock {
            (0, padding as i32 + (list_length - center_sum) / 2)
        } else if left_sum <= requested_eq_length
            && center_sum <= requested_eq_length
            && right_sum <= requested_eq_length
        {
            let center_padding = (requested_eq_length - center_sum) / 2;
            (
                right_sum,
                requested_eq_length + padding as i32 + center_padding,
            )
        } else {
            let center_padding = (list_length as i32 - total_sum) / 2;

            (right_sum, left_sum + padding as i32 + center_padding)
        };

        let mut prev: u32 = padding;

        let z = self.z_index().map(|z| z as u8);
        for (i, w) in &mut windows_left.iter_mut() {
            let size: Point<_, Logical> = (w.bbox().size.w, w.bbox().size.h).into();
            let cur: u32 = prev + spacing * *i as u32;
            match anchor {
                PanelAnchor::Left | PanelAnchor::Right => {
                    let cur = (
                        center_in_bar(list_thickness.try_into().unwrap(), size.x as u32),
                        cur,
                    );
                    prev += size.y as u32;
                    self.space
                        .map_window(w, (cur.0 as i32, cur.1 as i32), z, false);
                }
                PanelAnchor::Top | PanelAnchor::Bottom => {
                    let cur = (
                        cur,
                        center_in_bar(list_thickness.try_into().unwrap(), size.y as u32),
                    );
                    prev += size.x as u32;
                    self.space
                        .map_window(w, (cur.0 as i32, cur.1 as i32), z, false);
                }
            };
            self.space.commit(w.toplevel().wl_surface());
        }

        let mut prev: u32 = center_offset as u32;
        for (i, w) in &mut windows_center.iter_mut() {
            let size: Point<_, Logical> = (w.bbox().size.w, w.bbox().size.h).into();
            let cur = prev + spacing * *i as u32;
            match anchor {
                PanelAnchor::Left | PanelAnchor::Right => {
                    let cur = (
                        center_in_bar(list_thickness.try_into().unwrap(), size.x as u32),
                        cur,
                    );
                    prev += size.y as u32;
                    self.space
                        .map_window(w, (cur.0 as i32, cur.1 as i32), z, false);
                }
                PanelAnchor::Top | PanelAnchor::Bottom => {
                    let cur = (
                        cur,
                        center_in_bar(list_thickness.try_into().unwrap(), size.y as u32),
                    );
                    prev += size.x as u32;
                    self.space
                        .map_window(w, (cur.0 as i32, cur.1 as i32), z, false);
                }
            };
            self.space.commit(w.toplevel().wl_surface());
        }

        // twice padding is subtracted
        let mut prev: u32 = list_length as u32 - padding - right_sum as u32;

        for (i, w) in &mut windows_right.iter_mut() {
            let size: Point<_, Logical> = (w.bbox().size.w, w.bbox().size.h).into();
            let cur = prev + spacing * *i as u32;
            match anchor {
                PanelAnchor::Left | PanelAnchor::Right => {
                    let cur = (
                        center_in_bar(list_thickness.try_into().unwrap(), size.x as u32),
                        cur,
                    );
                    prev += size.y as u32;
                    self.space
                        .map_window(w, (cur.0 as i32, cur.1 as i32), z, false);
                }
                PanelAnchor::Top | PanelAnchor::Bottom => {
                    let cur = (
                        cur,
                        center_in_bar(list_thickness.try_into().unwrap(), size.y as u32),
                    );
                    prev += size.x as u32;
                    self.space
                        .map_window(w, (cur.0 as i32, cur.1 as i32), z, false);
                }
            };
            self.space.commit(w.toplevel().wl_surface());
        }
        Ok(())
    }

    pub(crate) fn handle_events(
        &mut self,
        dh: &DisplayHandle,
        popup_manager: &mut PopupManager,
        time: u32,
        renderer: Option<&mut Gles2Renderer>,
        egl_display: Option<&mut EGLDisplay>,
    ) -> Instant {
        self.space.refresh(dh);
        popup_manager.cleanup();

        self.handle_focus();
        let mut should_render = false;
        match self.space_event.take() {
            Some(SpaceEvent::Quit) => {
                info!(self.log, "root layer shell surface removed.");
            }
            Some(SpaceEvent::WaitConfigure {
                first,
                width,
                height,
            }) => {
                self.space_event.replace(Some(SpaceEvent::WaitConfigure {
                    first,
                    width,
                    height,
                }));
            }
            None => {
                if let (Some(size), Some(layer_surface)) =
                    (self.pending_dimensions.take(), self.layer.as_ref())
                {
                    let width = size.w.try_into().unwrap();
                    let height = size.h.try_into().unwrap();

                    layer_surface.set_size(width, height);
                    if let Visibility::Hidden = self.visibility {
                        if self.config.exclusive_zone() {
                            self.layer
                                .as_ref()
                                .unwrap()
                                .set_exclusive_zone(self.config.get_hide_handle().unwrap() as i32);
                        }
                        let target = match self.config.anchor() {
                            PanelAnchor::Left | PanelAnchor::Right => -(self.dimensions.w),
                            PanelAnchor::Top | PanelAnchor::Bottom => -(self.dimensions.h),
                        } + self.config.get_hide_handle().unwrap() as i32;
                        layer_surface.set_margin(target, target, target, target);
                    } else if self.config.exclusive_zone() {
                        let list_thickness = match self.config.anchor() {
                            PanelAnchor::Left | PanelAnchor::Right => width,
                            PanelAnchor::Top | PanelAnchor::Bottom => height,
                        };
                        self.layer
                            .as_ref()
                            .unwrap()
                            .set_exclusive_zone(list_thickness as i32);
                    }
                    layer_surface.wl_surface().commit();
                    self.space_event.replace(Some(SpaceEvent::WaitConfigure {
                        first: false,
                        width: size.w,
                        height: size.h,
                    }));
                } else if self.layer.is_some() {
                    should_render = if self.full_clear == 4 {
                        self.update_window_locations().is_ok()
                    } else {
                        true
                    };
                }
            }
        }

        if let (Some(renderer), Some(egl_display)) = (renderer, egl_display) {
            self.popups.retain_mut(|p: &mut WrapperPopup| {
                p.handle_events(
                    popup_manager,
                    renderer.egl_context(),
                    egl_display,
                    self.c_display.as_ref().unwrap(),
                )
            });

            if should_render {
                let _ = self.render(renderer, time);
            }
            if let Some(egl_surface) = self.egl_surface.as_ref() {
                if egl_surface.get_size() != Some(self.dimensions.to_physical(1)) {
                    self.full_clear = 4;
                }
            }
        }

        self.last_dirty.unwrap_or_else(Instant::now)
    }

    pub fn configure_panel_layer(
        &mut self,
        _layer: &LayerSurface,
        configure: sctk::shell::layer::LayerSurfaceConfigure,
        renderer: &mut Option<Gles2Renderer>,
        egl_display: &mut Option<EGLDisplay>,
    ) {
        let (w, h) = configure.new_size;

        match self.space_event.take() {
            Some(e) => match e {
                SpaceEvent::WaitConfigure {
                    first,
                    mut width,
                    mut height,
                } => {
                    if w != 0 {
                        width = w as i32;
                    }
                    if h != 0 {
                        height = h as i32;
                    }
                    if first {
                        let log = self.log.clone();
                        let client_egl_surface = unsafe {
                            ClientEglSurface::new(
                                WlEglSurface::new(
                                    self.layer.as_ref().unwrap().wl_surface().id(),
                                    width,
                                    height,
                                )
                                .unwrap(), // TODO remove unwrap
                                self.c_display.as_ref().unwrap().clone(),
                                self.layer.as_ref().unwrap().wl_surface().clone(),
                            )
                        };
                        let new_egl_display = if let Some(egl_display) = egl_display.take() {
                            egl_display
                        } else {
                            EGLDisplay::new(&client_egl_surface, log.clone())
                                .expect("Failed to initialize EGL display")
                        };

                        let egl_context = EGLContext::new_with_config(
                            &new_egl_display,
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
                                new_egl_display.get_display_handle().handle,
                                egl_context.config_id(),
                                ffi::egl::MIN_SWAP_INTERVAL as c_int,
                                &mut min_interval_attr,
                            );
                        }

                        let new_renderer = if let Some(renderer) = renderer.take() {
                            renderer
                        } else {
                            unsafe {
                                Gles2Renderer::new(egl_context, log.clone())
                                    .expect("Failed to initialize EGL Surface")
                            }
                        };

                        let egl_surface = Rc::new(
                            EGLSurface::new(
                                &new_egl_display,
                                new_renderer
                                    .egl_context()
                                    .pixel_format()
                                    .expect("Failed to get pixel format from EGL context "),
                                new_renderer.egl_context().config_id(),
                                client_egl_surface,
                                log.clone(),
                            )
                            .expect("Failed to initialize EGL Surface"),
                        );

                        renderer.replace(new_renderer);
                        egl_display.replace(new_egl_display);
                        self.egl_surface.replace(egl_surface);
                    } else if self.dimensions != (width as i32, height as i32).into()
                        && self.pending_dimensions.is_none()
                    {
                        self.w_accumulated_damage.drain(..);
                        self.egl_surface.as_ref().unwrap().resize(
                            width as i32,
                            height as i32,
                            0,
                            0,
                        );
                    }
                    self.dimensions = (w as i32, h as i32).into();
                    self.layer.as_ref().unwrap().wl_surface().commit();
                    self.full_clear = 4;
                }
                SpaceEvent::Quit => (),
            },
            None => {
                if w != 0
                    && h != 0
                    && self.dimensions != (w as i32, h as i32).into()
                    && self.pending_dimensions.is_none()
                {
                    self.w_accumulated_damage.drain(..);
                    self.egl_surface
                        .as_ref()
                        .unwrap()
                        .resize(w as i32, h as i32, 0, 0);
                    self.dimensions = (w as i32, h as i32).into();
                    self.layer.as_ref().unwrap().wl_surface().commit();
                    self.full_clear = 4;
                }
            }
        }
    }

    pub fn configure_panel_popup(
        &mut self,
        popup: &sctk::shell::xdg::popup::Popup,
        config: sctk::shell::xdg::popup::PopupConfigure,
        renderer: Option<&mut Gles2Renderer>,
        egl_display: Option<&mut EGLDisplay>,
    ) {
        let (renderer, egl_display) =
            if let (Some(renderer), Some(egl_display)) = (renderer, egl_display) {
                (renderer, egl_display)
            } else {
                return;
            };

        let (width, height) = (config.width, config.height);
        if let Some(p) = self
            .popups
            .iter_mut()
            .find(|p| popup.wl_surface() == p.c_popup.wl_surface())
        {
            p.state.take();
            let _ = p.s_surface.send_configure();
            match config.kind {
                popup::ConfigureKind::Initial => {
                    let wl_egl_surface = match WlEglSurface::new(p.c_wl_surface.id(), width, height)
                    {
                        Ok(s) => s,
                        Err(_) => return,
                    };
                    let client_egl_surface = unsafe {
                        ClientEglSurface::new(
                            wl_egl_surface,
                            self.c_display.as_ref().unwrap().clone(),
                            p.c_wl_surface.clone(),
                        )
                    };
                    let egl_context = renderer.egl_context();
                    let egl_surface = Rc::new(
                        EGLSurface::new(
                            egl_display,
                            egl_context
                                .pixel_format()
                                .expect("Failed to get pixel format from EGL context "),
                            egl_context.config_id(),
                            client_egl_surface,
                            None,
                        )
                        .expect("Failed to initialize EGL Surface"),
                    );
                    p.egl_surface.replace(egl_surface);
                }
                popup::ConfigureKind::Reactive => {
                    // TODO
                }
                popup::ConfigureKind::Reposition { token: _token } => {
                    // TODO
                }
                _ => {}
            };
            // TODO is this needed?
            // popup_manager.commit(p.s_surface.wl_surface());
            p.dirty = true;
            p.full_clear = 4;
        }
    }

    pub fn set_theme_window_color(&mut self, mut color: [f32; 4]) {
        if let CosmicPanelBackground::ThemeDefault(alpha) = self.config.background {
            if let Some(alpha) = alpha {
                color[3] = alpha;
            }
            self.bg_color = color;
        }
        self.bg_color = color;
        self.full_clear = 4;
    }
}
