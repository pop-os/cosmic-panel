use std::time::Duration;

use super::{panel_space::MyRenderElements, PanelSpace};
use cctk::wayland_client::{Proxy, QueueHandle};
use cosmic_panel_config::PanelAnchor;
use image::RgbaImage;
use itertools::Itertools;
use sctk::shell::WaylandSurface;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            damage::OutputDamageTracker,
            element::{
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
            },
            gles::GlesRenderer,
            Bind, Frame, Renderer, Unbind,
        },
    },
    utils::{Logical, Point, Rectangle, Transform},
};
use xdg_shell_wrapper::{shared_state::GlobalState, space::WrapperSpace};

impl PanelSpace {
    pub(crate) fn render<W: WrapperSpace>(
        &mut self,
        renderer: &mut GlesRenderer,
        time: u32,
        qh: &QueueHandle<GlobalState<W>>,
    ) -> anyhow::Result<()> {
        if self.space_event.get() != None
            || self.first_draw && (self.actual_size.w <= 20 || self.actual_size.h <= 20)
        {
            return Ok(());
        }
        let mut bg_color = self.bg_color();
        for c in 0..3 {
            bg_color[c] *= bg_color[3];
        }

        if self.is_dirty && self.has_frame {
            if let Err(err) = self.render_panel(bg_color) {
                tracing::error!(?err, "Error rendering the panel.");
            }
            let my_renderer = match self.damage_tracked_renderer.as_mut() {
                Some(r) => r,
                None => return Ok(()),
            };
            renderer.unbind()?;
            renderer.bind(self.egl_surface.as_ref().unwrap().clone())?;
            let is_dock = !self.config.expand_to_edges();
            let clear_color = if self.buffer.is_none() {
                &bg_color
            } else {
                &[0.0, 0.0, 0.0, 0.0]
            };
            // if not visible, just clear and exit early
            let not_visible = self.config.autohide.is_some()
                && matches!(
                    self.visibility,
                    xdg_shell_wrapper::space::Visibility::Hidden
                );

            // TODO check to make sure this is not going to cause damage issues
            if not_visible {
                let dim = self
                    .dimensions
                    .to_f64()
                    .to_physical(self.scale)
                    .to_i32_round();

                if let Ok(mut frame) = renderer.render(dim, smithay::utils::Transform::Normal) {
                    _ = frame.clear(
                        [0.0, 0.0, 0.0, 0.0],
                        &[Rectangle::from_loc_and_size((0, 0), dim)],
                    );
                    if let Ok(sync_point) = frame.finish() {
                        sync_point.wait();
                        self.egl_surface.as_ref().unwrap().swap_buffers(None)?;
                    }
                    let wl_surface = self.layer.as_ref().unwrap().wl_surface();
                    wl_surface.frame(qh, wl_surface.clone());
                    wl_surface.commit();
                    // reset the damage tracker
                    *my_renderer =
                        OutputDamageTracker::new(dim, 1.0, smithay::utils::Transform::Flipped180);
                    self.is_dirty = false;
                }

                renderer.unbind()?;
                return Ok(());
            }

            if let Some((o, _info)) = &self.output.as_ref().map(|(_, o, info)| (o, info)) {
                let mut elements: Vec<MyRenderElements<_>> = self
                    .space
                    .elements()
                    .map(|w| {
                        let loc = self
                            .space
                            .element_location(w)
                            .unwrap_or_default()
                            .to_f64()
                            .to_physical(self.scale)
                            .to_i32_round();
                        render_elements_from_surface_tree(
                            renderer,
                            w.toplevel().wl_surface(),
                            loc,
                            1.0,
                            1.0,
                            smithay::backend::renderer::element::Kind::Unspecified,
                        )
                        .into_iter()
                        .map(|r| MyRenderElements::WaylandSurface(r))
                    })
                    .flatten()
                    .collect_vec();

                // FIXME the first draw is stretched even when not scaled when using a buffer
                // this is a workaround
                if !self.first_draw {
                    if let Some(buff) = self.buffer.as_mut() {
                        let render_context = buff.render();
                        let margin_offset = match self.config.anchor {
                            PanelAnchor::Top | PanelAnchor::Left => {
                                self.config.get_effective_anchor_gap() as f64
                            }
                            PanelAnchor::Bottom | PanelAnchor::Right => 0.0,
                        };

                        let loc = if let Some(animate_state) = self.animate_state.as_ref() {
                            let actual_length = if self.config.is_horizontal() {
                                self.actual_size.w
                            } else {
                                self.actual_size.h
                            };
                            let dim_length = if self.config.is_horizontal() {
                                self.dimensions.w
                            } else {
                                self.dimensions.h
                            };
                            let container_length = (actual_length as f32
                                + (dim_length - actual_length) as f32 * animate_state.cur.expanded)
                                as i32;

                            let lengthwise_pos = (dim_length - container_length) as f64 / 2.0;

                            let crosswise_pos = match self.config.anchor {
                                PanelAnchor::Top | PanelAnchor::Left => {
                                    self.config.get_effective_anchor_gap() as f64
                                }
                                PanelAnchor::Bottom | PanelAnchor::Right => 0.0,
                            };

                            let (x, y) = if self.config.is_horizontal() {
                                (lengthwise_pos, crosswise_pos)
                            } else {
                                (crosswise_pos, lengthwise_pos)
                            };
                            Point::<f64, Logical>::from((x, y))
                        } else if is_dock {
                            let loc: Point<f64, Logical> = if self.config.is_horizontal() {
                                (
                                    ((self.dimensions.w - self.actual_size.w) as f64 / 2.0).round(),
                                    margin_offset,
                                )
                            } else {
                                (
                                    margin_offset,
                                    ((self.dimensions.h - self.actual_size.h) as f64 / 2.0).round(),
                                )
                            }
                            .into();

                            loc
                        } else {
                            let loc: Point<f64, Logical> = if self.config.is_horizontal() {
                                (0.0, margin_offset)
                            } else {
                                (margin_offset, 0.0)
                            }
                            .into();

                            loc
                        };

                        self.buffer_changed = false;

                        drop(render_context);
                        if let Ok(render_element) = MemoryRenderBufferRenderElement::from_buffer(
                            renderer,
                            loc.to_physical(self.scale).to_i32_round(),
                            &buff,
                            None,
                            None,
                            None,
                            smithay::backend::renderer::element::Kind::Unspecified,
                        ) {
                            elements.push(MyRenderElements::Memory(render_element));
                        }
                    }
                }

                _ = my_renderer.render_output(
                    renderer,
                    self.egl_surface
                        .as_ref()
                        .unwrap()
                        .buffer_age()
                        .unwrap_or_default() as usize,
                    &elements,
                    *clear_color,
                );
                self.egl_surface.as_ref().unwrap().swap_buffers(None)?;

                for window in self.space.elements() {
                    let output = o.clone();
                    window.send_frame(o, Duration::from_millis(time as u64), None, move |_, _| {
                        Some(output.clone())
                    });
                }
                let wl_surface = self.layer.as_ref().unwrap().wl_surface().clone();
                wl_surface.frame(qh, wl_surface.clone());
                wl_surface.commit();

                self.is_dirty = false;
                self.has_frame = false;

                // TODO clear the stencil buffer
            }
        }

        let clear_color = [0.0, 0.0, 0.0, 0.0];
        // TODO Popup rendering optimization
        for p in self.popups.iter_mut().filter(|p| {
            p.dirty
                && p.egl_surface.is_some()
                && p.state.is_none()
                && p.s_surface.alive()
                && p.c_popup.wl_surface().is_alive()
                && p.has_frame
        }) {
            renderer.unbind()?;
            renderer.bind(p.egl_surface.as_ref().unwrap().clone())?;

            let elements: Vec<WaylandSurfaceRenderElement<_>> = render_elements_from_surface_tree(
                renderer,
                p.s_surface.wl_surface(),
                (0, 0),
                1.0,
                1.0,
                smithay::backend::renderer::element::Kind::Unspecified,
            );
            p.damage_tracked_renderer.render_output(
                renderer,
                p.egl_surface
                    .as_ref()
                    .unwrap()
                    .buffer_age()
                    .unwrap_or_default() as usize,
                &elements,
                clear_color,
            )?;

            p.egl_surface.as_ref().unwrap().swap_buffers(None)?;

            let wl_surface = p.c_popup.wl_surface().clone();
            wl_surface.frame(qh, wl_surface.clone());
            wl_surface.commit();
            p.dirty = false;
            p.has_frame = false;
        }
        renderer.unbind()?;
        self.first_draw = false;

        Ok(())
    }

