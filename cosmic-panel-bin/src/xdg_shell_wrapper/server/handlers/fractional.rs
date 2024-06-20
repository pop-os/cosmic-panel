use smithay::{
    delegate_fractional_scale,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    wayland::{
        compositor::with_states,
        fractional_scale::{with_fractional_scale, FractionalScaleHandler},
    },
};

use crate::xdg_shell_wrapper::{shared_state::GlobalState, space::WrapperSpace};

impl FractionalScaleHandler for GlobalState {
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        // Here we can set the initial fractional scale
        //
        // We find the space that the surface is in, and set the fractional scale
        // to the fractional scale of the surface in the space

        for tracked_surface in &self.client_state.proxied_layer_surfaces {
            if tracked_surface.2.wl_surface() == &surface {
                with_states(&surface, |states| {
                    with_fractional_scale(states, |fractional_scale| {
                        fractional_scale.set_preferred_scale(tracked_surface.5);
                    });
                });

                return;
            }
        }

        with_states(&surface, |states| {
            with_fractional_scale(states, |fractional_scale| {
                let scale_factor = self.space.get_scale_factor(&surface).unwrap_or(1.0);
                fractional_scale.set_preferred_scale(scale_factor);
            });
        });
    }
}

delegate_fractional_scale!(GlobalState);
