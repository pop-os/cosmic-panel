// SPDX-License-Identifier: MPL-2.0

use sctk::delegate_layer;
use sctk::shell::WaylandSurface;
use sctk::shell::wlr_layer::{LayerShellHandler, LayerSurface, LayerSurfaceConfigure};

use crate::xdg_shell_wrapper::client_state::SurfaceState;
use crate::xdg_shell_wrapper::shared_state::GlobalState;
use crate::xdg_shell_wrapper::space::WrapperSpace;

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
            let server_surface = self.client_state.proxied_layer_surfaces[i].2.wl_surface().clone();
            self.client_state.proxied_layer_props.remove(&server_surface);
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
            let generation = match state {
                SurfaceState::Waiting(generation, _) | SurfaceState::WaitingFirst(generation, _) => {
                    let generation = *generation;
                    *state = SurfaceState::Dirty(generation);
                    generation
                },
                SurfaceState::Dirty(generation) => *generation,
            };
            tracing::trace!("Layer surface configure: {configure:?}, generation: {generation}");
            // Always relay the compositor's size to the proxied client. `send_pending_configure`
            // sends the initial configure (a client may not attach a buffer before it) and skips
            // later ones that would not change anything. Comparing against the size the proxy had
            // requested skipped the initial configure whenever the compositor granted exactly that
            // size, which is the normal case for a surface that is not stretched.
            s_layer_surface.layer_surface().with_pending_state(|pending_state| {
                pending_state.size =
                    Some((configure.new_size.0 as i32, configure.new_size.1 as i32).into());
            });
            s_layer_surface.layer_surface().send_pending_configure();
        } else {
            self.space.configure_layer(layer, configure);
        }
    }
}

delegate_layer!(GlobalState);
