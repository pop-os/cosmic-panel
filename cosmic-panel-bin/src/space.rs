// SPDX-License-Identifier: MPL-2.0-only

use std::{
    cell::{Cell, RefCell},
    ffi::OsString,
    fs,
    os::unix::{net::UnixStream, prelude::AsRawFd},
    process::Child,
    rc::Rc,
    time::{Instant, Duration}, collections::VecDeque,
};

use anyhow::bail;
use freedesktop_desktop_entry::{self, DesktopEntry, Iter};
use itertools::{Itertools, izip};
use libc::c_int;
use sctk::{
    environment::Environment,
    output::OutputInfo,
    reexports::{
        client::{self, Attached, Main},
        client::protocol::{wl_output as c_wl_output, wl_surface as c_wl_surface},
        protocols::{
            wlr::unstable::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1},
            xdg_shell::client::{
                xdg_popup,
                xdg_positioner::{Anchor, Gravity, XdgPositioner},
                xdg_surface::{self, XdgSurface},
                xdg_wm_base::{XdgWmBase, self},
            },
        },
    },
    shm::AutoMemPool, window,
};
use slog::{info, Logger, trace};
use smithay::{
    backend::{
        egl::{
            context::{EGLContext, GlAttributes},
            display::EGLDisplay,
            ffi::{
                self,
                egl::{GetConfigAttrib, SwapInterval},
            },
            surface::EGLSurface, self,
        },
        renderer::{
            Bind, Frame, gles2::Gles2Renderer, ImportEgl, Renderer, Unbind,
            utils::draw_surface_tree, ImportDma,
        },
    },
    desktop::{
        draw_window,
        Kind,
        PopupKind,
        PopupManager, space::{RenderError, SurfaceTree}, Space, utils::{damage_from_surface_tree, send_frames_surface_tree, bbox_from_surface_tree}, Window, WindowSurfaceType, draw_popups,
    },
    nix::{fcntl, libc},
    reexports::{wayland_protocols::xdg::shell::server::xdg_toplevel, wayland_server::{
        self, Client, Display as s_Display, DisplayHandle,
        protocol::wl_surface::WlSurface as s_WlSurface, Resource,
    }},
    utils::{Logical, Rectangle, Size, Point, Physical},
    wayland::{
        SERIAL_COUNTER,
        shell::xdg::{PopupSurface, PositionerState},
    },
};
use wayland_egl::WlEglSurface;
use xdg_shell_wrapper::{
    client_state::{Env, Focus},
    config::WrapperConfig,
    space::{ClientEglSurface, SpaceEvent, Visibility, WrapperSpace, Popup, PopupState},
    util::{exec_child, get_client_sock, smootherstep},
};

use cosmic_panel_config::{CosmicPanelConfig, PanelAnchor};

impl Default for PanelSpace {
    fn default() -> Self {
        Self {
            popup_manager: PopupManager::new(None),
            full_clear: false,
            config: Default::default(),
            log: Default::default(),
            space: Space::new(None),
            clients_left: Default::default(),
            clients_center: Default::default(),
            clients_right: Default::default(),
            children: Default::default(),
            last_dirty: Default::default(),
            pending_dimensions: Default::default(),
            next_render_event: Default::default(),
            dimensions: Default::default(),
            focused_surface: Default::default(),
            pool: Default::default(),
            layer_shell: Default::default(),
            output: Default::default(),
            c_display: Default::default(),
            egl_display: Default::default(),
            renderer: Default::default(),
            layer_surface: Default::default(),
            egl_surface: Default::default(),
            layer_shell_wl_surface: Default::default(),
            popups: Default::default(),
            w_accumulated_damage: Default::default(),
            visibility: Visibility::Visible,
        }
    }
}

