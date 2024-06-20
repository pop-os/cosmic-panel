use std::{
    slice::IterMut,
    sync::{atomic::AtomicBool, Arc, MutexGuard},
};

use crate::{
    iced::elements::{
        overflow_button::{self, overflow_button_element, OverflowButtonElement},
        CosmicMappedInternal,
    },
    minimize::MinimizeApplet,
    space::{corner_element::RoundedRectangleSettings, Alignment},
};

use super::{panel_space::PanelClient, PanelSpace};
use crate::xdg_shell_wrapper::space::WrapperSpace;
use anyhow::bail;
use cosmic::iced::id;
use cosmic_panel_config::PanelAnchor;
use itertools::{chain, Itertools};
use once_cell::sync::Lazy;
use sctk::shell::WaylandSurface;
use smithay::{
    desktop::{space::SpaceElement, Space, Window},
    reexports::wayland_server::Resource,
    utils::{IsAlive, Physical, Rectangle, Size},
};

static LEFT_BTN: Lazy<id::Id> = Lazy::new(|| id::Id::new("LEFT_OVERFLOW_BTN"));
static CENTER_BTN: Lazy<id::Id> = Lazy::new(|| id::Id::new("CENTER_OVERFLOW_BTN"));
static RIGHT_BTN: Lazy<id::Id> = Lazy::new(|| id::Id::new("RIGHT_OVERFLOW_BTN"));

