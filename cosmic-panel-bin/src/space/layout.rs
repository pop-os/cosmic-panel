use std::{
    i32,
    slice::IterMut,
    sync::{atomic::AtomicBool, Arc, MutexGuard},
    time::{Duration, Instant},
    u32,
};

use crate::{
    iced::{
        elements::{
            background::background_element,
            overflow_button::{
                self, overflow_button_element, OverflowButton, OverflowButtonElement,
            },
            overflow_popup::{overflow_popup_element, BORDER_WIDTH},
            CosmicMappedInternal, PopupMappedInternal,
        },
        IcedElement,
    },
    minimize::MinimizeApplet,
    space::{corner_element::RoundedRectangleSettings, Alignment},
    xdg_shell_wrapper::space::Visibility,
};

use super::{
    panel_space::{ClientShrinkSize, PanelClient},
    PanelSpace,
};
use crate::xdg_shell_wrapper::space::WrapperSpace;
use anyhow::bail;
use cosmic::widget::Id;
use cosmic_panel_config::PanelAnchor;
use itertools::{chain, Itertools};
use sctk::shell::WaylandSurface;
use smithay::{
    desktop::{space::SpaceElement, Space, Window},
    reexports::wayland_server::Resource,
    utils::{IsAlive, Physical, Rectangle, Size},
    wayland::{
        compositor::with_states, fractional_scale::with_fractional_scale, seat::WaylandFocus,
    },
};
use tracing::info;

impl PanelSpace {
    pub(crate) fn layout_(&mut self) -> anyhow::Result<()> {
        self.remap_attempts = self.remap_attempts.saturating_sub(1);

        let make_indices_contiguous = |windows: &mut Vec<(usize, Window, Option<u32>)>| {
            windows.sort_by(|(a_i, ..), (b_i, ..)| a_i.cmp(b_i));
            for (j, (i, ..)) in windows.iter_mut().enumerate() {
                *i = j;
            }
        };

        let mut left_overflow_button = None;
        let mut right_overflow_button = None;
        let mut center_overflow_button = None;

        let to_map = self
            .space
            .elements()
            .cloned()
            .filter_map(|w| {
                let w = match w {
                    CosmicMappedInternal::Window(w) => w,
                    CosmicMappedInternal::OverflowButton(b)
                        if overflow_button::with_id(&b, |id| {
                            &self.left_overflow_button_id == id
                        }) =>
                    {
                        left_overflow_button = Some(b);
                        return None;
                    },
                    CosmicMappedInternal::OverflowButton(b)
                        if overflow_button::with_id(&b, |id| {
                            &self.center_overflow_button_id == id
                        }) =>
                    {
                        center_overflow_button = Some(b);
                        return None;
                    },
                    CosmicMappedInternal::OverflowButton(b)
                        if overflow_button::with_id(&b, |id| {
                            &self.right_overflow_button_id == id
                        }) =>
                    {
                        right_overflow_button = Some(b);
                        return None;
                    },
                    _ => return None,
                };

                w.alive().then_some(w)
            })
            .collect_vec();

        let is_dock = !self.config.expand_to_edges()
            || self.animate_state.as_ref().is_some_and(|a| !(a.cur.expanded > 0.5));
        let mut windows_left = to_map
            .iter()
            .cloned()
            .filter_map(|w| {
                let Some(t) = w.toplevel() else {
                    tracing::warn!("Window {:?} has no toplevel", w.bbox());
                    return None;
                };
                self.clients_left.lock().unwrap().iter().enumerate().find_map(|(i, c)| {
                    if Some(c.client.id()) == t.wl_surface().client().map(|c| c.id()) {
                        Some((i, w.clone(), c.minimize_priority))
                    } else {
                        None
                    }
                })
            })
            .collect_vec();
        make_indices_contiguous(&mut windows_left);

        let mut windows_center = to_map
            .iter()
            .cloned()
            .filter_map(|w| {
                let Some(t) = w.toplevel() else {
                    tracing::warn!("Window {:?} has no toplevel", w.bbox());
                    return None;
                };
                self.clients_center.lock().unwrap().iter().enumerate().find_map(|(i, c)| {
                    if Some(c.client.id()) == t.wl_surface().client().map(|c| c.id()) {
                        Some((i, w.clone(), c.minimize_priority))
                    } else {
                        None
                    }
                })
            })
            .collect_vec();
        make_indices_contiguous(&mut windows_center);

        let mut windows_right = to_map
            .iter()
            .cloned()
            .filter_map(|w| {
                let Some(t) = w.toplevel() else {
                    tracing::warn!("Window {:?} has no toplevel", w.bbox());
                    return None;
                };
                self.clients_right.lock().unwrap().iter().enumerate().find_map(|(i, c)| {
                    if Some(c.client.id()) == t.wl_surface().client().map(|c| c.id()) {
                        Some((i, w.clone(), c.minimize_priority))
                    } else {
                        None
                    }
                })
            })
            .collect_vec();
        make_indices_contiguous(&mut windows_right);

        if is_dock {
            windows_center = windows_left
                .drain(..)
                .chain(windows_center)
                .chain(windows_right.drain(..))
                .collect_vec();
        }

        let res = self.layout(
            windows_left,
            windows_center,
            windows_right,
            left_overflow_button,
            right_overflow_button,
            center_overflow_button,
        );
        if let Err(e) = res.as_ref() {
            info!("Requires relayout: {:?}", e);
        }
        res
    }