/// space for the cosmic panel
#[derive(Debug)]
pub struct PanelSpace {
    /// config for the panel space
    pub config: CosmicPanelConfig,
    /// logger for the panel space
    pub log: Option<Logger>,
    pub(crate) space: Space,
    pub(crate) popup_manager: PopupManager,
    pub(crate) clients_left: Vec<Client>,
    pub(crate) clients_center: Vec<Client>,
    pub(crate) clients_right: Vec<Client>,    
    pub(crate) children: Vec<Child>,
    pub(crate) last_dirty: Option<Instant>,
    pub(crate) pending_dimensions: Option<Size<i32, Logical>>,
    pub(crate) full_clear: bool,
    pub(crate) next_render_event: Rc<Cell<Option<SpaceEvent>>>,
    pub(crate) dimensions: Size<i32, Logical>,
    /// focused surface so it can be changed when a window is removed
    focused_surface: Rc<RefCell<Option<s_WlSurface>>>,
    /// visibility state of the panel / panel
    pub(crate) visibility: Visibility,
    pub(crate) pool: Option<AutoMemPool>,
    pub(crate) layer_shell: Option<Attached<zwlr_layer_shell_v1::ZwlrLayerShellV1>>,
    pub(crate) output: Option<(c_wl_output::WlOutput, OutputInfo)>,
    pub(crate) c_display: Option<client::Display>,
    pub(crate) egl_display: Option<EGLDisplay>,
    pub(crate) renderer: Option<Gles2Renderer>,
    pub(crate) layer_surface: Option<Main<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>>,
    pub(crate) egl_surface: Option<Rc<EGLSurface>>,
    pub(crate) layer_shell_wl_surface: Option<Attached<c_wl_surface::WlSurface>>,
    pub(crate) popups: Vec<Popup>,
    pub(crate) w_accumulated_damage: VecDeque<Vec<Rectangle<i32, Physical>>>,
}

impl PanelSpace {
    /// create a new space for the cosmic panel
    pub fn new(config: CosmicPanelConfig, log: Logger) -> Self {
        Self {
            config,
            space: Space::new(log.clone()),
            popup_manager: PopupManager::new(log.clone()),
            log: Some(log),
            ..Default::default()
        }
    }


