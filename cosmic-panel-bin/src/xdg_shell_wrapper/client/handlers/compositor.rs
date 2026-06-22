// SPDX-License-Identifier: MPL-2.0

use sctk::compositor::CompositorHandler;
use sctk::reexports::client::protocol::wl_surface;
use sctk::reexports::client::{Connection, QueueHandle};
use sctk::shell::WaylandSurface;
use smithay::reexports::wayland_server::protocol::wl_output::Transform;

use crate::xdg_shell_wrapper::shared_state::GlobalState;
use crate::xdg_shell_wrapper::space::WrapperSpace;

impl CompositorHandler for GlobalState {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        self.scale_factor_changed(surface, new_factor as f64, true);
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        time: u32,
    ) {
        // TODO proxied layer surfaces
        if let Some((icon, s_surface)) = self.server_state.seats.iter_mut().find_map(|s| {
            s.client
                .dnd_icon
                .iter_mut()
                .find(|dnd_icon| &dnd_icon.surface == surface)
                .map(|dnd_icon| (dnd_icon, s.server.dnd_icon.clone()))
        }) {
            icon.has_frame = true;
            if let Some(s_surface) = s_surface
                && icon.egl_surface.is_none()
            {
                smithay::wayland::compositor::CompositorHandler::commit(self, &s_surface);
            }
            self.draw_dnd_icon();
        } else {
            self.space.frame(surface, time);
        }
    }

    fn transform_changed(
        &mut self,
        conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        new_transform: sctk::reexports::client::protocol::wl_output::Transform,
    ) {
        for tracked_surface in &mut self.client_state.proxied_layer_surfaces {
            if tracked_surface.3.wl_surface() == surface {
                let transform = match new_transform {
                    sctk::reexports::client::protocol::wl_output::Transform::Normal => {
                        Transform::Normal
                    },
                    sctk::reexports::client::protocol::wl_output::Transform::_90 => Transform::_90,
                    sctk::reexports::client::protocol::wl_output::Transform::_180 => {
                        Transform::_180
                    },
                    sctk::reexports::client::protocol::wl_output::Transform::_270 => {
                        Transform::_270
                    },
                    sctk::reexports::client::protocol::wl_output::Transform::Flipped => {
                        Transform::Flipped
                    },
                    sctk::reexports::client::protocol::wl_output::Transform::Flipped90 => {
                        Transform::Flipped90
                    },
                    sctk::reexports::client::protocol::wl_output::Transform::Flipped180 => {
                        Transform::Flipped180
                    },
                    sctk::reexports::client::protocol::wl_output::Transform::Flipped270 => {
                        Transform::Flipped270
                    },
                    _ => {
                        tracing::warn!("Received unknown transform.");
                        return;
                    },
                };
                tracked_surface.2.wl_surface().preferred_buffer_transform(transform);
                return;
            }
        }

        self.space.transform_changed(conn, surface, new_transform);
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &cctk::wayland_client::protocol::wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &cctk::wayland_client::protocol::wl_output::WlOutput,
    ) {
    }
}
