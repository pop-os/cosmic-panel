use smithay::{
    delegate_layer_shell,
    wayland::shell::wlr_layer::{Layer, WlrLayerShellHandler},
};

use crate::xdg_shell_wrapper::{shared_state::GlobalState, space::WrapperSpace};

delegate_layer_shell!(GlobalState);
impl WlrLayerShellHandler for GlobalState {
    fn shell_state(&mut self) -> &mut smithay::wayland::shell::wlr_layer::WlrLayerShellState {
        &mut self.server_state.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: smithay::wayland::shell::wlr_layer::LayerSurface,
        output: Option<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>,
        _layer: Layer,
        namespace: String,
    ) {
        self.client_state.pending_layer_surfaces.push((surface, output, namespace));
    }

    fn layer_destroyed(&mut self, surface: smithay::wayland::shell::wlr_layer::LayerSurface) {
        // cleanup proxied surfaces
        self.client_state
            .proxied_layer_surfaces
            .retain(|s| s.2.wl_surface() != surface.wl_surface());
    }
}