    pub(crate) fn layout(
        &mut self,
        mut windows_left: Vec<(usize, Window, Option<u32>)>,
        mut windows_center: Vec<(usize, Window, Option<u32>)>,
        mut windows_right: Vec<(usize, Window, Option<u32>)>,
        mut left_overflow_button: Option<OverflowButtonElement>,
        mut right_overflow_button: Option<OverflowButtonElement>,
        mut center_overflow_button: Option<OverflowButtonElement>,
    ) -> anyhow::Result<()> {
        self.space.refresh();
        let mut bg_color = self.bg_color();
        for c in 0..3 {
            bg_color[c] *= bg_color[3];
        }
        let gap = self.gap();
        let padding_u32 = self.config.padding();
        let padding_scaled = padding_u32 as f64 * self.scale;
        let anchor = self.config.anchor();
        let spacing_u32 = self.config.spacing();
        let spacing_scaled = spacing_u32 as f64 * self.scale;
        // First try partitioning the panel evenly into N spaces.
        // If all windows fit into each space, then set their offsets and return.
        let (list_cross, layer_major) = match anchor {
            PanelAnchor::Left | PanelAnchor::Right => (self.dimensions.w, self.dimensions.h),
            PanelAnchor::Top | PanelAnchor::Bottom => (self.dimensions.h, self.dimensions.w),
        };
        let is_dock = !self.config.expand_to_edges();
        if is_dock {
            if let Some(left_button) = left_overflow_button.take() {
                self.space.unmap_elem(&CosmicMappedInternal::OverflowButton(left_button));
            }
            if let Some(right_button) = right_overflow_button.take() {
                self.space.unmap_elem(&CosmicMappedInternal::OverflowButton(right_button));
            }
        }

        let has_sides = !windows_left.is_empty()
            || !windows_right.is_empty()
            || left_overflow_button.is_some()
            || right_overflow_button.is_some();
        let mut num_lists: u32 = 0;
        if has_sides {
            num_lists += 2;
        }
        let has_center = !windows_center.is_empty() || center_overflow_button.is_some();
        if has_center {
            num_lists += 1;
        }

        fn map_fn(
            (i, w, _): &(usize, Window, Option<u32>),
            anchor: PanelAnchor,
            alignment: Alignment,
        ) -> (Alignment, usize, i32, i32, i32) {
            let (mut size, mut suggested_bounds) = w
                .toplevel()
                .map(|t| {
                    let s = t.current_state();
                    (s.size.unwrap_or_default(), s.bounds.unwrap_or_default())
                })
                .unwrap_or_default();
            let bbox = w.bbox().size;

            if size.w == 0 {
                size.w = bbox.w;
            }
            size.w = size.w.min(bbox.w);

            if size.h == 0 {
                size.h = bbox.h;
            }
            size.h = size.h.min(bbox.h);

            if suggested_bounds.w == 0 {
                suggested_bounds.w = size.w;
            }
            if suggested_bounds.h == 0 {
                suggested_bounds.h = size.h;
            }

            match anchor {
                PanelAnchor::Left | PanelAnchor::Right => {
                    (alignment, *i, size.h, size.w, suggested_bounds.h.min(size.h))
                },
                PanelAnchor::Top | PanelAnchor::Bottom => {
                    (alignment, *i, size.w, size.h, suggested_bounds.w.min(suggested_bounds.w))
                },
            }
        }

        let left = windows_left.iter().map(|e| {
            let l = map_fn(e, anchor, Alignment::Left);
            l
        });

        let left_sum_scaled =
            left.clone().map(|(_, _, _, _, suggested_length)| suggested_length).sum::<i32>() as f64
                * self.scale
                + spacing_scaled * windows_left.len().saturating_sub(1) as f64;
        let left_sum_scaled = if let Some(left_button) = left_overflow_button.as_ref() {
            let size = left_button.bbox().size.to_f64();
            left_sum_scaled
                + if self.config.is_horizontal() { size.w } else { size.h }
                + spacing_scaled
        } else {
            left_sum_scaled
        };

        let center = windows_center.iter().map(|e| map_fn(e, anchor, Alignment::Center));
        let center_sum_scaled =
            center.clone().map(|(_, _, _, _, suggested_length)| suggested_length).sum::<i32>()
                as f64
                * self.scale
                + spacing_scaled * windows_center.len().saturating_sub(1) as f64;
        let center_sum_scaled = if let Some(center_button) = center_overflow_button.as_ref() {
            let size = center_button.bbox().size.to_f64();
            center_sum_scaled
                + if self.config.is_horizontal() { size.w } else { size.h }
                + spacing_scaled
        } else {
            center_sum_scaled
        };

        let right = windows_right.iter().map(|e| map_fn(e, anchor, Alignment::Right));
        let right_sum_scaled =
            right.clone().map(|(_, _, _length, _, suggested_length)| suggested_length).sum::<i32>()
                as f64
                * self.scale
                + spacing_scaled * windows_right.len().saturating_sub(1) as f64;
        let right_sum_scaled = if let Some(right_button) = right_overflow_button.as_ref() {
            let size = right_button.bbox().size.to_f64();
            right_sum_scaled
                + if self.config.is_horizontal() { size.w } else { size.h }
                + spacing_scaled
        } else {
            right_sum_scaled
        };

        let total_sum_scaled = left_sum_scaled + center_sum_scaled + right_sum_scaled;
        let new_list_length = (total_sum_scaled
            + padding_scaled * 2.0
            + spacing_scaled * num_lists.saturating_sub(1) as f64)
            as i32;
        let new_list_thickness = (2.0 * padding_scaled
            + chain!(left.clone(), center.clone(), right.clone())
                .map(|(_, _, _, thickness, _)| thickness)
                .max()
                .unwrap_or(0) as f64
                * self.scale) as i32;

        self.actual_size = Size::<i32, Physical>::from(if self.config.is_horizontal() {
            (new_list_length, new_list_thickness)
        } else {
            (new_list_thickness, new_list_length)
        })
        .to_f64()
        .to_logical(self.scale)
        .to_i32_round();

        let actual_size_constrained = self.constrain_dim(self.actual_size, Some(gap as u32));
        if self.config.is_horizontal() {
            self.actual_size.h = actual_size_constrained.h;
        } else {
            self.actual_size.w = actual_size_constrained.w;
        }

        let (new_logical_length, new_logical_crosswise_dim) = if self.config.is_horizontal() {
            (self.actual_size.w, self.actual_size.h)
        } else {
            (self.actual_size.h, self.actual_size.w)
        };
        if new_logical_crosswise_dim == 0 {
            tracing::warn!("Invalid crosswise dimension.");
        }
        let new_dim = if self.config.is_horizontal() {
            let mut dim = actual_size_constrained;
            dim.h += gap as i32;
            dim
        } else {
            let mut dim = actual_size_constrained;
            dim.w += gap as i32;
            dim
        };

        let (new_list_dim_length, new_list_thickness_dim) = if self.config.is_horizontal() {
            (new_dim.w, new_dim.h)
        } else {
            (new_dim.h, new_dim.w)
        };

        let left_sum = left_sum_scaled / self.scale;
        let center_sum = center_sum_scaled / self.scale;
        let right_sum = right_sum_scaled / self.scale;

        let container_length = if let Some(anim_state) = self.animate_state.as_ref() {
            (new_logical_length as f32
                + (new_list_dim_length - new_logical_length) as f32 * anim_state.cur.expanded)
                as i32
        } else if is_dock {
            new_logical_length
        } else {
            new_list_dim_length
        };
        self.container_length = container_length;
        let container_lengthwise_pos = (new_list_dim_length - container_length) / 2;

        let mut center_pos = layer_major as f64 / 2. - center_sum / 2.;

        let left_pos = container_lengthwise_pos as f64 + padding_u32 as f64;
        let mut right_pos = new_list_dim_length as f64
            - container_lengthwise_pos as f64
            - right_sum
            - padding_u32 as f64;

        let one_third = (layer_major as f64 - (spacing_u32 * num_lists.saturating_sub(1)) as f64)
            / (3.min(num_lists) as f64);
        let one_half = layer_major as f64 / (2.min(num_lists) as f64);
        let larger_side = left_sum.max(right_sum);

        let mut target_center_len =
            (layer_major as f64 - larger_side * (2.)).max(one_third).min(layer_major as f64);
        if num_lists == 1 {
            target_center_len -= padding_u32 as f64 * 2.;
        } else {
            target_center_len -= spacing_u32 as f64;
        }
        let target_left_len = if !has_center {
            (layer_major as f64
                - right_sum.min(one_half)
                - (spacing_u32 as f64) / 2.
                - padding_u32 as f64)
                .max(one_half)
        } else {
            (one_half
                - target_center_len.min(center_sum) / 2.
                - (spacing_u32 as f64) / 2.
                - padding_u32 as f64)
                .max(one_third)
        }
        .min(layer_major as f64);

        let target_right_len = if !has_center {
            (layer_major as f64
                - left_sum.min(one_half)
                - (spacing_u32 as f64) / 2.
                - padding_u32 as f64)
                .max(one_half)
        } else {
            (one_half
                - target_center_len.min(center_sum) / 2.
                - (spacing_u32 as f64) / 2.
                - padding_u32 as f64)
                .max(one_third)
        }
        .min(layer_major as f64);
        let suggested_size = ((self.config.size.get_applet_icon_size(true) as f64
            + self.config.size.get_applet_padding(true) as f64 * 2.)
            * -1.5 // allows some wiggle room
            * self.scale) as i32;

        let center_overflow = (center_sum - target_center_len) as i32;
        if center_overflow < suggested_size {
            // check if it can be expanded
            self.relax_overflow_center(center_overflow.unsigned_abs(), &mut center_overflow_button)
        } else if center_overflow > 0 {
            let overflow = self.shrink_center((center_sum - target_center_len) as u32);
            bail!("overflow: {}", overflow)
        }

        if !is_dock && self.animate_state.is_none() {
            let left_overflow = (left_sum - target_left_len) as i32;

            if left_overflow < suggested_size {
                self.relax_overflow_left(left_overflow.unsigned_abs(), &mut left_overflow_button);
            } else if left_overflow > 0 {
                info!("target: {target_left_len}, actual: {left_sum}");
                let overflow = self.shrink_left(left_overflow as u32);
                bail!("left overflow: {} {}", left_overflow, overflow)
            }

            let right_overflow = (right_sum - target_right_len) as i32;
            if right_overflow < suggested_size {
                self.relax_overflow_right(
                    right_overflow.unsigned_abs(),
                    &mut right_overflow_button,
                );
            } else if right_overflow > 0 {
                let overflow = self.shrink_right(right_overflow as u32);
                bail!("right overflow: {} {}", right_overflow, overflow)
            }
        }

        // update input region of panel when list changes
        let (input_region, layer) = match (self.input_region.as_ref(), self.layer.as_ref()) {
            (Some(r), Some(layer)) => (r, layer),
            _ => panic!("input region or layer missing"),
        };

        // must use logical coordinates for layout here

        fn center_in_bar(crosswise_dim: u32, dim: u32) -> i32 {
            (crosswise_dim as i32 - dim as i32) / 2
        }

        if new_list_thickness_dim != list_cross {
            self.pending_dimensions = Some(new_dim);
            self.is_dirty = true;
            anyhow::bail!("resizing list");
        }
        // offset for centering
        let margin_offset = match anchor {
            PanelAnchor::Top | PanelAnchor::Left => gap,
            PanelAnchor::Bottom | PanelAnchor::Right => 0,
        } as i32;

        if let Some(right_button) = right_overflow_button {
            let size = right_button.bbox().size.to_f64();
            let crosswise_pos = if self.config.is_horizontal() {
                margin_offset
                    + center_in_bar(new_logical_crosswise_dim.try_into().unwrap(), size.h as u32)
            } else {
                margin_offset
                    + center_in_bar(new_logical_crosswise_dim.try_into().unwrap(), size.w as u32)
            };

            let loc = if self.config().is_horizontal() {
                (right_pos.round() as i32, crosswise_pos)
            } else {
                (crosswise_pos, right_pos.round() as i32)
            };
            right_pos += size.h + spacing_u32 as f64;
            self.space.map_element(CosmicMappedInternal::OverflowButton(right_button), loc, true);
        };

        if let Some(center_button) = center_overflow_button {
            let size = center_button.bbox().size.to_f64();
            let crosswise_pos = if self.config.is_horizontal() {
                margin_offset
                    + center_in_bar(new_logical_crosswise_dim.try_into().unwrap(), size.h as u32)
            } else {
                margin_offset
                    + center_in_bar(new_logical_crosswise_dim.try_into().unwrap(), size.w as u32)
            };
            let loc = if self.config().is_horizontal() {
                (center_pos.round() as i32, crosswise_pos)
            } else {
                (crosswise_pos, center_pos.round() as i32)
            };
            self.space.map_element(CosmicMappedInternal::OverflowButton(center_button), loc, false);
            center_pos += size.h + spacing_u32 as f64;
        }

        let mut map_windows = |windows: IterMut<'_, (usize, Window, Option<u32>)>,
                               mut prev|
         -> f64 {
            for (_, w, minimize_priority) in windows {
                // XXX this is a hack to get the logical size of the window
                // TODO improve how this is done
                let mut size = w.bbox().size.to_f64();
                let configured_size =
                    w.toplevel().and_then(|t| t.current_state().bounds).unwrap_or_default();
                if configured_size.w != 0 {
                    size.w = size.w.min(configured_size.w as f64);
                }
                if configured_size.h != 0 {
                    size.h = size.h.min(configured_size.h as f64);
                }
                let cur: f64 = prev;
                let (x, y);

                if self.config.is_horizontal() {
                    let cur = (
                        cur,
                        margin_offset
                            + center_in_bar(
                                new_logical_crosswise_dim.try_into().unwrap(),
                                size.h as u32,
                            ),
                    );
                    (x, y) = (cur.0 as i32, cur.1);
                    prev += size.w + spacing_u32 as f64;
                    self.space.map_element(CosmicMappedInternal::Window(w.clone()), (x, y), false);
                } else {
                    let cur = (
                        margin_offset
                            + center_in_bar(
                                new_logical_crosswise_dim.try_into().unwrap(),
                                size.w as u32,
                            ),
                        cur,
                    );
                    (x, y) = (cur.0, cur.1 as i32);
                    prev += size.h + spacing_u32 as f64;
                    self.space.map_element(CosmicMappedInternal::Window(w.clone()), (x, y), false);
                }
                if minimize_priority.is_some() {
                    let new_rect = Rectangle {
                        loc: (x, y).into(),
                        size: ((size.w.ceil() as i32).max(1), (size.w.ceil() as i32).max(1)).into(),
                    };
                    if new_rect != self.minimize_applet_rect
                        && Instant::now().duration_since(self.last_minimize_update)
                            > Duration::from_secs(1)
                    {
                        self.minimize_applet_rect = new_rect;
                        self.last_minimize_update = Instant::now();
                        let output = self.output.as_ref().map(|o| o.1.name()).unwrap_or_default();
                        _ = self.panel_tx.send(crate::PanelCalloopMsg::MinimizeRect {
                            output,
                            applet_info: MinimizeApplet {
                                priority: if is_dock { 1 } else { 0 },
                                rect: new_rect,
                                surface: layer.wl_surface().clone(),
                            },
                        });
                    }
                }
            }
            prev
        };
        let left_pos = map_windows(windows_left.iter_mut(), left_pos);

        // will be already offset if dock
        map_windows(windows_center.iter_mut(), center_pos);

        map_windows(windows_right.iter_mut(), right_pos);
        // if there is a left overflow_button, map it
        if let Some(left_button) = left_overflow_button {
            let size = left_button.bbox().size.to_f64();
            let crosswise_pos = if self.config.is_horizontal() {
                margin_offset
                    + center_in_bar(new_logical_crosswise_dim.try_into().unwrap(), size.h as u32)
            } else {
                margin_offset
                    + center_in_bar(new_logical_crosswise_dim.try_into().unwrap(), size.w as u32)
            };
            let loc = if self.config().is_horizontal() {
                (left_pos.round() as i32, crosswise_pos)
            } else {
                (crosswise_pos, left_pos.round() as i32)
            };
            self.space.map_element(CosmicMappedInternal::OverflowButton(left_button), loc, false);
        }
        self.space.refresh();

        let mut panel_size = self.actual_size.to_f64().to_physical(self.scale);
        let container_length_scaled = self.container_length as f64 * self.scale;
        let container_lengthwise_pos_scaled = container_lengthwise_pos as f32 * self.scale as f32;
        if self.config.is_horizontal() {
            panel_size.w = container_length_scaled;
        } else {
            panel_size.h = container_length_scaled;
        }
        let (w, h) = if is_dock {
            if self.config.is_horizontal() {
                (container_length, new_logical_crosswise_dim)
            } else {
                (new_logical_crosswise_dim, container_length)
            }
        } else {
            (new_dim.w, new_dim.h)
        };
        if !self.background_element.as_ref().is_some_and(|e| {
            e.with_program(|p| {
                p.logical_height == h && p.logical_width == w && self.bg_color() == p.color
            })
        }) || self.animate_state.as_ref().is_some()
            || matches!(
                self.visibility,
                Visibility::TransitionToHidden { .. } | Visibility::TransitionToVisible { .. }
            )
        {
            if let Some(bg) = self.background_element.take() {
                self.space.unmap_elem(&CosmicMappedInternal::Background(bg));
            }
            let gap_scaled = self.gap() as f64 * self.scale;
            let border_radius = self.border_radius() as f64 * self.scale;

            let border_radius = border_radius.min(panel_size.w / 2.).min(panel_size.h / 2.);
            let (rad_tl, rad_tr, rad_bl, rad_br) = match (self.config.anchor, self.gap()) {
                (PanelAnchor::Right, 0) => (border_radius, 0., border_radius, 0.),
                (PanelAnchor::Left, 0) => (0., border_radius, 0., border_radius),
                (PanelAnchor::Bottom, 0) => (border_radius, border_radius, 0., 0.),
                (PanelAnchor::Top, 0) => (0., 0., border_radius, border_radius),
                _ => (border_radius, border_radius, border_radius, border_radius),
            };

            let anim_gap_scaled = self.anchor_gap as f32 * self.scale as f32;
            let loc = match self.config.anchor {
                PanelAnchor::Left => {
                    [gap_scaled as f32 + anim_gap_scaled, container_lengthwise_pos_scaled]
                },
                PanelAnchor::Right => [-anim_gap_scaled, container_lengthwise_pos_scaled],
                PanelAnchor::Top => [container_lengthwise_pos_scaled, -anim_gap_scaled],
                PanelAnchor::Bottom => {
                    [container_lengthwise_pos_scaled, gap_scaled as f32 + anim_gap_scaled]
                },
            };
            self.panel_rect_settings = RoundedRectangleSettings {
                rad_tl: rad_tl as f32,
                rad_tr: rad_tr as f32,
                rad_bl: rad_bl as f32,
                rad_br: rad_br as f32,
                loc,
                rect_size: [panel_size.w as f32, panel_size.h as f32],
                border_width: 0.0,
                drop_shadow: 0.0,
                bg_color: [0.0, 0.0, 0.0, 1.0],
                border_color: [0.0, 0.0, 0.0, 0.0],
            };

            let Some(output) = self.output.as_ref().map(|o| o.1.clone()) else {
                bail!("output missing");
            };
            let loc = match self.config.anchor {
                PanelAnchor::Left => [gap as f32, container_lengthwise_pos as f32],
                PanelAnchor::Right => [0., container_lengthwise_pos as f32],
                PanelAnchor::Bottom => [container_lengthwise_pos as f32, 0.],
                PanelAnchor::Top => [container_lengthwise_pos as f32, gap as f32],
            };

            let border_radius = self.border_radius().min(w as u32).min(h as u32) as f32 / 2.;
            let radius = match (self.config.anchor, self.gap()) {
                (PanelAnchor::Right, 0) => [border_radius as f32, 0., 0., border_radius as f32],
                (PanelAnchor::Left, 0) => [0., border_radius as f32, border_radius as f32, 0.],
                (PanelAnchor::Bottom, 0) => [border_radius as f32, border_radius as f32, 0., 0.],
                (PanelAnchor::Top, 0) => [0., 0., border_radius as f32, border_radius as f32],
                _ => [
                    border_radius as f32,
                    border_radius as f32,
                    border_radius as f32,
                    border_radius as f32,
                ],
            };
            let bg = background_element(
                Id::new("panel_bg"),
                w,
                h,
                radius,
                self.loop_handle.clone(),
                self.colors.theme.clone(),
                self.space.id(),
                loc,
                self.bg_color(),
                self.config.border_width,
            );
            bg.output_enter(&output, Rectangle::default());
            self.background_element = Some(bg.clone());
            self.space.map_element(CosmicMappedInternal::Background(bg), (0, 0), false);
        }
        input_region.subtract(0, 0, i32::MAX, i32::MAX);
        let anim_gap = self.anchor_gap;

        if is_dock {
            let (layer_length, actual_length) = if self.config.is_horizontal() {
                (new_dim.w, self.actual_size.w)
            } else {
                (new_dim.h, self.actual_size.h)
            };
            let side = (layer_length as u32 - actual_length as u32) / 2;

            let (loc, size) = match self.config.anchor {
                PanelAnchor::Left => (
                    (-1, side as i32),
                    (
                        new_logical_crosswise_dim + self.gap() as i32 + 1 + anim_gap,
                        container_length,
                    ),
                ),
                PanelAnchor::Right => (
                    (0, side as i32 - anim_gap),
                    (new_logical_crosswise_dim + self.gap() as i32 + 1, container_length),
                ),
                PanelAnchor::Top => (
                    (side as i32, -1),
                    (
                        container_length,
                        new_logical_crosswise_dim + self.gap() as i32 + 1 + anim_gap,
                    ),
                ),
                PanelAnchor::Bottom => (
                    (side as i32, 0 - anim_gap),
                    (container_length, new_logical_crosswise_dim + self.gap() as i32 + 1),
                ),
            };

            input_region.add(loc.0, loc.1, size.0, size.1);
        } else {
            let (loc, size) = match self.config.anchor {
                PanelAnchor::Left => ((-1, 0), (new_dim.w + 1 + anim_gap, new_dim.h)),
                PanelAnchor::Right => ((-anim_gap, 0), (new_dim.w + 1 + anim_gap, new_dim.h)),
                PanelAnchor::Top => ((0, -1), (new_dim.w, new_dim.h + 1 + anim_gap)),
                PanelAnchor::Bottom => ((0, -anim_gap), (new_dim.w, new_dim.h + 1 + anim_gap)),
            };

            input_region.add(loc.0, loc.1, size.0, size.1);
        };
        layer.wl_surface().set_input_region(Some(input_region.wl_region()));

        self.reorder_overflow_space(OverflowSection::Left);
        self.reorder_overflow_space(OverflowSection::Center);
        self.reorder_overflow_space(OverflowSection::Right);

        Ok(())
    }

