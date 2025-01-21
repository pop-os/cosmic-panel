// SPDX-License-Identifier: MPL-2.0

use sctk::{
    compositor::CompositorHandler,
    reexports::client::{protocol::wl_surface, Connection, QueueHandle},
};

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
