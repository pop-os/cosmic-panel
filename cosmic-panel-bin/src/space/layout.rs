use std::slice::IterMut;

use crate::minimize::MinimizeApplet;
use crate::space::corner_element::RoundedRectangleSettings;
use crate::space::Alignment;

use super::PanelSpace;
use cosmic_panel_config::PanelAnchor;
use itertools::{chain, Itertools};
use sctk::shell::WaylandSurface;
use smithay::utils::{IsAlive, Physical, Size};
use smithay::{desktop::Window, reexports::wayland_server::Resource, utils::Rectangle};

impl PanelSpace {
    pub(crate) fn layout(&mut self) -> anyhow::Result<()> {
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
        let list_thickness = match anchor {
            PanelAnchor::Left | PanelAnchor::Right => self.dimensions.w,
            PanelAnchor::Top | PanelAnchor::Bottom => self.dimensions.h,
        };
        let is_dock = !self.config.expand_to_edges();

        let mut num_lists = 0;
        if self
            .config
            .plugins_wings
            .as_ref()
            .map(|l| l.0.len() + l.1.len())
            .unwrap_or_default()
            > 0
        {
            num_lists += 2;
        }
        if self
            .config
            .plugins_center
            .as_ref()
            .map(|l| l.len())
            .unwrap_or_default()
            > 0
        {
            num_lists += 1;
        }

        let make_indices_contiguous = |windows: &mut Vec<(usize, Window, Option<u32>)>| {
            windows.sort_by(|(a_i, _, _), (b_i, _, _)| a_i.cmp(b_i));
            for (j, (i, _, _)) in windows.iter_mut().enumerate() {
                *i = j;
            }
        };
        let mut to_map: Vec<Window> = Vec::with_capacity(self.space.elements().count());
        // must handle unmapped windows, and unmap windows that are too large for the current configuration.
        let to_unmap = self
            .space
            .elements()
            .cloned()
            .filter(|w| {
                if !w.alive() {
                    return true;
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
                        "Window {size:?} is too large for what panel configuration allows {constrained:?}. It will be unmapped.",
                    );
                } else {
                    to_map.push(w.clone());
                }
                unmap
            })
            .collect_vec();
        for w in self.unmapped.drain(..).collect_vec() {
            if w.alive() && {
                let size = w.bbox().size.to_f64().downscale(self.scale).to_i32_round();

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
        for w in to_unmap {
            self.space.unmap_elem(&w);
            self.unmapped.push(w);
        }

        self.space.refresh();

        let mut windows_right = to_map
            .iter()
            .cloned()
            .filter_map(|w| {
                self.clients_right
                    .lock()
                    .unwrap()
                    .iter()
                    .enumerate()
                    .find_map(|(i, c)| {
                        if Some(c.client.id()) == w.toplevel().wl_surface().client().map(|c| c.id())
                        {
                            Some((i, w.clone(), c.minimize_priority))
                        } else {
                            None
                        }
                    })
            })
            .collect_vec();
        make_indices_contiguous(&mut windows_right);

        let mut windows_center = to_map
            .iter()
            .cloned()
            .filter_map(|w| {
                self.clients_center
                    .lock()
                    .unwrap()
                    .iter()
                    .enumerate()
                    .find_map(|(i, c)| {
                        if Some(c.client.id()) == w.toplevel().wl_surface().client().map(|c| c.id())
                        {
                            Some((i, w.clone(), c.minimize_priority))
                        } else {
                            None
                        }
                    })
            })
            .collect_vec();
        make_indices_contiguous(&mut windows_center);
        let mut windows_left = to_map
            .iter()
            .cloned()
            .filter_map(|w| {
                self.clients_left
                    .lock()
                    .unwrap()
                    .iter()
                    .enumerate()
                    .find_map(|(i, c)| {
                        if Some(c.client.id()) == w.toplevel().wl_surface().client().map(|c| c.id())
                        {
                            Some((i, w.clone(), c.minimize_priority))
                        } else {
                            None
                        }
                    })
            })
            .collect_vec();
        make_indices_contiguous(&mut windows_left);

        fn map_fn(
            (i, w, _): &(usize, Window, Option<u32>),
            anchor: PanelAnchor,
            alignment: Alignment,
            _scale: f64,
        ) -> (Alignment, usize, i32, i32) {
            let bbox = w.bbox().size;

            match anchor {
                PanelAnchor::Left | PanelAnchor::Right => (alignment, *i, bbox.h, bbox.w),
                PanelAnchor::Top | PanelAnchor::Bottom => (alignment, *i, bbox.w, bbox.h),
            }
        }

        let left = windows_left
            .iter()
            .map(|e| map_fn(e, anchor, Alignment::Left, self.scale));
        let left_sum_scaled = left.clone().map(|(_, _, length, _)| length).sum::<i32>() as f64
            + spacing_scaled as f64 * (windows_left.len().max(1) as f64 - 1.0);

        let center = windows_center
            .iter()
            .map(|e| map_fn(e, anchor, Alignment::Center, self.scale));
        let center_sum_scaled = center.clone().map(|(_, _, length, _)| length).sum::<i32>() as f64
            + spacing_scaled * (windows_center.len().max(1) - 1) as f64;

        let right = windows_right
            .iter()
            .map(|e| map_fn(e, anchor, Alignment::Right, self.scale));

        let right_sum_scaled = right.clone().map(|(_, _, length, _)| length).sum::<i32>() as f64
            + spacing_scaled * (windows_right.len().max(1) - 1) as f64;

        let total_sum_scaled = left_sum_scaled + center_sum_scaled + right_sum_scaled;
        let new_list_length = (total_sum_scaled as f64
            + padding_scaled * 2.0
            + spacing_scaled * (num_lists - 1) as f64) as i32;
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
        // update input region of panel when list changes
        let (input_region, layer) = match (self.input_region.as_ref(), self.layer.as_ref()) {
            (Some(r), Some(layer)) => (r, layer),
            _ => panic!("input region or layer missing"),
        };

        let (new_list_dim_length, new_list_thickness_dim) = match anchor {
            PanelAnchor::Left | PanelAnchor::Right => (new_dim.h, new_dim.w),
            PanelAnchor::Top | PanelAnchor::Bottom => (new_dim.w, new_dim.h),
        };

        self.panel_changed |= old_actual != self.actual_size
            || new_list_thickness_dim != list_thickness
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
            layer
                .wl_surface()
                .set_input_region(Some(input_region.wl_region()));
        }

        // must use logical coordinates for layout here

        fn center_in_bar(crosswise_dim: u32, dim: u32) -> i32 {
            (crosswise_dim as i32 - dim as i32) / 2
        }
        // eq length should assign space evenly to all lists even if they are empty
        let requested_eq_length: f64 = container_length as f64 / 3.;
        let center_left_spacing = if left_sum < requested_eq_length as f64
            && center_sum < requested_eq_length as f64
            && right_sum < requested_eq_length as f64
        {
            let center_spacing = (requested_eq_length as f64 - center_sum) / 2.0;
            let left_spacing = requested_eq_length as f64 - left_sum - padding_u32 as f64;
            left_spacing + center_spacing
        } else {
            (container_length as f64 - left_sum - center_sum - right_sum - 2. * padding_u32 as f64)
                as f64
                / 2.
        };
        if new_list_thickness_dim != list_thickness {
            self.pending_dimensions = Some(new_dim);
            self.is_dirty = true;
            anyhow::bail!("resizing list");
        }
        // offset for centering
        let margin_offset = match anchor {
            PanelAnchor::Top | PanelAnchor::Left => gap,
            PanelAnchor::Bottom | PanelAnchor::Right => 0,
        } as i32;
        let mut map_windows = |windows: IterMut<'_, (usize, Window, Option<u32>)>,
                               mut prev|
         -> f64 {
            for (i, w, minimize_priority) in windows {
                // XXX this is a hack to get the logical size of the window
                // TODO improve how this is done
                let size = w.bbox().size.to_f64().downscale(self.scale);

                let cur: f64 = prev + spacing_u32 as f64 * *i as f64;
                let (x, y);
                match anchor {
                    PanelAnchor::Left | PanelAnchor::Right => {
                        let cur = (
                            margin_offset
                                + center_in_bar(
                                    new_logical_thickness.try_into().unwrap(),
                                    size.w as u32,
                                ),
                            cur,
                        );
                        (x, y) = (cur.0 as i32, cur.1 as i32);
                        prev += size.h as f64;
                        self.space.map_element(w.clone(), (x, y), false);
                    }
                    PanelAnchor::Top | PanelAnchor::Bottom => {
                        let cur = (
                            cur,
                            margin_offset
                                + center_in_bar(
                                    new_logical_thickness.try_into().unwrap(),
                                    size.h as u32,
                                ),
                        );
                        (x, y) = (cur.0 as i32, cur.1 as i32);
                        prev += size.w as f64;
                        self.space.map_element(w.clone(), (x, y), false);
                    }
                };
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
        let mut prev: f64 = container_lengthwise_pos as f64 + padding_u32 as f64;

        prev = map_windows(windows_left.iter_mut(), prev);
        // will be already offset if dock
        prev += center_left_spacing;
        map_windows(windows_center.iter_mut(), prev);

        let prev = container_lengthwise_pos as f64 + container_length as f64
            - padding_u32 as f64
            - right_sum;

        map_windows(windows_right.iter_mut(), prev);
        self.space.refresh();

        Ok(())
    }
}