    // reorder overflow space windows, and remove dead windows
    fn reorder_overflow_space(&mut self, section: OverflowSection) {
        let (space, clients) = match section {
            OverflowSection::Left => (&mut self.overflow_left, self.clients_left.lock().unwrap()),
            OverflowSection::Center => {
                (&mut self.overflow_center, self.clients_center.lock().unwrap())
            },
            OverflowSection::Right => {
                (&mut self.overflow_right, self.clients_right.lock().unwrap())
            },
        };
        let mut elements = space.elements().cloned().collect_vec();
        if elements.is_empty() {
            return;
        } else {
            elements.retain_mut(|e| {
                if let PopupMappedInternal::Window(w) = e {
                    if !w.alive() {
                        space.unmap_elem(&PopupMappedInternal::Window(w.clone()));
                        false
                    } else {
                        true
                    }
                } else {
                    true
                }
            });
        }

        let mut overflow_cnt: usize = 0;
        let cur_cnt = elements.len();

        let applet_size_unit = self.config.size.get_applet_icon_size_with_padding(true);
        let padding = self.config.padding as i32;
        let spacing = self.config.spacing as i32;
        let Some(output) = self.output.as_ref().map(|o| o.1.clone()) else {
            return;
        };

        elements.sort_by(|a, b| {
            // sort by position in client list
            let pos_a = clients.iter().position(|c| {
                if let PopupMappedInternal::Window(w) = a {
                    w.toplevel().is_some_and(|t| {
                        t.wl_surface().client().is_some_and(|w_client| w_client == c.client)
                    })
                } else {
                    false
                }
            });
            let pos_b = clients.iter().position(|c| {
                if let PopupMappedInternal::Window(w) = b {
                    w.toplevel().is_some_and(|t| {
                        t.wl_surface().client().is_some_and(|w_client| w_client == c.client)
                    })
                } else {
                    false
                }
            });
            pos_a.cmp(&pos_b)
        });

        for e in elements {
            match &e {
                PopupMappedInternal::Window(w) => {
                    if !w.alive() {
                        space.unmap_elem(&PopupMappedInternal::Window(w.clone()));
                    } else {
                        let x_i = overflow_cnt % 8;
                        let mut x = BORDER_WIDTH as i32
                            + padding
                            + x_i as i32 * (applet_size_unit as i32 + spacing);
                        let mut y = BORDER_WIDTH as i32
                            + padding
                            + (overflow_cnt / 8) as i32 * (applet_size_unit as i32 + spacing);
                        if !self.config.is_horizontal() {
                            std::mem::swap(&mut x, &mut y);
                        }
                        space.map_element(e, (x, y), false);
                        overflow_cnt += 1;
                    }
                },
                PopupMappedInternal::Popup(p) => {
                    let prev_cnt = p.with_program(|p| p.count);
                    if prev_cnt != cur_cnt {
                        let actual = cur_cnt.saturating_sub(1);
                        let mut popup_major = 2. * BORDER_WIDTH as f32
                            + actual.min(8) as f32 * applet_size_unit as f32
                            + 2. * padding as f32
                            + (actual.min(8).saturating_sub(1) as f32) * spacing as f32;
                        let mut popup_cross = 2. * BORDER_WIDTH as f32
                            + (actual as f32 / 8.).ceil().min(1.0) * applet_size_unit as f32
                            + 2. * padding as f32;
                        if !self.config.is_horizontal() {
                            std::mem::swap(&mut popup_major, &mut popup_cross);
                        }

                        let new_popup = PopupMappedInternal::Popup(overflow_popup_element(
                            match section {
                                OverflowSection::Left => self.left_overflow_popup_id.clone(),
                                OverflowSection::Center => self.center_overflow_popup_id.clone(),
                                OverflowSection::Right => self.right_overflow_popup_id.clone(),
                            },
                            popup_major,
                            popup_cross,
                            self.loop_handle.clone(),
                            self.colors.theme.clone(),
                            self.space.id(),
                            actual,
                        ));
                        space.unmap_elem(&PopupMappedInternal::Popup(p.clone()));
                        new_popup.output_enter(&output, Rectangle::default());
                        space.map_element(new_popup, (0, 0), false);
                    }
                },
                _ => (),
            }
        }
    }

