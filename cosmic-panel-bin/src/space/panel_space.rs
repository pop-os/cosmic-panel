// SPDX-License-Identifier: MPL-2.0-only

use std::{
    cell::{Cell, RefCell},
    process::Child,
    rc::Rc,
    time::{Duration, Instant},
};

use itertools::Itertools;
use sctk::{
    output::OutputInfo,
    reexports::{
        client::protocol::{wl_output as c_wl_output, wl_surface as c_wl_surface},
        client::{self, Attached, Main},
        protocols::wlr::unstable::layer_shell::v1::client::{
            zwlr_layer_shell_v1::{self, Layer},
            zwlr_layer_surface_v1,
        },
    },
    shm::AutoMemPool,
};
use slog::Logger;
use smithay::desktop::space::RenderZindex;
use smithay::{
    backend::{
        egl::{display::EGLDisplay, surface::EGLSurface},
        renderer::{gles2::Gles2Renderer, utils::draw_surface_tree, Bind, Frame, Renderer, Unbind},
    },
    desktop::{
        draw_window,
        space::RenderError,
        utils::{bbox_from_surface_tree, damage_from_surface_tree},
        PopupKind, PopupManager, Space, Window,
    },
    reexports::wayland_server::{protocol::wl_surface::WlSurface as s_WlSurface, Client, Resource},
    utils::{Logical, Physical, Point, Rectangle, Size},
};
use xdg_shell_wrapper::{
    client_state::Focus,
    space::{Popup, SpaceEvent, Visibility},
    util::smootherstep,
};

use cosmic_panel_config::{CosmicPanelBackground, CosmicPanelConfig, PanelAnchor};

use crate::space::Alignment;

/// space for the cosmic panel
#[derive(Debug)]
pub(crate) struct PanelSpace {
    pub config: CosmicPanelConfig,
    pub log: Option<Logger>,
    pub(crate) space: Space,
    pub(crate) popup_manager: PopupManager,
    pub(crate) clients_left: Vec<Client>,
    pub(crate) clients_center: Vec<Client>,
    pub(crate) clients_right: Vec<Client>,
    pub(crate) children: Vec<Child>,
    pub(crate) last_dirty: Option<Instant>,
    pub(crate) pending_dimensions: Option<Size<i32, Logical>>,
    pub(crate) full_clear: u8,
    pub(crate) next_render_event: Rc<Cell<Option<SpaceEvent>>>,
    pub(crate) dimensions: Size<i32, Logical>,
    pub(crate) focused_surface: Rc<RefCell<Option<s_WlSurface>>>,
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
    pub(crate) w_accumulated_damage: Vec<Vec<Rectangle<i32, Physical>>>,
}

impl PanelSpace {
    /// create a new space for the cosmic panel
    pub fn new(config: CosmicPanelConfig, log: Logger) -> Self {
        Self {
            config,
            space: Space::new(log.clone()),
            popup_manager: PopupManager::new(log.clone()),
            log: Some(log),
            full_clear: 0,
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

    pub(crate) fn next_space_event(&self) -> Rc<Cell<Option<SpaceEvent>>> {
        Rc::clone(&self.next_render_event)
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
                p.send_popup_done();
                self.popup_manager.commit(p.wl_surface());
            }
        }
    }