    fn handle_focus(&mut self, focus: &Focus) {
        // always visible if not configured for autohide
        if self.config.autohide().is_none() {
            return;
        }

        let layer_surface = self.layer_surface.as_ref().unwrap();
        let layer_shell_wl_surface = self.layer_shell_wl_surface.as_ref().unwrap();
        match self.visibility {
            Visibility::Hidden => {
                if let Focus::Current(_) = focus {
                    // start transition to visible
                    let margin = match self.config.anchor() {
                        PanelAnchor::Left | PanelAnchor::Right => {
                            -(self.dimensions.w)
                        }
                        PanelAnchor::Top | PanelAnchor::Bottom => {
                            -(self.dimensions.h)
                        }
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
                    let panel_size = match self.config.anchor() {
                        PanelAnchor::Left | PanelAnchor::Right => {
                            self.dimensions.w
                        }
                        PanelAnchor::Top | PanelAnchor::Bottom => {
                            self.dimensions.h
                        }
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

                if let Focus::LastFocus(_) = focus {
                    // start transition to visible
                    self.close_popups();
                    self.visibility = Visibility::TransitionToHidden {
                        last_instant: now,
                        progress: total_t.checked_sub(progress).unwrap_or_default(),
                        prev_margin,
                    }
                } else {
                    let panel_size = match self.config.anchor() {
                        PanelAnchor::Left | PanelAnchor::Right => {
                            self.dimensions.w
                        }
                        PanelAnchor::Top | PanelAnchor::Bottom => {
                            self.dimensions.h
                        }
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


    fn constrain_dim(&self, size: Size<i32, Logical>) -> Size<i32, Logical> {
        let mut w = size.w.try_into().unwrap();
        let mut h = size.h.try_into().unwrap();

        let output_dims = self
            .output
            .as_ref()
            .map(|(_, info)| info.modes[0].dimensions);
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
                } else if w > w_range.end {
                    w = w_range.end;
                }
            }
            if let (_, Some(h_range)) = self.config.get_dimensions((o_w as u32, o_h as u32)) {
                if h < h_range.start {
                    h = h_range.start;
                } else if h > h_range.end {
                    h = h_range.end;
                }
            }
        }
        (w.try_into().unwrap(), h.try_into().unwrap()).into()
    }

    fn render(&mut self, time: u32) -> Result<(), RenderError<Gles2Renderer>> {
        if self.next_render_event.get() != None {
            return Ok(());
        }
        let clear_color = [0.0, 0.0, 0.0, 0.0];
        let renderer = self.renderer.as_mut().unwrap();
        renderer
            .bind(self.egl_surface.as_ref().unwrap().clone())
            .expect("Failed to bind surface to GL");

        let log_clone = self.log.clone().unwrap();
        if let Some(o) = self.space
            .windows()
            .find_map(|w| self.space.outputs_for_window(w).pop()) {
            let output_size = o.current_mode().ok_or(RenderError::OutputNoMode)?.size;
            // TODO handle fractional scaling?
            // let output_scale = o.current_scale().fractional_scale();
            // We explicitly use ceil for the output geometry size to make sure the damage
            // spans at least the output size. Round and floor would result in parts not drawn as the
            // frame size could be bigger than the maximum the output_geo would define.
            let output_geo = Rectangle::from_loc_and_size(o.current_location(), output_size.to_logical(1));

            let cur_damage = if self.full_clear {
                vec![(Rectangle::from_loc_and_size((0, 0), self.dimensions.to_physical(1)))]
            } else {
                let mut acc_damage = self.space.windows().fold(vec![], |acc, w| {
                    acc.into_iter()
                        .chain(w.accumulated_damage(
                            w.geometry().loc.to_f64().to_physical(1.0),
                            1.0,
                            Some((&self.space, &o)),
                        ))
                        .collect_vec()
                });
                acc_damage.dedup();
                acc_damage.retain(|rect| rect.overlaps(output_geo.to_physical(1)));
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
                acc_damage
            };

            let mut damage = Self::damage_for_buffer(cur_damage, &mut self.w_accumulated_damage, self.egl_surface.as_ref().unwrap());
            if damage.is_empty() {
                damage.push(Rectangle::from_loc_and_size((0, 0), self.dimensions.to_physical(1)));
            }

            let _ = renderer.render(
                self.dimensions.to_physical(1),
                smithay::utils::Transform::Flipped180,
                |renderer: &mut Gles2Renderer, frame| {
                    frame
                        .clear(clear_color, damage.iter().cloned().collect_vec().as_slice())
                        .expect("Failed to clear frame.");
                    for w in self.space.windows() {
                        let mut w_damage = damage.iter().filter_map(|r| r.intersection(w.bbox().to_physical(1))).collect_vec();
                        w_damage.dedup();
                        if damage.len() == 0 {
                            continue;
                        }

                        let _ = draw_window(
                            renderer,
                            frame,
                            w,
                            1.0,
                            w.geometry().loc.to_f64().to_physical(1.0),
                            &w_damage,
                            &log_clone,
                        );
                    }
                },
            );
            self.egl_surface
            .as_ref()
            .unwrap()
            .swap_buffers(Some(&mut damage))
            .expect("Failed to swap buffers.");

            // Popup rendering
            for p in self.popups.iter_mut().filter(|p| 
                p.dirty && match p.popup_state.get() {
                    None => true,
                    _ => false,
                }
            ) {
                let _ = renderer.unbind();
                renderer
                .bind(p.egl_surface.clone())
                .expect("Failed to bind surface to GL");
                let p_bbox = bbox_from_surface_tree(p.s_surface.wl_surface(), (0,0));
                let cur_damage = if self.full_clear {
                    vec![p_bbox.to_physical(1)]
                } else {
                    damage_from_surface_tree(p.s_surface.wl_surface(), p_bbox.loc.to_f64().to_physical(1.0), 1.0, Some((&self.space, &o)))
                };
                if cur_damage.len() == 0 {
                    continue;
                }

                let mut damage = Self::damage_for_buffer(cur_damage, &mut p.accumulated_damage, &p.egl_surface);
                if damage.is_empty() {
                    damage.push(p_bbox.to_physical(1));
                }

                let _ = renderer.render(
                    p_bbox.size.to_physical(1),
                    smithay::utils::Transform::Flipped180,
                    |renderer: &mut Gles2Renderer, frame| {
                        frame
                            .clear(clear_color, damage.iter().cloned().collect_vec().as_slice())
                            .expect("Failed to clear frame.");

                        let _ = draw_surface_tree(
                            renderer,
                            frame,
                            p.s_surface.wl_surface(),
                            1.0,
                            p_bbox.loc.to_f64().to_physical(1.0),
                            &damage,
                            &log_clone,
                        );
                    },
                );
                p.egl_surface
                .swap_buffers(Some(&mut damage))
                .expect("Failed to swap buffers.");
                p.dirty = false;
            }
        }

        self.space.send_frames(time);
        let _ = renderer.unbind();
        self.full_clear = false;
        Ok(())
    }

    fn damage_for_buffer(cur_damage: Vec<Rectangle<i32, Physical>>, acc_damage: &mut VecDeque<Vec<Rectangle<i32, Physical>>>, egl_surface: &Rc<EGLSurface>) -> Vec<Rectangle<i32, Physical>> {
        let age: usize = egl_surface.buffer_age().unwrap_or_default().try_into().unwrap_or_default();
        let dmg_counts = acc_damage.len();
        acc_damage.push_front(cur_damage);
        // dbg!(age, dmg_counts);

        let ret = if age == 0 || age > dmg_counts {
            Vec::new()
        } else {
             acc_damage.range(0..age).cloned().flatten().collect_vec()
        };

        if acc_damage.len() > 4 {
            acc_damage.drain(4..);
        }

        ret
    }


    fn update_window_locations(&mut self) {
        let padding = self.config.padding();
        let anchor = self.config.anchor();
        let spacing = self.config.spacing();
        // First try partitioning the panel evenly into N spaces.
        // If all windows fit into each space, then set their offsets and return.
        let (list_length, list_thickness) = match anchor {
            PanelAnchor::Left | PanelAnchor::Right => {
                (self.dimensions.h, self.dimensions.w)
            }
            PanelAnchor::Top | PanelAnchor::Bottom => {
                (self.dimensions.w, self.dimensions.h)
            }
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

        let mut windows_right = self.space.windows().cloned().filter_map(|w| {
            self.clients_right.iter().enumerate().find_map(|(i, c)| {
                if Some(c.id()) == w.toplevel().wl_surface().client_id() {
                    Some((i, w.clone()))
                } else {
                    None
                }
            })
        }).collect_vec();
        windows_right.sort_by(|(a_i, _), (b_i, _)| {
            a_i.cmp(b_i)
        });

        let mut windows_center = self.space.windows().cloned().filter_map(|w| {
            self.clients_center.iter().enumerate().find_map(|(i, c)| {
                if Some(c.id()) == w.toplevel().wl_surface().client_id() {
                    Some((i, w.clone()))
                } else {
                    None
                }
            })
        }).collect_vec();
        windows_center.sort_by(|(a_i, _), (b_i, _)| {
            a_i.cmp(b_i)
        });


        let mut windows_left = self.space.windows().cloned().filter_map(|w| {
            self.clients_left.iter().enumerate().find_map(|(i, c)| {
                if Some(c.id()) == w.toplevel().wl_surface().client_id() {
                    Some((i, w.clone()))
                } else {
                    None
                }
            })
        }).collect_vec();
        windows_left.sort_by(|(a_i, _), (b_i, _)| {
            a_i.cmp(b_i)
        });


        fn map_fn(
            (i, w): &(usize, Window),
            anchor: PanelAnchor,
            alignment: Alignment,
        ) -> (Alignment, usize, i32) {
            match anchor {
                PanelAnchor::Left | PanelAnchor::Right => {
                    (alignment, *i, w.bbox().size.h)
                }
                PanelAnchor::Top | PanelAnchor::Bottom => {
                    (alignment, *i, w.bbox().size.w)
                }
            }
        }

        let left = windows_left
            .iter()
            .map(|e| map_fn(e, anchor, Alignment::Left));
        let mut left_sum = left.clone().map(|(_, _, d)| d).sum::<i32>()
            + spacing as i32 * (windows_left.len().max(1) as i32 - 1);

        let center = windows_center
            .iter()
            .map(|e| map_fn(e, anchor, Alignment::Center));
        let mut center_sum = center.clone().map(|(_, _, d)| d).sum::<i32>()
            + spacing as i32 * (windows_center.len().max(1) as i32 - 1);

        let right = windows_right
            .iter()
            .map(|e| map_fn(e, anchor, Alignment::Right));
        let mut right_sum = right.clone().map(|(_, _, d)| d).sum::<i32>()
            + spacing as i32 * (windows_right.len().max(1) as i32 - 1);

        // TODO should the center area in the panel be scrollable? and if there are too many on the sides the rightmost are moved to the center?
        // FIXME panics if the list is larger than the output can hold
        let mut total_sum = left_sum + center_sum + right_sum;
        if total_sum + padding as i32 * 2 + spacing as i32 * (num_lists as i32 - 1)
            > list_length as i32
        {
           panic!("List expanded past max size!");
        }

        // XXX making sure the sum is > 0 after possibly over-subtracting spacing
        left_sum = left_sum.max(0);
        center_sum = center_sum.max(0);
        right_sum = right_sum.max(0);

        fn center_in_bar(thickness: u32, dim: u32) -> i32 {
            (thickness as i32 - dim as i32) / 2
        }

        let requested_eq_length: i32 = (list_length / num_lists).try_into().unwrap();
        let (right_sum, center_offset) = if is_dock {
            (0, padding as i32)
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
        // TODO remap windows with new locations instead of setting bbox loc

        for (i, w) in &mut windows_left
            .iter_mut()
        {
            let size: Point<_, Logical> =
                (w.bbox().size.w, w.bbox().size.h).into();
            let cur: u32 = prev + spacing * *i as u32;
            match anchor {
                PanelAnchor::Left | PanelAnchor::Right => {
                    let cur = (center_in_bar(list_thickness.try_into().unwrap(), size.x as u32), cur);
                    prev += size.y as u32;
                    w.bbox().loc = (cur.0 as i32, cur.1 as i32).into();
                }
                PanelAnchor::Top | PanelAnchor::Bottom => {
                    let cur = (cur, center_in_bar(list_thickness.try_into().unwrap(), size.y as u32));
                    prev += size.x as u32;
                    w.bbox().loc = (cur.0 as i32, cur.1 as i32).into();
                }
            };
        }

        let mut prev: u32 = center_offset as u32;
        // TODO remap windows with new locations instead of setting bbox loc

        for (i, w) in &mut windows_center
            .iter_mut()
        {
            let size: Point<_, Logical> =
                (w.bbox().size.w, w.bbox().size.h).into();
            let cur = prev + spacing * *i as u32;
            match anchor {
                PanelAnchor::Left | PanelAnchor::Right => {
                    let cur = (center_in_bar(list_thickness.try_into().unwrap(), size.x as u32), cur);
                    prev += size.y as u32;
                    w.bbox().loc = (cur.0 as i32, cur.1 as i32).into();
                }
                PanelAnchor::Top | PanelAnchor::Bottom => {
                    let cur = (cur, center_in_bar(list_thickness.try_into().unwrap(), size.y as u32));
                    prev += size.x as u32;
                    w.bbox().loc = (cur.0 as i32, cur.1 as i32).into();
                }
            };
        }

        // twice padding is subtracted
        let mut prev: u32 = list_length as u32 - padding - right_sum as u32;
        // TODO remap windows with new locations instead of setting bbox loc

        for (i, w) in &mut windows_right
            .iter_mut()
        {
            let size: Point<_, Logical> =
                (w.bbox().size.w, w.bbox().size.h).into();
            let cur = prev + spacing * *i as u32;
            match anchor {
                PanelAnchor::Left | PanelAnchor::Right => {
                    let cur = (center_in_bar(list_thickness.try_into().unwrap(), size.x as u32), cur);
                    prev += size.y as u32;
                    w.bbox().loc = (cur.0 as i32, cur.1 as i32).into();
                }
                PanelAnchor::Top | PanelAnchor::Bottom => {
                    let cur = (cur, center_in_bar(list_thickness.try_into().unwrap(), size.y as u32));
                    prev += size.x as u32;
                    w.bbox().loc = (cur.0 as i32, cur.1 as i32).into();
                }
            };
        }

    }

}

impl WrapperSpace for PanelSpace {
    type Config = CosmicPanelConfig;

    fn handle_events(&mut self, dh: &DisplayHandle, time: u32, _: &Focus) -> Instant {
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
                     width,
                     height,
                     serial: _serial,
                 }) => {
                if self.dimensions != (width as i32, height as i32).into()
                    && self.pending_dimensions.is_none()
                {
                    self.dimensions = (width as i32, height as i32).into();
                    self.layer_shell_wl_surface.as_ref().unwrap().commit();
                    self.egl_surface
                        .as_ref()
                        .unwrap()
                        .resize(width as i32, height as i32, 0, 0);
                    self.full_clear = true;
                }
            }
            Some(SpaceEvent::WaitConfigure { width, height }) => {
                self.next_render_event
                    .replace(Some(SpaceEvent::WaitConfigure { width, height }));
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
                            PanelAnchor::Left | PanelAnchor::Right => {
                                -(self.dimensions.w)
                            }
                            PanelAnchor::Top | PanelAnchor::Bottom => {
                                -(self.dimensions.h)
                            }
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
                        .replace(Some(SpaceEvent::WaitConfigure { width: size.w, height: size.h }));
                } else {
                    should_render = true;
                }
            }
        }

        self.popups.retain_mut(|p: &mut Popup| {
            p.handle_events(&mut self.popup_manager)
        });

        if should_render {
            let _ = self.render(time);
        }
        if self.egl_surface.as_ref().unwrap().get_size() != Some(self.dimensions.to_physical(1)) {
            self.full_clear = true;
        }

        self.last_dirty.unwrap_or_else(|| Instant::now())
    }

    fn popups(&self) -> &[Popup] {
        &self.popups
    }

    fn handle_button(&mut self, c_focused_surface: &c_wl_surface::WlSurface) {
        if self.focused_surface.borrow().is_none()
            && **self.layer_shell_wl_surface.as_ref().unwrap() == *c_focused_surface
        {
            self.close_popups()
        }
    }

    fn add_top_level(&mut self, w: Window) {
        self.full_clear = true;
        self.space.map_window(&w, (0, 0), false);
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
        PositionerState { rect_size, anchor_rect, anchor_edges, gravity, constraint_adjustment, offset, reactive, parent_size, parent_configure }: PositionerState,
    ) {
        self.close_popups();

        let s = if let Some(s) = self.space.windows().find(|w| {
            match w.toplevel() {
                Kind::Xdg(wl_s) => Some(wl_s.wl_surface()) == s_surface.get_parent_surface().as_ref(),
            }
        }) {
            s
        } else {
            return;
        };

        let c_wl_surface = env.create_surface().detach();
        let c_xdg_surface = xdg_wm_base.get_xdg_surface(&c_wl_surface);

        let wl_surface = s_surface.wl_surface().clone();
        let s_popup_surface = s_surface.clone();
        self.popup_manager.track_popup(PopupKind::Xdg(s_surface.clone())).unwrap();
        self.popup_manager.commit(&wl_surface);

        // dbg!(s.bbox().loc);
        positioner.set_size(rect_size.w, rect_size.h);
        positioner.set_anchor_rect(
            anchor_rect.loc.x + s.bbox().loc.x,
            anchor_rect.loc.y + s.bbox().loc.y,
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

        let cur_popup_state = Rc::new(Cell::new(Some(PopupState::WaitConfigure)));
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
                        popup_state.set(Some(PopupState::Configure {
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
        let client_egl_surface = ClientEglSurface {
            wl_egl_surface: WlEglSurface::new(&c_wl_surface, rect_size.w, rect_size.h),
            display: self.c_display.as_ref().unwrap().clone(),
        };

        let egl_context = self.renderer.as_ref().unwrap().egl_context();
        let egl_surface = Rc::new(
            EGLSurface::new(
                &self.egl_display.as_ref().unwrap(),
                egl_context
                    .pixel_format()
                    .expect("Failed to get pixel format from EGL context "),
                egl_context.config_id(),
                client_egl_surface,
                self.log.clone(),
            )
            .expect("Failed to initialize EGL Surface"),
        );

        self.popups.push(Popup {
            c_popup,
            c_xdg_surface,
            c_wl_surface: c_wl_surface,
            s_surface,
            egl_surface,
            dirty: false,
            popup_state: cur_popup_state,
            position: (0,0).into(),
            accumulated_damage: Default::default(),
        });
    }

    fn close_popups(&mut self) {
        for w in &mut self.space.windows() {
            for (PopupKind::Xdg(p), _) in
            PopupManager::popups_for_surface(w.toplevel().wl_surface())
            {
                p.send_popup_done();
                self.popup_manager.commit(p.wl_surface());
            }
        }
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
            let (clients_left, sockets_left): (Vec<_>, Vec<_>) = self
            .config
            .plugins_left()
            .unwrap_or_default()
            .iter()
            .map(|p| {
                let (c, s) = get_client_sock(display);
                (c, s)
            })
            .unzip();
        self.clients_left = clients_left;
        let (clients_center, sockets_center): (Vec<_>, Vec<_>) = self
            .config
            .plugins_center()
            .unwrap_or_default()
            .iter()
            .map(|p| {
                let (c, s) = get_client_sock(display);
                (c, s)
            })
            .unzip();
        self.clients_center = clients_center;
        let (clients_right, sockets_right): (Vec<_>, Vec<_>) = self
            .config
            .plugins_right()
            .unwrap_or_default()
            .iter()
            .map(|p| {
                let (c, s) = get_client_sock(display);
                (c, s)
            })
            .unzip();
        self.clients_right = clients_right;  
                  // TODO how slow is this? Would it be worth using a faster method of comparing strings?
                  self.children = Iter::new(freedesktop_desktop_entry::default_paths())
                  .filter_map(|path| {
                      izip!(
                          self.config.plugins_left().unwrap_or_default().iter(),
                          &self.clients_left,
                          &sockets_left
                      )
                      .chain(izip!(
                          self.config.plugins_center().unwrap_or_default().iter(),
                          &self.clients_center,
                          &sockets_center
                      ))
                      .chain(izip!(
                          self.config.plugins_right().unwrap_or_default().iter(),
                          &self.clients_right,
                          &sockets_right
                      ))
                      .find(|(app_file_name, _, _)| {
                          Some(OsString::from(&app_file_name).as_os_str()) == path.file_stem()
                      })
                      .and_then(|(_, _, client_socket)| {
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
                      })
                  })
                  .collect_vec();
  
              Ok(sockets_left
                  .into_iter()
                  .chain(sockets_center.into_iter())
                  .chain(sockets_right.into_iter())
                  .collect())
        } else {
            bail!("Clients have already been spawned!");
        }
    }

    fn add_output(
        &mut self,
        output: Option<&c_wl_output::WlOutput>,
        output_info: Option<&OutputInfo>,
        pool: AutoMemPool,
        c_display: client::Display,
        layer_shell: Attached<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
        log: Logger,
        c_surface: Attached<c_wl_surface::WlSurface>,
        focused_surface: Rc<RefCell<Option<s_WlSurface>>>,
    ) -> anyhow::Result<()> {
        if self.layer_shell_wl_surface.is_some()
            || self.output.is_some()
            || self.layer_shell.is_some()
        {
            bail!("output already added!")
        }

        let dimensions = self.constrain_dim((1, 1).into());

        let layer_surface =
            layer_shell.get_layer_surface(&c_surface, output, self.config.layer(), "".to_owned());

        layer_surface.set_anchor(self.config.anchor.into());
        layer_surface.set_keyboard_interactivity(self.config.keyboard_interactivity());
        layer_surface.set_size(
            dimensions.w.try_into().unwrap(),
            dimensions.h.try_into().unwrap(),
        );

        // Commit so that the server will send a configure event
        c_surface.commit();

        let next_render_event = Rc::new(Cell::new(Some(SpaceEvent::WaitConfigure {
            width: dimensions.w,
            height: dimensions.h,
        })));

        //let egl_surface_clone = egl_surface.clone();
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
                    next_render_event_handle.set(Some(SpaceEvent::Configure {
                        width: width.try_into().unwrap(),
                        height: height.try_into().unwrap(),
                        serial: serial.try_into().unwrap(),
                    }));
                }
                (_, _) => {}
            }
        });

        let client_egl_surface = ClientEglSurface {
            wl_egl_surface: WlEglSurface::new(&c_surface, dimensions.w, dimensions.h),
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
                    next_render_event_handle.set(Some(SpaceEvent::Configure {
                        width: width.try_into().unwrap(),
                        height: height.try_into().unwrap(),
                        serial: serial.try_into().unwrap(),
                    }));
                }
                (_, _) => {}
            }
        });