    fn shrinkable_clients<'a>(
        &self,
        clients: impl Iterator<Item = &'a PanelClient>,
    ) -> OverflowClientPartition {
        let mut overflow_partition = OverflowClientPartition::default();
        overflow_partition.suggested_size =
            (self.config.size.get_applet_icon_size_with_padding(true) as f64
                + 2. * self.config.get_applet_padding(true) as f64)
                .round() as u32;
        for c in clients {
            let Some(w) = self.space.elements().find_map(|e| {
                let CosmicMappedInternal::Window(w) = e else {
                    return None;
                };
                if w.alive()
                    && w.toplevel().is_some_and(|t| {
                        t.wl_surface().client().is_some_and(|w_client| w_client == c.client)
                    })
                {
                    let w_clone = w.clone();
                    w_clone.refresh();
                    Some((w_clone, c.shrink_priority.unwrap_or_default()))
                } else {
                    None
                }
            }) else {
                continue;
            };
            if let Some(shrink_min_size) = c.shrink_min_size {
                overflow_partition.shrinkable.push((w.0, w.1 as i32, shrink_min_size));
            } else if c.shrink_priority.is_some() {
                overflow_partition.movable.push(w);
            } else {
                // make shrinkable if no shrink priority with lowest priority so it is moved last
                overflow_partition.shrinkable.push((w.0, -1, ClientShrinkSize::AppletUnit(1)));
            }
        }
        // sort by priority
        overflow_partition.shrinkable.sort_by(|(_, a, _), (_, b, _)| b.cmp(a));
        overflow_partition.movable.sort_by(|(_, a), (_, b)| b.cmp(a));
        overflow_partition
    }

    fn shrink_left(&mut self, overflow: u32) -> u32 {
        let left = self.clients_left.lock().unwrap();
        let mut clients = self.shrinkable_clients(left.iter());
        drop(left);
        self.shrink_clients(overflow, &mut clients, OverflowSection::Left, false)
    }

    fn shrink_center(&mut self, overflow: u32) -> u32 {
        let g = self.clients_center.lock().unwrap();
        let left_g = self.clients_left.lock().unwrap();
        let right_g = self.clients_right.lock().unwrap();
        let center: Vec<&PanelClient> = if self.config.expand_to_edges {
            g.iter().collect()
        } else {
            left_g.iter().chain(g.iter()).chain(right_g.iter()).collect()
        };
        let mut clients = self.shrinkable_clients(center.into_iter());
        drop(g);
        drop(left_g);
        drop(right_g);
        self.shrink_clients(overflow, &mut clients, OverflowSection::Center, false)
    }

    fn shrink_right(&mut self, overflow: u32) -> u32 {
        let right = self.clients_right.lock().unwrap();
        let mut clients = self.shrinkable_clients(right.iter());
        drop(right);
        self.shrink_clients(overflow, &mut clients, OverflowSection::Right, false)
    }

    fn shrink_clients(
        &mut self,
        mut overflow: u32,
        clients: &mut OverflowClientPartition,
        section: OverflowSection,
        force_smaller: bool,
    ) -> u32 {
        info!("Overflow: {overflow} in section {section:?}");
        let unit_size = self.config.size.get_applet_icon_size_with_padding(true);

        let mut sum = 0.;
        for (w, priority, min_units) in clients.shrinkable.iter_mut() {
            if overflow == 0 {
                break;
            }
            let suggested_bounds = w
                .toplevel()
                .map(|t| {
                    let s = t.current_state();
                    s.bounds.unwrap_or_default()
                })
                .unwrap();

            let mut size = w.bbox().size.to_f64();
            if size.w < 1. {
                size.w = 1.;
            }
            if size.h < 1. {
                size.h = 1.;
            }
            let configured_size = w
                .toplevel()
                .and_then(|t| t.current_state().size)
                .map(|s| s.to_f64())
                .unwrap_or(size);
            if configured_size.w >= 1. {
                size.w = size.w.min(configured_size.w as f64);
            }
            if configured_size.h >= 1. {
                size.h = size.h.min(configured_size.h as f64);
            }

            let (major_dim, suggested_dim) = if self.config.is_horizontal() {
                (size.w, suggested_bounds.w)
            } else {
                (size.h, suggested_bounds.h)
            };
            sum += major_dim;
            if (major_dim < min_units.to_pixels(unit_size) as f64 || *priority < 0)
                && !force_smaller
            {
                continue;
            }
            let new_dim = (major_dim as u32).saturating_sub(overflow);
            let new_dim = if force_smaller || new_dim >= min_units.to_pixels(unit_size) {
                new_dim
            } else {
                min_units.to_pixels(unit_size)
            }
            .max(1);
            let diff = (major_dim as u32).saturating_sub(new_dim);
            if diff == 0 && suggested_dim as u32 == new_dim {
                continue;
            }
            tracing::info!("Shrinking window {size:?} by {diff} to {new_dim} {suggested_dim}");

            if let Some(t) = w.toplevel() {
                t.with_pending_state(|s| {
                    if self.config.is_horizontal() {
                        s.size = None;
                        s.bounds = Some((new_dim as i32, 0).into());
                    } else {
                        s.size = None;
                        s.bounds = Some((0, new_dim as i32).into());
                    }
                });
                t.send_pending_configure();
                overflow = overflow.saturating_sub(diff);
            }
        }
        if overflow > 0 {
            overflow = self.move_to_overflow(
                overflow,
                self.config.is_horizontal(),
                clients.clone(),
                section,
            );
        }
        if overflow > 0 && !force_smaller {
            tracing::info!(
                "Overflow not resolved {sum:.1} {overflow}. Forcing lowest priority shrinkable applets to be \
                 smaller than configured...",
            );
            return self.shrink_clients(overflow, clients, section, true);
        }
        overflow
    }

    /// Move clients to overflow space
    fn move_to_overflow(
        &mut self,
        mut overflow: u32,
        is_horizontal: bool,
        clients: OverflowClientPartition,
        section: OverflowSection,
    ) -> u32 {
        if clients.movable.len() <= 1 {
            tracing::info!("Needs at least 2 movable clients to move to overflow space.");
            return overflow;
        }
        info!("Moving clients to overflow space {section:?} {overflow}");
        let overflow_space = match section {
            OverflowSection::Left => &mut self.overflow_left,
            OverflowSection::Center => &mut self.overflow_center,
            OverflowSection::Right => &mut self.overflow_right,
        };
        let mut overflow_cnt = overflow_space.elements().count();
        let applet_size_unit = self.config.size.get_applet_icon_size(true)
            + 2 * self.config.size.get_applet_padding(true) as u32;
        let spacing = self.config.spacing;

        if overflow_cnt == 0 {
            overflow += applet_size_unit + spacing;
        }
        let space = &mut self.space;

        tracing::info!("Number of movable clients {}", clients.movable.len());
        for w in clients.movable {
            if overflow == 0 {
                break;
            }
            let bbox = w.0.bbox();
            tracing::info!("Moving applet with bbox: {bbox:?}");

            if bbox.size.w == 0
                || bbox.size.h == 0
                || !w.0.wl_surface().map(|s| s.is_alive()).unwrap_or_default()
            {
                continue;
            }
            let diff = if is_horizontal { bbox.size.w as u32 } else { bbox.size.h as u32 };
            overflow = overflow.saturating_sub(diff);
            let padding = self.config.padding as i32;
            // TODO spacing & padding
            let x_i = overflow_cnt % 8;
            let mut x = padding
                + x_i as i32 * (applet_size_unit as i32 + spacing as i32)
                + BORDER_WIDTH as i32;
            let mut y = BORDER_WIDTH as i32
                + (overflow_cnt / 8) as i32 * (applet_size_unit + spacing) as i32;
            if !self.config.is_horizontal() {
                std::mem::swap(&mut x, &mut y);
            }

            space.unmap_elem(&CosmicMappedInternal::Window(w.0.clone()));
            overflow_space.map_element(PopupMappedInternal::Window(w.0.clone()), (x, y), true);
            // Rows of 8 with configured applet size
            if let Some(t) = w.0.toplevel() {
                with_states(t.wl_surface(), |states| {
                    with_fractional_scale(states, |fractional_scale| {
                        fractional_scale.set_preferred_scale(self.scale);
                    });
                });
                t.with_pending_state(|s| {
                    s.size = Some((applet_size_unit as i32, applet_size_unit as i32).into());
                    s.bounds = Some((applet_size_unit as i32, applet_size_unit as i32).into());
                });

                t.send_pending_configure();
            }
            overflow_cnt += 1;
        }
        overflow_space.refresh();
        let overflow_cnt = overflow_space
            .elements()
            .filter(|e| if let PopupMappedInternal::Window(w) = e { w.alive() } else { false })
            .count();

        let space = self.config.spacing as f32;
        let padding = self.config.padding as f32;
        let mut popup_major = 2. * BORDER_WIDTH as f32
            + overflow_cnt.min(8) as f32 * applet_size_unit as f32
            + 2. * padding
            + (overflow_cnt.min(8).saturating_sub(1) as f32) * space;
        let mut popup_cross = 2. * BORDER_WIDTH as f32
            + (overflow_cnt as f32 / 8.).ceil().min(1.0) * applet_size_unit as f32
            + 2. * padding;
        if !self.config.is_horizontal() {
            std::mem::swap(&mut popup_major, &mut popup_cross);
        }
        let popup = overflow_space
            .elements()
            .find(|e| {
                if let PopupMappedInternal::Popup(b) = e {
                    b.with_program(|p| {
                        &p.id
                            == match section {
                                OverflowSection::Left => &self.left_overflow_popup_id,
                                OverflowSection::Center => &self.center_overflow_popup_id,
                                OverflowSection::Right => &self.right_overflow_popup_id,
                            }
                    })
                } else {
                    false
                }
            })
            .cloned();
        let new_popup = |count| {
            PopupMappedInternal::Popup(overflow_popup_element(
                match section {
                    OverflowSection::Left => self.left_overflow_popup_id.clone(),
                    OverflowSection::Center => self.center_overflow_popup_id.clone(),
                    OverflowSection::Right => self.right_overflow_popup_id.clone(),
                },
                popup_major,
                popup_cross,
                self.loop_handle.clone(),
                self.colors.theme.clone(),
                self.space.id(),
                count,
            ))
        };

        let count = overflow_space.elements().count();
        if let Some(overflow_popup) = popup {
            let e = new_popup(count);
            let output = self.output.as_ref().map(|o| &o.1).unwrap();

            e.output_enter(output, Default::default());
            overflow_space.unmap_elem(&overflow_popup);
            overflow_space.map_element(e, (0, 0), false);
        } else {
            let output = self.output.as_ref().map(|o| &o.1).unwrap();
            let new_popup = new_popup(count);
            new_popup.output_enter(output, Default::default());
            overflow_space.map_element(new_popup, (0, 0), false);

            self.is_dirty = true;
            self.space.refresh();
        }

        if self.space.elements().all(|e| !matches!(e, CosmicMappedInternal::OverflowButton(_))) {
            let overflow_button_loc = (0, 0);
            let id = match section {
                OverflowSection::Left => self.left_overflow_button_id.clone(),
                OverflowSection::Center => self.center_overflow_button_id.clone(),
                OverflowSection::Right => self.right_overflow_button_id.clone(),
            };

            let icon_size = self.config.size.get_applet_icon_size(true);
            let padding = self.config.size.get_applet_padding(true);
            let icon = if self.config.is_horizontal() {
                "view-more-horizontal-symbolic"
            } else {
                "view-more-symbolic"
            };
            let e = overflow_button_element(
                id,
                (0, 0).into(),
                u16::try_from(icon_size).unwrap_or(32),
                (padding as f32).into(),
                Arc::new(AtomicBool::new(false)),
                icon.into(),
                self.loop_handle.clone(),
                self.colors.theme.clone(),
                self.space.id(),
            );
            let output = self.output.as_ref().map(|o| &o.1).unwrap();
            e.output_enter(output, Default::default());
            self.space.map_element(
                CosmicMappedInternal::OverflowButton(e),
                overflow_button_loc,
                false,
            );
            self.space.refresh();
            self.is_dirty = true;
        }
        overflow
    }

    fn move_from_overflow(
        mut extra_space: u32,
        is_horizontal: bool,
        space: &mut Space<CosmicMappedInternal>,
        overflow_space: &mut Space<PopupMappedInternal>,
        suggested_size: u32,
    ) -> u32 {
        // TODO move applets until extra_space is as close as possible to 0
        let overflow_elements = overflow_space.elements().cloned().collect_vec();
        for w in overflow_elements {
            if extra_space < suggested_size {
                break;
            }
            let size: Size<i32, _> = w.bbox().size;

            let applet_len = if is_horizontal { size.w as u32 } else { size.h as u32 };
            if extra_space >= applet_len {
                let w = match w {
                    PopupMappedInternal::Window(w) => w,
                    _ => continue,
                };
                extra_space = extra_space.saturating_sub(applet_len);
                overflow_space.unmap_elem(&PopupMappedInternal::Window(w.clone()));
                overflow_space.refresh();
                space.map_element(CosmicMappedInternal::Window(w.clone()), (0, 0), false);
                space.refresh();
                if let Some(t) = w.toplevel() {
                    t.with_pending_state(|s| {
                        s.size = None;
                        s.bounds = None;
                    });
                    t.send_pending_configure();
                }
            }
        }

        extra_space
    }

    fn relax_overflow_left(
        &mut self,
        extra_space: u32,
        left_overflow_button: &mut Option<IcedElement<OverflowButton>>,
    ) {
        let left = self.clients_left.lock().unwrap();
        let mut clients = self.shrinkable_clients(left.iter());
        drop(left);
        let suggested_size = self.config.size.get_applet_icon_size(true)
            + self.config.size.get_applet_padding(true) as u32 * 2;
        if clients.shrinkable_is_relaxed(self.config.is_horizontal(), self.scale) {
            Self::move_from_overflow(
                extra_space,
                self.config.is_horizontal(),
                &mut self.space,
                &mut self.overflow_left,
                suggested_size,
            );
            if self.overflow_left.elements().all(|e| matches!(e, PopupMappedInternal::Popup(_))) {
                if let Some(overflow_button) = left_overflow_button.take() {
                    self.space.unmap_elem(&CosmicMappedInternal::OverflowButton(overflow_button));
                    self.space.refresh();
                }
            }
        } else if extra_space > suggested_size {
            self.relax_overflow_clients(&mut clients, extra_space);
        }
    }

    fn relax_overflow_center(
        &mut self,
        extra_space: u32,
        center_overflow_button: &mut Option<IcedElement<OverflowButton>>,
    ) {
        let center: MutexGuard<Vec<PanelClient>> = self.clients_center.lock().unwrap();
        let mut clients = self.shrinkable_clients(center.iter());
        drop(center);
        if clients.shrinkable_is_relaxed(self.config.is_horizontal(), self.scale) {
            let suggested_size = self.config.size.get_applet_icon_size(true)
                + self.config.size.get_applet_padding(true) as u32 * 2;
            Self::move_from_overflow(
                extra_space,
                self.config.is_horizontal(),
                &mut self.space,
                &mut self.overflow_center,
                suggested_size,
            );
            if self.overflow_center.elements().all(|e| matches!(e, PopupMappedInternal::Popup(_))) {
                if let Some(overflow_button) = center_overflow_button.take() {
                    self.space.unmap_elem(&CosmicMappedInternal::OverflowButton(overflow_button));
                    self.space.refresh();
                }
            }
        } else {
            self.relax_overflow_clients(&mut clients, extra_space);
        }
    }

    pub(crate) fn relax_all(&mut self) {
        let mut left_overflow_button = None;
        let mut right_overflow_button = None;
        let mut center_overflow_button = None;

        for w in self.space.elements().cloned() {
            match w {
                CosmicMappedInternal::OverflowButton(b)
                    if overflow_button::with_id(&b, |id| &self.left_overflow_button_id == id) =>
                {
                    left_overflow_button = Some(b);
                },
                CosmicMappedInternal::OverflowButton(b)
                    if overflow_button::with_id(&b, |id| &self.center_overflow_button_id == id) =>
                {
                    center_overflow_button = Some(b);
                },
                CosmicMappedInternal::OverflowButton(b)
                    if overflow_button::with_id(&b, |id| &self.right_overflow_button_id == id) =>
                {
                    right_overflow_button = Some(b);
                },
                _ => {},
            };
        }
        let suggested_size = self.config.size.get_applet_icon_size(true)
            + self.config.size.get_applet_padding(true) as u32 * 2;
        self.relax_overflow_left(u32::MAX, &mut left_overflow_button);
        self.relax_overflow_center(u32::MAX, &mut center_overflow_button);
        self.relax_overflow_right(u32::MAX, &mut right_overflow_button);
        PanelSpace::move_from_overflow(
            u32::MAX,
            self.config.is_horizontal(),
            &mut self.space,
            &mut self.overflow_left,
            suggested_size,
        );
        PanelSpace::move_from_overflow(
            u32::MAX,
            self.config.is_horizontal(),
            &mut self.space,
            &mut self.overflow_center,
            suggested_size,
        );
        PanelSpace::move_from_overflow(
            u32::MAX,
            self.config.is_horizontal(),
            &mut self.space,
            &mut self.overflow_right,
            suggested_size,
        );
    }
    fn relax_overflow_right(
        &mut self,
        extra_space: u32,
        right_overflow_button: &mut Option<IcedElement<OverflowButton>>,
    ) {
        let right = self.clients_right.lock().unwrap();
        let mut clients = self.shrinkable_clients(right.iter());

        if clients.shrinkable_is_relaxed(self.config.is_horizontal(), self.scale) {
            let suggested_size = self.config.size.get_applet_icon_size(true)
                + self.config.size.get_applet_padding(true) as u32 * 2;
            Self::move_from_overflow(
                extra_space,
                self.config.is_horizontal(),
                &mut self.space,
                &mut self.overflow_right,
                suggested_size,
            );
            if self.overflow_right.elements().all(|e| matches!(e, PopupMappedInternal::Popup(_))) {
                if let Some(overflow_button) = right_overflow_button.take() {
                    self.space.unmap_elem(&CosmicMappedInternal::OverflowButton(overflow_button));
                    self.space.refresh();
                }
            }
        } else {
            self.relax_overflow_clients(&mut clients, extra_space);
        }
    }

    fn relax_overflow_clients(&self, clients: &mut OverflowClientPartition, mut extra_space: u32) {
        if self.remap_attempts > 0 {
            return;
        }
        for (w, ..) in
            clients.constrained_shrinkables(self.config.is_horizontal(), self.scale).drain(..).rev()
        {
            let expand = extra_space as i32;
            tracing::info!("Relaxing overflow client by {expand}, {:?}", w.bbox().size);
            if extra_space == 0 {
                tracing::info!("No more space to relax");
                break;
            }

            let Some(t) = w.toplevel() else {
                continue;
            };

            let is_horizontal: bool = self.config.is_horizontal();

            let skip = t.with_pending_state(|state| {
                state.size = None;
                if let Some(size) = &mut state.bounds {
                    info!("Old size: {:?}", size);
                    if is_horizontal {
                        size.h = 0;
                        if size.w != 0 {
                            size.w = size.w.saturating_add(expand);
                        } else {
                            return true;
                        }
                    } else {
                        size.w = 0;
                        if size.h != 0 {
                            size.h = size.h.saturating_add(expand);
                        } else {
                            return true;
                        }
                    }

                    info!("New size: {:?}", state.size);
                    false
                } else {
                    true
                }
            });
            t.send_pending_configure();
            if !skip {
                extra_space = extra_space.saturating_sub(expand as u32);
            }
        }
    }

    /// Send frame callback to hidden applets
    pub fn update_hidden_applet_frame(&mut self) {
        let Some(output) = self.output.as_ref().map(|o| o.1.clone()) else {
            return;
        };

        for w in self
            .overflow_left
            .elements()
            .chain(self.overflow_center.elements())
            .chain(self.overflow_right.elements())
        {
            let output_clone = output.clone();
            if let PopupMappedInternal::Window(w) = w {
                w.send_frame(&output, Duration::from_secs(1), None, |_, _| {
                    Some(output_clone.clone())
                });
                w.refresh();
                self.is_dirty = true;
            }
        }
    }
}

