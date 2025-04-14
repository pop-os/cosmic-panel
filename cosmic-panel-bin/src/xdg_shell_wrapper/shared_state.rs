// SPDX-License-Identifier: MPL-2.0

use std::time::Duration;

use cctk::wayland_client::Proxy;
use itertools::Itertools;
use sctk::{reexports::client::protocol::wl_surface::WlSurface, shell::WaylandSurface};
use smithay::{
    backend::{
        input::KeyState,
        renderer::{
            damage::OutputDamageTracker,
            element::surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
            gles::GlesRenderer,
            Bind, ImportDma, ImportEgl,
        },
    },
    desktop::utils::send_frames_surface_tree,
    input::keyboard::FilterResult,
    reexports::wayland_server::DisplayHandle,
    utils::SERIAL_COUNTER,
    wayland::{
        compositor::with_states, dmabuf::DmabufState, fractional_scale::with_fractional_scale,
    },
};
use tracing::{error, info};

use crate::{
    space_container::SpaceContainer,
    xdg_shell_wrapper::{
        client_state::ClientState, server_state::ServerState, space::WrapperSpace,
    },
};

/// the  global state for the embedded server state
#[allow(missing_debug_implementations)]
pub struct GlobalState {
    /// the implemented space
    pub space: SpaceContainer,
    /// desktop client state
    pub client_state: ClientState,
    /// embedded server state
    pub server_state: ServerState,
    /// instant that the panel was started
    pub start_time: std::time::Instant,
    /// iter count
    pub iter_count: u128,
}

impl GlobalState {
    pub(crate) fn new(
        client_state: ClientState,
        server_state: ServerState,
        space: SpaceContainer,
        start_time: std::time::Instant,
    ) -> Self {
        Self { space, client_state, server_state, start_time, iter_count: 0 }
    }

    pub(crate) fn cleanup(&mut self) {
        // cleanup popup manager
        self.server_state.popup_manager.cleanup();

        // handle funky keyboard state.
        // if a client layer shell surface is closed, then it won't receive the release
        // event then the client will keep receiving input
        // so we send the release here instead
        let press = if let Some((key_pressed, kbd)) = self
            .client_state
            .last_key_pressed
            .iter()
            .position(|(_, _, layer_shell_wl_surface)| !layer_shell_wl_surface.is_alive())
            .and_then(|key_pressed| {
                self.server_state
                    .seats
                    .iter()
                    .find(|s| s.name == self.client_state.last_key_pressed[key_pressed].0)
                    .and_then(|s| {
                        s.server.seat.get_keyboard().map(|kbd| {
                            (self.client_state.last_key_pressed.remove(key_pressed), kbd)
                        })
                    })
            }) {
            Some((key_pressed, kbd))
        } else {
            None
        };
        if let Some((key_pressed, kbd)) = press {
            kbd.input::<(), _>(
                self,
                key_pressed.1 .0.into(),
                KeyState::Released,
                SERIAL_COUNTER.next_serial(),
                key_pressed.1 .1.wrapping_add(1),
                move |_, _modifiers, _keysym| FilterResult::Forward,
            );
        }
        self.space.cleanup();
    }

    /// set the scale factor for a surface
    /// this should be called when the scale factor of a surface changes
    pub fn scale_factor_changed(&mut self, surface: &WlSurface, scale_factor: f64, legacy: bool) {
        if legacy && self.client_state.fractional_scaling_manager.is_some() {
            return;
        }
        for tracked_surface in &mut self.client_state.proxied_layer_surfaces {
            if tracked_surface.3.wl_surface() == surface {
                if legacy {
                    surface.set_buffer_scale(scale_factor as i32);
                }
                tracked_surface.5 = scale_factor;
                let mut size = tracked_surface.2.bbox().size;
                size.w = size.w.max(1);
                size.h = size.h.max(1);
                let scaled_size = size.to_f64().to_physical_precise_round(scale_factor);

                tracked_surface.1 = OutputDamageTracker::new(
                    scaled_size,
                    scale_factor,
                    smithay::utils::Transform::Flipped180,
                );
                if let Some(viewport) = tracked_surface.7.as_ref() {
                    viewport.set_destination(size.w, size.h);
                }
                tracked_surface.0.resize(scaled_size.w.max(1), scaled_size.h.max(1), 0, 0);

                with_states(tracked_surface.2.wl_surface(), |states| {
                    with_fractional_scale(states, |fractional_scale| {
                        fractional_scale.set_preferred_scale(scale_factor);
                    });
                });
                return;
            }
        }

        self.space.scale_factor_changed(surface, scale_factor, legacy);
    }
}

impl GlobalState {
    /// bind the display for the space
    pub fn bind_display(&mut self, dh: &DisplayHandle) {
        if let Some(renderer) = self.space.renderer() {
            let res = renderer.bind_wl_display(dh);
            if let Err(err) = res {
                error!("{:?}", err);
            } else {
                let dmabuf_formats = renderer.dmabuf_formats().into_iter().collect_vec();
                let mut state = DmabufState::new();
                let global = state.create_global::<GlobalState>(dh, dmabuf_formats);
                self.server_state.dmabuf_state.replace((state, global));
            }
        }
    }

    /// draw the dnd icon if it exists and is ready
    pub fn draw_dnd_icon(&mut self) {
        // TODO proxied layer surfaces
        if let Some((
            (egl_surface, wl_surface, ref mut dmg_tracked_renderer, is_dirty, has_frame),
            s_icon,
        )) = self
            .server_state
            .seats
            .iter_mut()
            .find_map(|s| s.client.dnd_icon.as_mut().zip(s.server.dnd_icon.as_mut()))
        {
            if !*is_dirty || has_frame.is_none() {
                return;
            }
            let Some(egl_surface) = egl_surface.as_mut() else {
                return;
            };
            info!("draw_dnd_icon actually happening");

            let clear_color = &[0.0, 0.0, 0.0, 0.0];
            let renderer = match self.space.renderer() {
                Some(r) => r,
                None => {
                    error!("no renderer");
                    return;
                },
            };
            info!("draw_dnd_icon got renderer");
            _ = unsafe { renderer.egl_context().make_current_with_surface(egl_surface) };
            let age = egl_surface.buffer_age().unwrap_or_default() as usize;
            let Ok(mut f) = renderer.bind(egl_surface) else {
                return;
            };
            info!("draw_dnd_icon bound renderer");
            let elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                render_elements_from_surface_tree(
                    renderer,
                    s_icon,
                    (1, 1),
                    1.0,
                    1.0,
                    smithay::backend::renderer::element::Kind::Unspecified,
                );

            _ = dmg_tracked_renderer.render_output(renderer, &mut f, age, &elements, *clear_color);
            drop(f);
            egl_surface.swap_buffers(None).unwrap();
            // FIXME: damage tracking issues on integrated graphics but not nvidia
            // self.egl_surface
            //     .as_ref()
            //     .unwrap()
            //     .swap_buffers(res.0.as_deref_mut())?;

            // // TODO what if there is "no output"?
            for o in &self.client_state.outputs {
                let output = &o.1;
                send_frames_surface_tree(
                    s_icon,
                    &o.1,
                    Duration::from_millis(16),
                    None,
                    move |_, _| Some(output.clone()),
                );
            }
            wl_surface.frame(&self.client_state.queue_handle, wl_surface.clone());
            wl_surface.commit();

            *is_dirty = false;
            *has_frame = None;
        }
    }
}