        self.output = output.cloned().zip(output_info.cloned());
        self.egl_display.replace(egl_display);
        self.renderer.replace(renderer);
        self.layer_shell.replace(layer_shell);
        self.c_display.replace(c_display);
        self.pool.replace(pool);
        self.layer_surface.replace(layer_surface);
        self.egl_surface.replace(egl_surface);
        self.dimensions = dimensions;
        self.focused_surface = focused_surface;
        self.next_render_event = next_render_event;
        self.full_clear = true;
        self.layer_shell_wl_surface = Some(c_surface);

        Ok(())
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

    fn dirty_window(&mut self, s: &s_WlSurface) {
        self.last_dirty = Some(Instant::now());
        let mut full_clear = false;
        
        if let Some(w) = self.space.window_for_surface(s, WindowSurfaceType::ALL) {
            // TODO improve this for when there are changes to the lists of plugins while running
            let padding: Size<i32, Logical> = ((2 * self.config.padding()).try_into().unwrap(), (2 * self.config.padding()).try_into().unwrap()).into();
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
                    } => (width, height),
                    SpaceEvent::WaitConfigure { width, height } => (width, height),
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

            if full_clear {
                self.full_clear = true;
                self.update_window_locations();
            }
        }
    }

    fn dirty_popup(&mut self, s: &s_WlSurface) {
        if let Some(p) = self.popups.iter_mut().find(|p| p.s_surface.wl_surface() == s) {
            p.dirty = true;
            self.popup_manager.commit(s);
        }
    }

    fn renderer(&mut self) -> Option<&mut Gles2Renderer> {
        self.renderer.as_mut()
    }
}

#[derive(Debug)]
pub enum Alignment {
    Left,
    Center,
    Right,
}
