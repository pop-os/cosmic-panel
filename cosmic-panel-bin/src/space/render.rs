use std::time::Duration;

use super::{panel_space::MyRenderElements, PanelSpace};
use cctk::wayland_client::{Proxy, QueueHandle};
use cosmic_panel_config::PanelAnchor;
use itertools::Itertools;
use sctk::shell::WaylandSurface;
use smithay::{
    backend::renderer::{
        damage::OutputDamageTracker,
        element::{
            memory::MemoryRenderBufferRenderElement,
            surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
        },
        gles::GlesRenderer,
        Bind, Frame, Renderer, Unbind,
    },
    utils::{Logical, Point, Rectangle, Size},
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
        let bg_color = self.bg_color();

        if self.is_dirty && self.has_frame {
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
                        let mut render_context = buff.render();
                        let margin_offset = match self.config.anchor {
                            PanelAnchor::Top | PanelAnchor::Left => {
                                self.config.get_effective_anchor_gap() as f64
                            }
                            PanelAnchor::Bottom | PanelAnchor::Right => 0.0,
                        };

                        let (panel_size, loc) = if let Some(animate_state) =
                            self.animate_state.as_ref()
                        {
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
                            let crosswise_length = if self.config.is_horizontal() {
                                self.actual_size.h
                            } else {
                                self.actual_size.w
                            };
                            let (x, y, width, height) = if self.config.is_horizontal() {
                                (
                                    lengthwise_pos,
                                    crosswise_pos,
                                    container_length,
                                    crosswise_length,
                                )
                            } else {
                                (
                                    crosswise_pos,
                                    lengthwise_pos,
                                    crosswise_length,
                                    container_length,
                                )
                            };
                            (
                                Size::<i32, Logical>::from((width, height)),
                                Point::<f64, Logical>::from((x, y)),
                            )
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

                            (self.actual_size, loc)
                        } else {
                            let loc: Point<f64, Logical> = if self.config.is_horizontal() {
                                (0.0, margin_offset)
                            } else {
                                (margin_offset, 0.0)
                            }
                            .into();

                            (self.dimensions, loc)
                        };
                        let scaled_panel_size =
                            panel_size.to_f64().to_physical(self.scale).to_i32_round();

                        let _ = render_context.draw(|_| {
                            if self.buffer_changed {
                                Result::<_, ()>::Ok(vec![Rectangle::from_loc_and_size(
                                    Point::default(),
                                    (scaled_panel_size.w, scaled_panel_size.h),
                                )])
                            } else {
                                Result::<_, ()>::Ok(Default::default())
                            }
                        });
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
}
