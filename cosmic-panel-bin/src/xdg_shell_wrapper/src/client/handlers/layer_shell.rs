// SPDX-License-Identifier: MPL-2.0

use sctk::{
    delegate_layer,
    shell::{
        wlr_layer::{LayerShellHandler, LayerSurface, LayerSurfaceConfigure},
        WaylandSurface,
    },
};

use crate::xdg_shell_wrapper::{client_state::SurfaceState, shared_state::GlobalState, space::WrapperSpace};

impl LayerShellHandler for GlobalState {
    fn closed(
        &mut self,
        _conn: &sctk::reexports::client::Connection,
        _qh: &sctk::reexports::client::QueueHandle<Self>,
        layer: &LayerSurface,
    ) {
        if let Some(i) = self
            .client_state
            .proxied_layer_surfaces
            .iter()
            .position(|(_, _, _, s, _, _, ..)| s.wl_surface() == layer.wl_surface())
        {
            self.client_state.proxied_layer_surfaces.remove(i);
        } else {
            self.space.close_layer(layer);
        }
    }

    fn configure(
        &mut self,
        _conn: &sctk::reexports::client::Connection,
        _qh: &sctk::reexports::client::QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        if let Some((_, _, s_layer_surface, c_layer_surface, mut state, _, ..)) = self
            .client_state
            .proxied_layer_surfaces
            .iter_mut()
            .find(|(_, _, _, s, _, _, ..)| s.wl_surface() == layer.wl_surface())
        {
            match state {
                SurfaceState::Waiting => {
                    state = SurfaceState::Dirty;
                }
                SurfaceState::Dirty => {}
                SurfaceState::WaitingFirst => {
                    state = SurfaceState::Waiting;
                }
            };
            let (width, height) = configure.new_size;

            s_layer_surface
                .layer_surface()
                .with_pending_state(|pending_state| {
                    pending_state.size = Some((width as i32, height as i32).into());
                });
            s_layer_surface.layer_surface().send_configure();
            c_layer_surface.set_size(width, height);
            c_layer_surface.wl_surface().commit();
        } else {
            self.space.configure_layer(layer, configure);
        }
    }
}

delegate_layer!(GlobalState);