    fn render_panel(&mut self, bg_color: [f32; 4]) -> anyhow::Result<()> {
        if !self.buffer_changed {
            return Ok(());
        }

        let gap = self.gap();
        let border_radius = self.border_radius();
        let mut panel_size = self.actual_size;
        let container_length = self.container_length;

        if self.config.is_horizontal() {
            panel_size.w = container_length as i32;
        } else {
            panel_size.h = container_length as i32;
        }
        let panel_size = panel_size.to_f64().to_physical(self.scale).to_i32_round();

        let bg_color: [u8; 4] = bg_color
            .iter()
            .map(|c| ((c * 255.0) as u8).clamp(0, 255))
            .collect_vec()
            .try_into()
            .unwrap();

        let radius = (border_radius as f64 * self.scale).round() as u32;
        let radius = radius
            .min(panel_size.w as u32 / 2)
            .min(panel_size.h as u32 / 2);
        let mut calculated_corner_image = None;
        // TODO use 2 MemoryRenderBuffer for sides, and 1 single pixel buffer for center
        let mut buff = MemoryRenderBuffer::new(
            Fourcc::Abgr8888,
            (panel_size.w, panel_size.h),
            1,
            Transform::Normal,
            None,
        );

        let mut render_context = buff.render();
        let _ = render_context.draw(|buffer| {
            buffer.chunks_exact_mut(4).for_each(|chunk| {
                chunk.copy_from_slice(&bg_color);
            });

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
                for (c_x, c_y) in match (self.config.anchor, gap > 0) {
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

            calculated_corner_image = Some(corner_image);
            // Return the whole buffer as damage
            Result::<_, ()>::Ok(vec![Rectangle::from_loc_and_size(
                Point::default(),
                (panel_size.w, panel_size.h),
            )])
        });
        drop(render_context);
        self.buffer = Some(buff);
        self.buffer_changed = true;

        Ok(())
    }
}
