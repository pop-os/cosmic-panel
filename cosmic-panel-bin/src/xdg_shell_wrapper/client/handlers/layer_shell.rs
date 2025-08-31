// SPDX-License-Identifier: MPL-2.0

use sctk::{
    delegate_layer,
    shell::{
        WaylandSurface,
        wlr_layer::{LayerShellHandler, LayerSurface, LayerSurfaceConfigure},
    },
};

use crate::xdg_shell_wrapper::{
    client_state::SurfaceState, shared_state::GlobalState, space::WrapperSpace,
};

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
            .position(|(_, _, _, s, ..)| s.wl_surface() == layer.wl_surface())
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
        if let Some((_, _, s_layer_surface, _, state, ..)) = self
            .client_state
            .proxied_layer_surfaces
            .iter_mut()
            .find(|(_, _, _, s, ..)| s.wl_surface() == layer.wl_surface())
        {
            let mut requested_size = configure.new_size;
            let generation = match state {
                SurfaceState::Waiting(generation, size) => {
                    requested_size.0 = size.w as u32;
                    requested_size.1 = size.h as u32;
                    let generation = *generation;
                    *state = SurfaceState::Dirty(generation);
                    generation
                },
                SurfaceState::Dirty(generation) => *generation,
                SurfaceState::WaitingFirst(generation, size) => {
                    requested_size.0 = size.w as u32;
                    requested_size.1 = size.h as u32;
                    let generation = *generation;
                    *state = SurfaceState::Dirty(generation);
                    generation
                },
            };
            tracing::trace!("Layer surface configure: {configure:?}, generation: {generation}");
            if requested_size != configure.new_size {
                s_layer_surface.layer_surface().with_pending_state(|pending_state| {
                    pending_state.size =
                        Some((configure.new_size.0 as i32, configure.new_size.1 as i32).into());
                });
                s_layer_surface.layer_surface().send_configure();
            }
        } else {
            self.space.configure_layer(layer, configure);
        }
    }
}

delegate_layer!(GlobalState);