    pub(crate) fn handle_focus(&mut self, focus: &Focus) {
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

    pub(crate) fn render(&mut self, time: u32) -> Result<(), RenderError<Gles2Renderer>> {
        if self.next_render_event.get() != None {
            return Ok(());
        }

        let clear_color = match self.config.background {
            CosmicPanelBackground::ThemeDefault => [0.5, 0.5, 0.5, 0.5],
            CosmicPanelBackground::Color(c) => c,
        };
        let renderer = self.renderer.as_mut().unwrap();
        renderer
            .bind(self.egl_surface.as_ref().unwrap().clone())
            .expect("Failed to bind surface to GL");

        let log_clone = self.log.clone().unwrap();
        if let Some(o) = self
            .space
            .windows()
            .find_map(|w| self.space.outputs_for_window(w).pop())
        {
            let output_size = o.current_mode().ok_or(RenderError::OutputNoMode)?.size;
            // TODO handle fractional scaling?
            // let output_scale = o.current_scale().fractional_scale();
            // We explicitly use ceil for the output geometry size to make sure the damage
            // spans at least the output size. Round and floor would result in parts not drawn as the
            // frame size could be bigger than the maximum the output_geo would define.
            let output_geo =
                Rectangle::from_loc_and_size(o.current_location(), output_size.to_logical(1));

            let cur_damage = if self.full_clear > 0 {
                vec![]
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
            // dbg!(&cur_damage);
            let damage = Self::damage_for_buffer(
                cur_damage,
                &mut self.w_accumulated_damage,
                self.egl_surface.as_ref().unwrap(),
                self.full_clear,
            );
            let should_render = damage.as_ref().map(|d| !d.is_empty()).unwrap_or(true);
            if should_render {
                let mut damage = damage.unwrap_or_else(|| vec![]);
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
                                    .window_location(&w)
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

                                if w_damage.len() == 0 {
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

                self.egl_surface
                    .as_ref()
                    .unwrap()
                    .swap_buffers(if damage.is_empty() {
                        None
                    } else {
                        Some(&mut damage)
                    })
                    .expect("Failed to swap buffers.");
            }

            // Popup rendering
            let clear_color = [0.0, 0.0, 0.0, 0.0];
            for p in self.popups.iter_mut().filter(|p| {
                p.dirty
                    && match p.popup_state.get() {
                        None => true,
                        _ => false,
                    }
            }) {
                let _ = renderer.unbind();
                renderer
                    .bind(p.egl_surface.as_ref().unwrap().clone())
                    .expect("Failed to bind surface to GL");
                let p_bbox = bbox_from_surface_tree(p.s_surface.wl_surface(), (0, 0));
                let cur_damage = if p.full_clear > 0 {
                    vec![]
                } else {
                    damage_from_surface_tree(
                        p.s_surface.wl_surface(),
                        p_bbox.loc.to_f64().to_physical(1.0),
                        1.0,
                        Some((&self.space, &o)),
                    )
                };

                let mut damage = match Self::damage_for_buffer(
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
                            vec![p_bbox.to_physical(1)]
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
                p.egl_surface
                    .as_ref()
                    .unwrap()
                    .swap_buffers(if damage.is_empty() {
                        None
                    } else {
                        Some(&mut damage)
                    })
                    .expect("Failed to swap buffers.");
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
        cur_damage: Vec<Rectangle<i32, Physical>>,
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
                .into_iter()
                .map(|v| v.into_iter().cloned())
                .flatten()
                .collect_vec();
            Some(d)
        };

        // acc damage should only ever be length 4
        if acc_damage.len() > 4 {
            acc_damage.drain(..acc_damage.len() - 4);
        }

        // dbg!(age, dmg_counts, &acc_damage, &ret);
        ret
    }

    pub(crate) fn update_window_locations(&mut self) {
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

        let mut windows_right = self
            .space
            .windows()
            .cloned()
            .filter_map(|w| {
                self.clients_right.iter().enumerate().find_map(|(i, c)| {
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
                self.clients_center.iter().enumerate().find_map(|(i, c)| {
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
                self.clients_left.iter().enumerate().find_map(|(i, c)| {
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
        ) -> (Alignment, usize, i32) {
            // dbg!(w.bbox());
            match anchor {
                PanelAnchor::Left | PanelAnchor::Right => (alignment, *i, w.bbox().size.h),
                PanelAnchor::Top | PanelAnchor::Bottom => (alignment, *i, w.bbox().size.w),
            }
        }

        let left = windows_left
            .iter()
            .map(|e| map_fn(e, anchor, Alignment::Left));
        let left_sum = left.clone().map(|(_, _, d)| d).sum::<i32>()
            + spacing as i32 * (windows_left.len().max(1) as i32 - 1);

        let center = windows_center
            .iter()
            .map(|e| map_fn(e, anchor, Alignment::Center));
        let center_sum = center.clone().map(|(_, _, d)| d).sum::<i32>()
            + spacing as i32 * (windows_center.len().max(1) as i32 - 1);

        let right = windows_right
            .iter()
            .map(|e| map_fn(e, anchor, Alignment::Right));

        let right_sum = right.clone().map(|(_, _, d)| d).sum::<i32>()
            + spacing as i32 * (windows_right.len().max(1) as i32 - 1);

        // TODO should the center area in the panel be scrollable? and if there are too many on the sides the rightmost are moved to the center?
        // FIXME panics if the list is larger than the output can hold
        let total_sum = left_sum + center_sum + right_sum;
        if total_sum + padding as i32 * 2 + spacing as i32 * (num_lists as i32 - 1)
            > list_length as i32
        {
            panic!("List expanded past max size!");
        }

        fn center_in_bar(thickness: u32, dim: u32) -> i32 {
            (thickness as i32 - dim as i32) / 2
        }

        let requested_eq_length: i32 = (list_length / num_lists).try_into().unwrap();
        // dbg!(requested_eq_length);
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
                        .map_window(&w, (cur.0 as i32, cur.1 as i32), z, false);
                }
                PanelAnchor::Top | PanelAnchor::Bottom => {
                    let cur = (
                        cur,
                        center_in_bar(list_thickness.try_into().unwrap(), size.y as u32),
                    );
                    prev += size.x as u32;
                    self.space
                        .map_window(&w, (cur.0 as i32, cur.1 as i32), z, false);
                }
            };
            self.space.commit(w.toplevel().wl_surface());
        }

        let mut prev: u32 = center_offset as u32;
        // dbg!(center_offset);
        for (i, w) in &mut windows_center.iter_mut() {
            let size: Point<_, Logical> = (w.bbox().size.w, w.bbox().size.h).into();
            // dbg!(size);
            let cur = prev + spacing * *i as u32;
            match anchor {
                PanelAnchor::Left | PanelAnchor::Right => {
                    let cur = (
                        center_in_bar(list_thickness.try_into().unwrap(), size.x as u32),
                        cur,
                    );
                    prev += size.y as u32;
                    self.space
                        .map_window(&w, (cur.0 as i32, cur.1 as i32), z, false);
                }
                PanelAnchor::Top | PanelAnchor::Bottom => {
                    let cur = (
                        cur,
                        center_in_bar(list_thickness.try_into().unwrap(), size.y as u32),
                    );
                    // dbg!(cur);
                    prev += size.x as u32;
                    self.space
                        .map_window(&w, (cur.0 as i32, cur.1 as i32), z, false);
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
                        .map_window(&w, (cur.0 as i32, cur.1 as i32), z, false);
                }
                PanelAnchor::Top | PanelAnchor::Bottom => {
                    let cur = (
                        cur,
                        center_in_bar(list_thickness.try_into().unwrap(), size.y as u32),
                    );
                    prev += size.x as u32;
                    self.space
                        .map_window(&w, (cur.0 as i32, cur.1 as i32), z, false);
                }
            };
            self.space.commit(w.toplevel().wl_surface());
        }
    }
}
