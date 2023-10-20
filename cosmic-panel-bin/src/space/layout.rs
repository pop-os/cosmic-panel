use crate::space::Alignment;

use super::PanelSpace;
use cosmic_panel_config::PanelAnchor;
use image::RgbaImage;
use itertools::{chain, Itertools};
use sctk::shell::WaylandSurface;
use smithay::utils::{IsAlive, Physical, Size};
use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    desktop::Window,
    reexports::wayland_server::Resource,
    utils::{Point, Rectangle, Transform},
};

impl PanelSpace {
    pub(crate) fn layout(&mut self) -> anyhow::Result<()> {
        self.space.refresh();
        let padding_u32 = self.config.padding() as u32;
        let padding_scaled = padding_u32 as f64 * self.scale;
        let anchor = self.config.anchor();
        let spacing_u32 = self.config.spacing() as u32;
        let spacing_scaled = spacing_u32 as f64 * self.scale;
        // First try partitioning the panel evenly into N spaces.
        // If all windows fit into each space, then set their offsets and return.
        let (list_length, list_thickness, actual_length) = match anchor {
            PanelAnchor::Left | PanelAnchor::Right => {
                (self.dimensions.h, self.dimensions.w, self.actual_size.h)
            }
            PanelAnchor::Top | PanelAnchor::Bottom => {
                (self.dimensions.w, self.dimensions.h, self.actual_size.w)
            }
        };
        let is_dock = !self.config.expand_to_edges();

        let mut num_lists = 0;
        if !is_dock && self.config.plugins_wings.is_some() {
            num_lists += 2;
        }
        if self.config.plugins_center.is_some() {
            num_lists += 1;
        }

        let mut windows_right = self
            .space
            .elements()
            .cloned()
            .filter(|w| w.alive())
            .filter_map(|w| {
                self.clients_right
                    .iter()
                    .enumerate()
                    .find_map(|(i, (_, c, _))| {
                        if Some(c.id()) == w.toplevel().wl_surface().client().map(|c| c.id()) {
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
            .elements()
            .cloned()
            .filter(|w| w.alive())
            .filter_map(|w| {
                self.clients_center
                    .iter()
                    .enumerate()
                    .find_map(|(i, (_, c, _))| {
                        if Some(c.id()) == w.toplevel().wl_surface().client().map(|c| c.id()) {
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
            .elements()
            .cloned()
            .filter(|w| w.alive())
            .filter_map(|w| {
                self.clients_left
                    .iter()
                    .enumerate()
                    .find_map(|(i, (_, c, _))| {
                        if Some(c.id()) == w.toplevel().wl_surface().client().map(|c| c.id()) {
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
            + spacing_scaled * (windows_center.len().max(1) as f64 - 1.0);

        let right = windows_right
            .iter()
            .map(|e| map_fn(e, anchor, Alignment::Right, self.scale));

        let right_sum_scaled = right.clone().map(|(_, _, length, _)| length).sum::<i32>() as f64
            + spacing_scaled * (windows_right.len().max(1) as f64 - 1.0);

        let total_sum_scaled = left_sum_scaled + center_sum_scaled + right_sum_scaled;
        let new_list_length = (total_sum_scaled as f64
            + padding_scaled * 2.0
            + spacing_scaled * (num_lists as f64 - 1.0)) as i32;
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

        let (new_logical_length, new_logical_thickness) = if self.config.is_horizontal() {
            (self.actual_size.w, self.actual_size.h)
        } else {
            (self.actual_size.h, self.actual_size.w)
        };
        let mut new_dim = if self.config.is_horizontal() {
            let mut dim = self.actual_size;
            dim.h += self.config.get_effective_anchor_gap() as i32;
            dim
        } else {
            let mut dim = self.actual_size;
            dim.w += self.config.get_effective_anchor_gap() as i32;
            dim
        };
        new_dim = self.constrain_dim(new_dim);
        // update input region of panel when list changes
        if old_actual != self.actual_size && is_dock {
            let (input_region, layer) = match (self.input_region.as_ref(), self.layer.as_ref()) {
                (Some(r), Some(layer)) => (r, layer),
                _ => anyhow::bail!("Missing input region or layer!"),
            };

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

        let (new_list_dim_length, new_list_thickness_dim) = match anchor {
            PanelAnchor::Left | PanelAnchor::Right => (new_dim.h, new_dim.w),
            PanelAnchor::Top | PanelAnchor::Bottom => (new_dim.w, new_dim.h),
        };

        if new_list_dim_length != list_length as i32 || new_list_thickness_dim != list_thickness {
            self.pending_dimensions = Some(new_dim);
            self.is_dirty = true;
            anyhow::bail!("resizing list");
        }

        // must use logical coordinates for layout here

        fn center_in_bar(thickness: u32, dim: u32) -> i32 {
            (thickness as i32 - dim as i32) / 2
        }

        let left_sum = left_sum_scaled / self.scale;
        let center_sum = center_sum_scaled / self.scale;
        let right_sum = right_sum_scaled / self.scale;
        let total_sum = total_sum_scaled / self.scale;

        let requested_eq_length: f64 = new_list_dim_length as f64 / num_lists as f64;
        let (right_sum, center_offset) = if is_dock {
            (
                0.0,
                padding_u32 as f64 + (new_list_dim_length - new_logical_length) as f64 / 2.0,
            )
        } else if left_sum < requested_eq_length as f64
            && center_sum < requested_eq_length as f64
            && right_sum < requested_eq_length as f64
        {
            let center_padding = (requested_eq_length as f64 - center_sum) / 2.0;
            (
                right_sum,
                requested_eq_length + padding_u32 as f64 + center_padding,
            )
        } else {
            let center_padding = (new_list_dim_length as f64 - total_sum) / 2.0;

            (right_sum, (left_sum + padding_u32 as f64 + center_padding))
        };

        let mut prev: f64 = padding_u32 as f64;

        // offset for centering
        let margin_offset = match anchor {
            PanelAnchor::Top | PanelAnchor::Left => self.config.get_effective_anchor_gap(),
            PanelAnchor::Bottom | PanelAnchor::Right => 0,
        } as i32;

        for (i, w) in &mut windows_left.iter_mut() {
            // XXX this is a hack to get the logical size of the window
            // TODO improve how this is done
            let size = w.bbox().size.to_f64().downscale(self.scale);

            let cur: f64 = prev + spacing_u32 as f64 * *i as f64;
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
                    prev += size.h as f64;
                    self.space
                        .map_element(w.clone(), (cur.0 as i32, cur.1 as i32), false);
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
                    prev += size.w as f64;
                    self.space
                        .map_element(w.clone(), (cur.0 as i32, cur.1 as i32), false);
                }
            };
        }

        let mut prev: f64 = center_offset;
        for (i, w) in &mut windows_center.iter_mut() {
            // XXX this is a hack to get the logical size of the window
            let size = w.bbox().size.to_f64().downscale(self.scale);

            let cur = prev + spacing_u32 as f64 * *i as f64;
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
                    prev += size.h as f64;
                    self.space
                        .map_element(w.clone(), (cur.0 as i32, cur.1 as i32), false);
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
                    prev += size.w as f64;
                    self.space
                        .map_element(w.clone(), (cur.0 as i32, cur.1 as i32), false);
                }
            };
        }

        // twice padding is subtracted
        let mut prev: f64 = if is_dock {
            prev
        } else {
            list_length as f64 - padding_u32 as f64 - right_sum
        };

        for (i, w) in &mut windows_right.iter_mut() {
            let size = w.bbox().size.to_f64().downscale(self.scale);
            let cur = prev + spacing_u32 as f64 * *i as f64;
            match anchor {
                PanelAnchor::Left | PanelAnchor::Right => {
                    let cur = (
                        margin_offset
                            + center_in_bar(new_list_thickness.try_into().unwrap(), size.w as u32),
                        cur,
                    );
                    prev += size.h as f64;
                    self.space
                        .map_element(w.clone(), (cur.0 as i32, cur.1 as i32), false);
                }
                PanelAnchor::Top | PanelAnchor::Bottom => {
                    let cur = (
                        cur,
                        margin_offset
                            + center_in_bar(new_list_thickness.try_into().unwrap(), size.h as u32),
                    );
                    prev += size.w as f64;
                    self.space
                        .map_element(w.clone(), (cur.0 as i32, cur.1 as i32), false);
                }
            };
        }
        self.space.refresh();
        if self.actual_size.w > 0
            && self.actual_size.h > 0
            && actual_length > 0
            && (self.config.border_radius > 0 || self.config.get_effective_anchor_gap() > 0)
        {
            // corners calculation with border_radius

            // default to actual size of the panel
            let mut panel_size = self
                .actual_size
                .to_f64()
                .to_physical(self.scale)
                .to_i32_round();

            // adjust the length if the panel extends to edges
            if !is_dock {
                if self.config.is_horizontal() {
                    panel_size.w = self
                        .dimensions
                        .to_f64()
                        .to_physical(self.scale)
                        .to_i32_round()
                        .w;
                } else {
                    panel_size.h = self
                        .dimensions
                        .to_f64()
                        .to_physical(self.scale)
                        .to_i32_round()
                        .h;
                }
            }

            let mut buff = MemoryRenderBuffer::new(
                Fourcc::Abgr8888,
                (panel_size.w, panel_size.h),
                1,
                Transform::Normal,
                None,
            );
            let mut render_context = buff.render();
            let bg_color = self
                .bg_color
                .iter()
                .map(|c| ((c * 255.0) as u8).clamp(0, 255))
                .collect_vec();
            let _ = render_context.draw(|buffer| {
                buffer.chunks_exact_mut(4).for_each(|chunk| {
                    chunk.copy_from_slice(&bg_color);
                });

                let radius = (self.config.border_radius as f64 * self.scale).round() as u32;
                let radius = radius
                    .min(panel_size.w as u32 / 2)
                    .min(panel_size.h as u32 / 2);

                // early return if no radius
                if radius == 0 {
                    return Result::<_, ()>::Ok(vec![Rectangle::from_loc_and_size(
                        Point::default(),
                        (panel_size.w, panel_size.h),
                    )]);
                }
                let drawn_radius = 128;
                let drawn_radius2 = drawn_radius as f64 * drawn_radius as f64;
                let grid = (0..((drawn_radius + 1) * (drawn_radius + 1)))
                    .into_iter()
                    .map(|i| {
                        let (x, y) = (i as u32 % (drawn_radius + 1), i as u32 / (drawn_radius + 1));
                        drawn_radius2 - (x as f64 * x as f64 + y as f64 * y as f64)
                    })
                    .collect_vec();

                let bg_color: [u8; 4] = self
                    .bg_color
                    .iter()
                    .map(|c| ((c * 255.0) as u8).clamp(0, 255))
                    .collect_vec()
                    .try_into()
                    .unwrap();
                let empty = [0, 0, 0, 0];

                let mut corner_image = RgbaImage::new(drawn_radius, drawn_radius);
                for i in 0..(drawn_radius * drawn_radius) {
                    let (x, y) = (i as u32 / drawn_radius, i as u32 % drawn_radius);
                    let bottom_left = grid[(y * (drawn_radius + 1) + x) as usize];
                    let bottom_right = grid[(y * (drawn_radius + 1) + x + 1) as usize];
                    let top_left = grid[((y + 1) * (drawn_radius + 1) + x) as usize];
                    let top_right = grid[((y + 1) * (drawn_radius + 1) + x + 1) as usize];
                    let color = if bottom_left >= 0.0
                        && bottom_right >= 0.0
                        && top_left >= 0.0
                        && top_right >= 0.0
                    {
                        bg_color.clone()
                    } else {
                        empty
                    };
                    corner_image.put_pixel(x, y, image::Rgba(color));
                }
                let corner_image = image::imageops::resize(
                    &corner_image,
                    radius as u32,
                    radius as u32,
                    image::imageops::FilterType::CatmullRom,
                );

                for (i, color) in corner_image.pixels().enumerate() {
                    let (x, y) = (i as u32 % radius, i as u32 / radius);
                    let top_left = (radius - 1 - x, radius - 1 - y);
                    let top_right = (panel_size.w as u32 - radius + x, radius - 1 - y);
                    let bottom_left = (radius - 1 - x, panel_size.h as u32 - radius + y);
                    let bottom_right = (
                        panel_size.w as u32 - radius + x,
                        panel_size.h as u32 - radius + y,
                    );
                    for (c_x, c_y) in match (self.config.anchor, self.config.anchor_gap) {
                        (PanelAnchor::Left, false) => vec![top_right, bottom_right],
                        (PanelAnchor::Right, false) => vec![top_left, bottom_left],
                        (PanelAnchor::Top, false) => vec![bottom_left, bottom_right],
                        (PanelAnchor::Bottom, false) => vec![top_left, top_right],
                        _ => vec![top_left, top_right, bottom_left, bottom_right],
                    } {
                        let b_i = (c_y * panel_size.w as u32 + c_x) as usize * 4;
                        let c = buffer.get_mut(b_i..b_i + 4).unwrap();
                        c.copy_from_slice(&color.0);
                    }
                }

                // Return the whole buffer as damage
                Result::<_, ()>::Ok(vec![Rectangle::from_loc_and_size(
                    Point::default(),
                    (panel_size.w, panel_size.h),
                )])
            });
            drop(render_context);
            let old = self.buffer.replace(buff);
            self.old_buff = old;
            self.buffer_changed = true;
        }

        Ok(())
    }
}