// if middle collides with left or right, it must be constrained
// middle cant be constrained below 1/3 of the size of the output.
// If the left or right extends past the min(1/3, middle), then the left or
// right must be constrained. If there is no middle, then left and right must
// each be constrained to no less than 1/2. This is unlikely to happen.

// middle constraint must go in priority order
// applets with higher priority must be constrained first.
// can't be constrained to be smaller than min(configured panel size suggested
// applet icon size * requested min, cur_applet size).

// applets that don't offer a priority are constrained last, and don't shrink,
// but instead are moved to the overflow popup. applets that are in the overflow
// popup are not constrained, but are instead moved to the overflow popup space.

// If after all constraints are applied, the panel is still too small, then the
// panel will move the offending applet to overflow.

// When there is more space available in a section, the applets in the overflow
// popup will be moved back to the panel. If there is still space in the panel,
// then the constrained applets that shrink should be unconstrained

// panels will now have up to 4 spaces.
// they can have nested popups in a common use case now too.
// overflow buttons go in the original space.

#[derive(Debug, Clone, Copy)]
pub enum OverflowSection {
    Left,
    Center,
    Right,
}

#[derive(Debug, Default, Clone)]
pub struct OverflowClientPartition {
    /// windows for clients that can be shrunk, but not moved to the overflow
    /// popup
    pub(crate) shrinkable: Vec<(Window, i32, ClientShrinkSize)>,
    /// windows for clients that can be moved to the overflow popup, but not
    /// shrunk
    pub(crate) movable: Vec<(Window, u32)>,
    pub suggested_size: u32,
}

