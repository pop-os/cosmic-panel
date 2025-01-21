// SPDX-License-Identifier: MPL-2.0

use sctk::{
    delegate_layer,
    shell::wlr_layer::{LayerShellHandler, LayerSurface, LayerSurfaceConfigure},
};

use crate::xdg_shell_wrapper::{shared_state::GlobalState, space::WrapperSpace};

impl LayerShellHandler for GlobalState {
    fn closed(
        &mut self,
        _conn: &sctk::reexports::client::Connection,
        _qh: &sctk::reexports::client::QueueHandle<Self>,
        layer: &LayerSurface,
    ) {
        self.space.close_layer(layer);
    }

    fn configure(
        &mut self,
        _conn: &sctk::reexports::client::Connection,
        _qh: &sctk::reexports::client::QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        self.space.configure_layer(layer, configure);
    }
}

delegate_layer!(GlobalState);