impl PanelSpace {
    pub(crate) fn layout_(&mut self) -> anyhow::Result<()> {
        let gap = self.gap();

        let make_indices_contiguous = |windows: &mut Vec<(usize, Window, Option<u32>)>| {
            windows.sort_by(|(a_i, ..), (b_i, ..)| a_i.cmp(b_i));
            for (j, (i, ..)) in windows.iter_mut().enumerate() {
                *i = j;
            }
        };
        let mut to_map: Vec<Window> = Vec::with_capacity(self.space.elements().count());
        // must handle unmapped windows, and unmap windows that are too large for the
        // current configuration.

        let mut left_overflow_button = None;
        let mut right_overflow_button = None;

        let to_unmap = self
            .space
            .elements()
            .cloned()
            .filter_map(|w| {
                let w = match w {
                    CosmicMappedInternal::Window(w) => w,
                    CosmicMappedInternal::OverflowButton(b)
                        if overflow_button::with_id(&b, |id| {
                            Lazy::get(&LEFT_BTN).is_some_and(|left_id| left_id == id)
                        }) =>
                    {
                        left_overflow_button = Some(b);
                        return None;
                    },
                    CosmicMappedInternal::OverflowButton(b)
                        if overflow_button::with_id(&b, |id| {
                            Lazy::get(&CENTER_BTN).is_some_and(|center_id| center_id == id)
                        }) =>
                    {
                        return None;
                    },
                    CosmicMappedInternal::OverflowButton(b)
                        if overflow_button::with_id(&b, |id| {
                            Lazy::get(&RIGHT_BTN).is_some_and(|right_id| right_id == id)
                        }) =>
                    {
                        right_overflow_button = Some(b);
                        return None;
                    },
                    _ => return None,
                };

                if !w.alive() {
                    return Some(w);
                }
                let size = w.bbox().size.to_f64().downscale(self.scale).to_i32_round();

                let constrained = self.constrain_dim(size, Some(gap as u32));
                let unmap = if self.config.is_horizontal() {
                    constrained.h < size.h
                } else {
                    constrained.w < size.w
                };
                if unmap {
                    tracing::error!(
                        "Window {size:?} is too large for what panel configuration allows \
                         {constrained:?}. It will be unmapped.",
                    );
                } else {
                    to_map.push(w.clone());
                }
                unmap.then_some(w)
            })
            .collect_vec();
        for w in self.unmapped.drain(..).collect_vec() {
            let size = w.bbox().size.to_f64().downscale(self.scale).to_i32_round();

            if w.alive() && {
                let constrained = self.constrain_dim(size, Some(gap as u32));
                if self.config.is_horizontal() {
                    constrained.h >= size.h
                } else {
                    constrained.w >= size.w
                }
            } {
                to_map.push(w);
            } else {
                tracing::trace!("Window was unmapped and will stay so. {:?}", w.bbox());
                self.unmapped.push(w);
            }
        }
        // HACK temporarily avoid unmapping windows when changing scale
        if to_unmap.len() > 0 && self.scale_change_retries == 0 {
            for w in to_unmap {
                self.space.unmap_elem(&CosmicMappedInternal::Window(w.clone()));
                self.unmapped.push(w);
            }
        } else {
            self.scale_change_retries -= 1;
        }

        self.space.refresh();
        let is_dock = !self.config.expand_to_edges()
            || self.animate_state.as_ref().is_some_and(|a| !(a.cur.expanded > 0.5));
        let mut windows_left = to_map
            .iter()
            .cloned()
            .filter_map(|w| {
                self.clients_left.lock().unwrap().iter().enumerate().find_map(|(i, c)| {
                    let Some(t) = w.toplevel() else {
                        return None;
                    };

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
                self.clients_center.lock().unwrap().iter().enumerate().find_map(|(i, c)| {
                    let Some(t) = w.toplevel() else {
                        return None;
                    };
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
                self.clients_right.lock().unwrap().iter().enumerate().find_map(|(i, c)| {
                    let Some(t) = w.toplevel() else {
                        return None;
                    };
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
        self.layout(
            windows_left,
            windows_center,
            windows_right,
            left_overflow_button,
            right_overflow_button,
        )
    }

    pub(crate) fn layout(
        &mut self,
        mut windows_left: Vec<(usize, Window, Option<u32>)>,
        mut windows_center: Vec<(usize, Window, Option<u32>)>,
        mut windows_right: Vec<(usize, Window, Option<u32>)>,
        left_overflow_button: Option<OverflowButtonElement>,
        right_overflow_button: Option<OverflowButtonElement>,
    ) -> anyhow::Result<()> {
        self.space.refresh();
        let mut bg_color = self.bg_color();
        for c in 0..3 {
            bg_color[c] *= bg_color[3];
        }
        let gap = self.gap();
        let padding_u32 = self.config.padding() as u32;
        let padding_scaled = padding_u32 as f64 * self.scale;
        let anchor = self.config.anchor();
        let spacing_u32 = self.config.spacing() as u32;
        let spacing_scaled = spacing_u32 as f64 * self.scale;
        // First try partitioning the panel evenly into N spaces.
        // If all windows fit into each space, then set their offsets and return.
        let (list_cross, layer_major) = match anchor {
            PanelAnchor::Left | PanelAnchor::Right => (self.dimensions.w, self.dimensions.h),
            PanelAnchor::Top | PanelAnchor::Bottom => (self.dimensions.h, self.dimensions.w),
        };
        let is_dock = !self.config.expand_to_edges();

        let mut num_lists: u32 = 0;
        if windows_left.len() + windows_right.len() > 0 {
            num_lists += 2;
        }
        if !windows_center.is_empty() {
            num_lists += 1;
        }

        fn map_fn(
            (i, w, _): &(usize, Window, Option<u32>),
            anchor: PanelAnchor,
            alignment: Alignment,
            scale: f64,
        ) -> (Alignment, usize, i32, i32) {
            let mut bbox = w.bbox().size;
            let constrained_bbox = w
                .toplevel()
                .and_then(|t| t.current_state().size)
                .map(|s| s.to_f64().upscale(scale).to_i32_ceil())
                .unwrap_or_else(|| w.bbox().size);
            if constrained_bbox.w > 0 {
                bbox.w = constrained_bbox.w;
            }
            if constrained_bbox.h > 0 {
                bbox.h = constrained_bbox.h;
            }

            match anchor {
                PanelAnchor::Left | PanelAnchor::Right => (alignment, *i, bbox.h, bbox.w),
                PanelAnchor::Top | PanelAnchor::Bottom => (alignment, *i, bbox.w, bbox.h),
            }
        }

        let left = windows_left.iter().map(|e| map_fn(e, anchor, Alignment::Left, self.scale));

        let left_sum_scaled = left.clone().map(|(_, _, length, _)| length).sum::<i32>() as f64
            + spacing_scaled as f64 * windows_left.len().saturating_sub(1) as f64;
        let left_sum_scaled = if let Some(left_button) = left_overflow_button.as_ref() {
            let size = left_button.bbox().size.to_f64().upscale(self.scale);
            left_sum_scaled
                + if self.config.is_horizontal() { size.w } else { size.h } as f64
                + spacing_scaled
        } else {
            left_sum_scaled
        };

        let center =
            windows_center.iter().map(|e| map_fn(e, anchor, Alignment::Center, self.scale));
        let center_sum_scaled = center.clone().map(|(_, _, length, _)| length).sum::<i32>() as f64
            + spacing_scaled * windows_center.len().saturating_sub(1) as f64;

        let right = windows_right.iter().map(|e| map_fn(e, anchor, Alignment::Right, self.scale));
        let right_sum_scaled = right.clone().map(|(_, _, length, _)| length).sum::<i32>() as f64
            + spacing_scaled * windows_right.len().saturating_sub(1) as f64;
        let right_sum_scaled = if let Some(right_button) = right_overflow_button.as_ref() {
            let size = right_button.bbox().size.to_f64().upscale(self.scale);
            right_sum_scaled
                + if self.config.is_horizontal() { size.w } else { size.h } as f64
                + spacing_scaled
        } else {
            right_sum_scaled
        };

        let total_sum_scaled = left_sum_scaled + center_sum_scaled + right_sum_scaled;
        let new_list_length = (total_sum_scaled as f64
            + padding_scaled * 2.0
            + spacing_scaled * num_lists.saturating_sub(1) as f64)
            as i32;
        let new_list_thickness = (2.0 * padding_scaled
            + chain!(left.clone(), center.clone(), right.clone())
                .map(|(_, _, _, thickness)| thickness)
                .max()
                .unwrap_or(0) as f64) as i32;
        let old_actual = self.actual_size;

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

        let (new_logical_length, new_logical_thickness) = if self.config.is_horizontal() {
            (self.actual_size.w, self.actual_size.h)
        } else {
            (self.actual_size.h, self.actual_size.w)
        };
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

        self.panel_changed |= old_actual != self.actual_size
            || new_list_thickness_dim != list_cross
            || self.animate_state.is_some();

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

        let center_pos = layer_major as f64 / 2. - center_sum / 2.;
        let left_pos = container_lengthwise_pos as f64 + padding_u32 as f64;

        let mut right_pos = container_lengthwise_pos as f64 + container_length as f64
            - padding_u32 as f64
            - right_sum;
        let one_third = (layer_major as f64 - (spacing_u32 * num_lists.saturating_sub(1)) as f64)
            / (3.min(num_lists) as f64);
        let one_half = layer_major as f64 / (2.min(num_lists) as f64);
        let larger_side = left_sum.max(right_sum);

        let target_center_len = (layer_major as f64 - spacing_u32 as f64 * 2. - larger_side * (2.))
            .max(one_third)
            .min(layer_major as f64);

        let target_left_len = if windows_center.is_empty() {
            (layer_major as f64 - right_sum.min(one_half) - spacing_u32 as f64).max(one_half)
        } else {
            (one_half - target_center_len.min(center_sum) / 2.).max(one_third)
        }
        .min(layer_major as f64);

        let target_right_len = if windows_center.is_empty() {
            (layer_major as f64 - left_sum.min(one_half) - spacing_u32 as f64).max(one_half)
        } else {
            (one_half - target_center_len.min(center_sum) / 2.).max(one_third)
        }
        .min(layer_major as f64);

        let center_overflow = (center_sum - target_center_len) as i32;
        if center_overflow <= 0 {
            // check if it can be expanded
            self.relax_overflow_center(center_overflow.abs() as u32)
        } else if center_overflow > 0 {
            dbg!(center_sum, target_center_len);

            let overflow = self.shrink_center((center_sum - target_center_len) as u32);
            bail!("overflow: {}", overflow)
        }

        let left_overflow = (left_sum - target_left_len) as i32;
        if left_overflow <= 0 {
            // check if it can be expanded
            self.relax_overflow_left(left_overflow.abs() as u32);
        } else if left_overflow > 0 {
            let overflow = self.shrink_left(left_overflow as u32);
            bail!("overflow: {}", overflow)
        }

        let right_overflow = (right_sum - target_right_len) as i32;
        if right_overflow <= 0 {
            // check if it can be expanded
            self.relax_overflow_right(right_overflow.abs() as u32);
        } else {
            let overflow = self.shrink_right(right_overflow as u32);
            bail!("overflow: {}", overflow)
        }

        // update input region of panel when list changes
        let (input_region, layer) = match (self.input_region.as_ref(), self.layer.as_ref()) {
            (Some(r), Some(layer)) => (r, layer),
            _ => panic!("input region or layer missing"),
        };
        if self.panel_changed {
            {
                let gap = self.gap() as f64 * self.scale;
                let border_radius = self.border_radius() as f64 * self.scale;

                let mut panel_size = self.actual_size.to_f64().to_physical(self.scale);
                let container_length = self.container_length as f64 * self.scale;
                let container_lengthwise_pos = container_lengthwise_pos as f32 * self.scale as f32;
                if self.config.is_horizontal() {
                    panel_size.w = container_length;
                } else {
                    panel_size.h = container_length;
                }

                let border_radius = border_radius.min(panel_size.w / 2.).min(panel_size.h / 2.);
                let (rad_tl, rad_tr, rad_bl, rad_br) = match (self.config.anchor, self.gap()) {
                    (PanelAnchor::Right, 0) => (border_radius, 0., border_radius, 0.),
                    (PanelAnchor::Left, 0) => (0., border_radius, 0., border_radius),
                    (PanelAnchor::Bottom, 0) => (border_radius, border_radius, 0., 0.),
                    (PanelAnchor::Top, 0) => (0., 0., border_radius, border_radius),
                    _ => (border_radius, border_radius, border_radius, border_radius),
                };
                let loc = match self.config.anchor {
                    PanelAnchor::Left => [gap as f32, container_lengthwise_pos as f32],
                    PanelAnchor::Right => [0., container_lengthwise_pos as f32],
                    PanelAnchor::Top => [container_lengthwise_pos as f32, 0.],
                    PanelAnchor::Bottom => [container_lengthwise_pos as f32, gap as f32],
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
            }

            input_region.subtract(
                0,
                0,
                self.dimensions.w.max(new_dim.w),
                self.dimensions.h.max(new_dim.h),
            );

            if is_dock {
                let (layer_length, actual_length) = if self.config.is_horizontal() {
                    (new_dim.w, self.actual_size.w)
                } else {
                    (new_dim.h, self.actual_size.h)
                };
                let side = (layer_length as u32 - actual_length as u32) / 2;

                let (loc, size) = if self.config.is_horizontal() {
                    ((side as i32, 0), (self.actual_size.w, new_dim.h))
                } else {
                    ((0, side as i32), (new_dim.w, self.actual_size.h))
                };

                input_region.add(loc.0, loc.1, size.0, size.1);
            } else {
                input_region.add(0, 0, new_dim.w, new_dim.h);
            }
            layer.wl_surface().set_input_region(Some(input_region.wl_region()));
        }

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
            let size = right_button.bbox().size.to_f64().upscale(self.scale);

            let crosswise_pos = if self.config.is_horizontal() {
                margin_offset
                    + center_in_bar(new_logical_thickness.try_into().unwrap(), size.w as u32)
            } else {
                margin_offset
                    + center_in_bar(new_logical_thickness.try_into().unwrap(), size.h as u32)
            };
            let loc = if self.config().is_horizontal() {
                (right_pos.round() as i32, crosswise_pos)
            } else {
                (crosswise_pos, right_pos.round() as i32)
            };
            right_pos += size.h as f64 + spacing_u32 as f64;
            self.space.map_element(CosmicMappedInternal::OverflowButton(right_button), loc, true);
        };

        let mut map_windows = |windows: IterMut<'_, (usize, Window, Option<u32>)>,
                               mut prev|
         -> f64 {
            for (_, w, minimize_priority) in windows {
                // XXX this is a hack to get the logical size of the window
                // TODO improve how this is done
                let bbox = w.bbox().size.to_f64().downscale(self.scale);
                let size = w
                    .toplevel()
                    .and_then(|t| t.current_state().size)
                    .map(|s| {
                        let mut ret = s.to_f64();
                        if s.w == 0 {
                            ret.w = bbox.w;
                        }
                        if s.h == 0 {
                            ret.h = bbox.h;
                        }

                        ret
                    })
                    .unwrap_or_else(|| bbox);

                let cur: f64 = prev;
                let (x, y);

                if self.config.is_horizontal() {
                    let cur = (
                        cur,
                        margin_offset
                            + center_in_bar(
                                new_logical_thickness.try_into().unwrap(),
                                size.h as u32,
                            ),
                    );
                    (x, y) = (cur.0 as i32, cur.1 as i32);
                    prev += size.w as f64 + spacing_u32 as f64;
                    self.space.map_element(CosmicMappedInternal::Window(w.clone()), (x, y), false);
                } else {
                    let cur = (
                        margin_offset
                            + center_in_bar(
                                new_logical_thickness.try_into().unwrap(),
                                size.w as u32,
                            ),
                        cur,
                    );
                    (x, y) = (cur.0 as i32, cur.1 as i32);
                    prev += size.h as f64 + spacing_u32 as f64;
                    self.space.map_element(CosmicMappedInternal::Window(w.clone()), (x, y), false);
                }
                if minimize_priority.is_some() {
                    let new_rect = Rectangle {
                        loc: (x, y).into(),
                        size: ((size.w.ceil() as i32).max(1), (size.w.ceil() as i32).max(1)).into(),
                    };
                    if new_rect != self.minimize_applet_rect {
                        self.minimize_applet_rect = new_rect;
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
        let left_pos = map_windows(windows_left.iter_mut(), left_pos as f64);

        // will be already offset if dock
        map_windows(windows_center.iter_mut(), center_pos as f64);

        map_windows(windows_right.iter_mut(), right_pos as f64);
        // if there is a left overflow_button, map it
        if let Some(left_button) = left_overflow_button {
            let size = left_button.bbox().size.to_f64().upscale(self.scale);

            let crosswise_pos = if self.config.is_horizontal() {
                margin_offset
                    + center_in_bar(new_logical_thickness.try_into().unwrap(), size.w as u32)
            } else {
                margin_offset
                    + center_in_bar(new_logical_thickness.try_into().unwrap(), size.h as u32)
            };
            let loc = if self.config().is_horizontal() {
                (left_pos.round() as i32, crosswise_pos)
            } else {
                (crosswise_pos, left_pos.round() as i32)
            };
            self.space.map_element(CosmicMappedInternal::OverflowButton(left_button), loc, false);
        }
        self.space.refresh();

        Ok(())
    }

    fn shrinkable_clients<'a>(
        &self,
        clients: impl Iterator<Item = &'a PanelClient>,
    ) -> OverflowClientPartition {
        let mut overflow_partition = OverflowClientPartition::default();
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
                    Some((w.clone(), c.minimize_priority.unwrap_or_default()))
                } else {
                    return None;
                }
            }) else {
                // tracing::warn!("Client not found in space {:?}", c.name);
                continue;
            };
            if c.shrink_min_size.is_some_and(|s| s > 0) {
                overflow_partition.shrinkable.push((w.0, w.1, c.shrink_min_size.unwrap()));
            } else {
                overflow_partition.movable.push(w);
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
        self.shrink_clients(overflow, &mut clients, OverflowSection::Left)
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
        self.shrink_clients(overflow, &mut clients, OverflowSection::Center)
    }

    fn shrink_right(&mut self, overflow: u32) -> u32 {
        let right = self.clients_right.lock().unwrap();
        let mut clients = self.shrinkable_clients(right.iter());
        drop(right);
        self.shrink_clients(overflow, &mut clients, OverflowSection::Right)
    }

    fn shrink_clients(
        &mut self,
        mut overflow: u32,
        clients: &mut OverflowClientPartition,
        section: OverflowSection,
    ) -> u32 {
        let mut i = 0;
        let shrinkable = &mut clients.shrinkable;
        let unit_size = self.config.size.get_applet_icon_size_with_padding(true);
        // dbg!(overflow, shrinkable.len(), clients.movable.len());
        while overflow > 0 && i < shrinkable.len() {
            let (w, _, min_units) = &mut shrinkable[i];
            let size = w.bbox().size.to_f64().downscale(self.scale).to_i32_round();
            let major_dim = if self.config.is_horizontal() { size.w } else { size.h };
            let unit_size = (major_dim as f32 / unit_size as f32).ceil() as u32;
            let new_dim = (major_dim as u32).saturating_sub(overflow).max(*min_units * unit_size);
            let diff = (major_dim as u32).saturating_sub(new_dim);
            if diff == 0 {
                i += 1;
                continue;
            }

            if let Some(t) = w.toplevel() {
                t.with_pending_state(|s| {
                    if self.config.is_horizontal() {
                        s.size = Some((new_dim as i32, size.h).into());
                    } else {
                        s.size = Some((size.w, new_dim as i32).into());
                    }
                });
                t.send_pending_configure();
                overflow = overflow.saturating_sub(diff);
            }
            i += 1;
        }
        if overflow > 0 {
            return self.move_to_overflow(
                overflow,
                self.config.is_horizontal(),
                clients.clone(),
                section,
            );
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
        let overflow_0 = overflow;
        let overflow_space = match section {
            OverflowSection::Left => &mut self.overflow_left,
            OverflowSection::Center => &mut self.overflow_center,
            OverflowSection::Right => &mut self.overflow_right,
        };
        let mut overflow_cnt = overflow_space.elements().count();
        let had_overflow_prev = overflow_cnt > 0;
        let applet_size_unit = self.config.size.get_applet_icon_size(true)
            + 2 * self.config.size.get_applet_padding(true) as u32;
        if overflow_cnt == 0 {
            overflow += applet_size_unit;
        }
        let space = &mut self.space;
        // TODO move applets until overflow is <= 0
        for w in clients.movable {
            if overflow == 0 {
                break;
            }
            overflow_cnt += 1;
            let diff =
                if is_horizontal { w.0.bbox().size.w as u32 } else { w.0.bbox().size.h as u32 };
            overflow = overflow.saturating_sub(diff);
            let x = (overflow_cnt % 8) as i32 * applet_size_unit as i32;
            let y = (overflow_cnt / 8) as i32 * applet_size_unit as i32;

            space.unmap_elem(&CosmicMappedInternal::Window(w.0.clone()));
            // Rows of 8 with configured applet size
            if let Some(t) = w.0.toplevel() {
                t.with_pending_state(|s| {
                    s.size = Some((applet_size_unit as i32, applet_size_unit as i32).into());
                });
                t.send_pending_configure();
            }
            overflow_space.map_element(w.0, (x, y), false);
        }

        if !had_overflow_prev && overflow != overflow_0 {
            // TODO use a borrowed bool for selected
            // XXX the location will be adjusted later so this is ok
            let overflow_button_loc = (0, 0);
            let id = match section {
                OverflowSection::Left => Lazy::force(&LEFT_BTN).clone(),
                OverflowSection::Center => Lazy::force(&CENTER_BTN).clone(),
                OverflowSection::Right => Lazy::force(&RIGHT_BTN).clone(),
            };
            // if there was no overflow before, and there is now, then we need to add the
            // overflow button

            let icon_size = self.config.size.get_applet_icon_size(true);
            let padding = self.config.size.get_applet_padding(true);
            let icon = if self.config.is_horizontal() {
                "view-more-horizontal-symbolic"
            } else {
                "view-more-symbolic"
            };
            self.space.map_element(
                CosmicMappedInternal::OverflowButton(overflow_button_element(
                    id,
                    (0, 0).into(),
                    u16::try_from(icon_size).unwrap_or(32),
                    (padding as f32).into(),
                    Arc::new(AtomicBool::new(false)),
                    icon.into(),
                    self.loop_handle.clone(),
                    self.colors.theme.clone(),
                )),
                overflow_button_loc,
                false,
            )
        }

        overflow
    }

    fn move_from_overflow(
        mut extra_space: u32,
        is_horizontal: bool,
        space: &mut Space<CosmicMappedInternal>,
        overflow_space: &mut Space<Window>,
        suggested_size: u32,
    ) -> u32 {
        // TODO move applets until extra_space is as close as possible to 0
        let mut overflow_cnt = overflow_space.elements().count();
        while overflow_cnt > 0 && extra_space > suggested_size {
            overflow_cnt -= 1;

            let w = overflow_space.elements().next().unwrap().clone();
            let diff = if is_horizontal { w.bbox().size.w as u32 } else { w.bbox().size.h as u32 };
            if extra_space >= diff {
                extra_space = extra_space.saturating_sub(diff);
                overflow_space.unmap_elem(&w);
                space.map_element(CosmicMappedInternal::Window(w), (0, 0), false);
            }
        }
        extra_space
    }

    fn relax_overflow_left(&mut self, extra_space: u32) {
        let left = self.clients_left.lock().unwrap();
        let mut clients = self.shrinkable_clients(left.iter());
        drop(left);
        if clients.shrinkable_is_relaxed(self.config.is_horizontal()) {
            self.relax_overflow_clients(&mut clients);
        } else {
            let suggested_size = self.config.size.get_applet_icon_size(true) as u32
                + self.config.size.get_applet_padding(true) as u32 * 2;
            Self::move_from_overflow(
                extra_space,
                self.config.is_horizontal(),
                &mut self.space,
                &mut self.overflow_left,
                suggested_size,
            );
        }
    }

    fn relax_overflow_center(&mut self, extra_space: u32) {
        let center: MutexGuard<Vec<PanelClient>> = self.clients_center.lock().unwrap();
        let mut clients = self.shrinkable_clients(center.iter());
        drop(center);
        if clients.shrinkable_is_relaxed(self.config.is_horizontal()) {
            self.relax_overflow_clients(&mut clients);
        } else {
            let suggested_size = self.config.size.get_applet_icon_size(true) as u32
                + self.config.size.get_applet_padding(true) as u32 * 2;
            Self::move_from_overflow(
                extra_space,
                self.config.is_horizontal(),
                &mut self.space,
                &mut self.overflow_left,
                suggested_size,
            );
        }
    }

    fn relax_overflow_right(&mut self, extra_space: u32) {
        let right = self.clients_right.lock().unwrap();
        let mut clients = self.shrinkable_clients(right.iter());
        drop(right);
        if clients.shrinkable_is_relaxed(self.config.is_horizontal()) {
            self.relax_overflow_clients(&mut clients);
        } else {
            let suggested_size = self.config.size.get_applet_icon_size(true) as u32
                + self.config.size.get_applet_padding(true) as u32 * 2;
            Self::move_from_overflow(
                extra_space,
                self.config.is_horizontal(),
                &mut self.space,
                &mut self.overflow_left,
                suggested_size,
            );
        }
    }

    fn relax_overflow_clients(&self, clients: &mut OverflowClientPartition) {
        for (w, ..) in clients.shrinkable.drain(..) {
            let Some(t) = w.toplevel() else {
                continue;
            };
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
    pub(crate) shrinkable: Vec<(Window, u32, u32)>,
    /// windows for clients that can be moved to the overflow popup, but not
    /// shrunk
    pub(crate) movable: Vec<(Window, u32)>,
}

impl OverflowClientPartition {
    fn shrinkable_is_relaxed(&self, is_horizontal: bool) -> bool {
        self.shrinkable.is_empty() || {
            self.shrinkable.iter().all(|(w, ..)| {
                w.toplevel().is_some_and(|t| {
                    t.with_pending_state(|s| {
                        if is_horizontal {
                            s.size.map(|s| s.w as u32).unwrap_or_default().saturating_sub(1) == 0
                        } else {
                            s.size.map(|s| s.h as u32).unwrap_or_default().saturating_sub(1) == 0
                        }
                    })
                })
            })
        }
    }
}
