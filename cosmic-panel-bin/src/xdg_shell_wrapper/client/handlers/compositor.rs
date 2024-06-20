// SPDX-License-Identifier: MPL-2.0

use sctk::{
    compositor::CompositorHandler,
    reexports::client::{protocol::wl_surface, Connection, QueueHandle},
    shell::WaylandSurface,
};
use smithay::reexports::wayland_server::protocol::wl_output::Transform;

use crate::xdg_shell_wrapper::{shared_state::GlobalState, space::WrapperSpace};

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
        if let Some(seat) = self
            .server_state
            .seats
            .iter_mut()
            .find(|s| s.client.dnd_icon.iter().any(|dnd_icon| &dnd_icon.1 == surface))
        {
            seat.client.dnd_icon.as_mut().unwrap().4 = Some(time);
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