impl OverflowClientPartition {
    fn constrained_shrinkables(
        &self,
        is_horizontal: bool,
        scale: f64,
    ) -> Vec<(Window, i32, ClientShrinkSize)> {
        self.shrinkable
            .iter()
            .filter(|(w, ..)| {
                w.toplevel().is_some_and(|t| {
                    let state = t.current_state();
                    let cur_size = w.bbox().size;
                    if is_horizontal {
                        state.bounds.is_none()
                            || state.bounds.is_some_and(|s| {
                                s.w != 0
                                    || cur_size.w.saturating_sub((s.w as f64 * scale) as i32)
                                        > self.suggested_size as i32
                            })
                    } else {
                        state.bounds.is_none()
                            || state.bounds.is_some_and(|s| {
                                s.h != 0
                                    || cur_size.h.saturating_sub((s.h as f64 * scale) as i32)
                                        > self.suggested_size as i32
                            })
                    }
                })
            })
            .cloned()
            .collect_vec()
    }

    fn shrinkable_is_relaxed(&self, is_horizontal: bool, scale: f64) -> bool {
        self.shrinkable.is_empty() || {
            self.shrinkable.iter().all(|(w, ..)| {
                w.toplevel().is_some_and(|t| {
                    let state = t.current_state();
                    let cur_size = w.bbox().size;
                    if is_horizontal {
                        state.bounds.is_none()
                            || state.bounds.is_some_and(|s| {
                                s.w == 0
                                    || cur_size
                                        .w
                                        .saturating_sub((s.w as f64 * scale).round() as i32)
                                        > self.suggested_size as i32
                            })
                    } else {
                        state.bounds.is_none()
                            || state.bounds.is_some_and(|s| {
                                s.h == 0
                                    || cur_size
                                        .h
                                        .saturating_sub((s.h as f64 * scale).round() as i32)
                                        > self.suggested_size as i32
                            })
                    }
                })
            })
        }
    }
}
