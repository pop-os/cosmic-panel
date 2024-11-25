// SPDX-License-Identifier: MPL-2.0

use sctk::{
    delegate_compositor, delegate_output, delegate_registry, delegate_shm,
    output::OutputState,
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::SeatState,
    shm::{Shm, ShmHandler},
};

use crate::xdg_shell_wrapper::shared_state::GlobalState;

pub mod compositor;
pub mod data_device;
pub mod keyboard;
pub mod layer_shell;
/// output helpers
pub mod output;
pub mod overlap;
pub mod pointer;
pub mod seat;
pub mod shell;
pub mod toplevel;
pub mod workspace;
pub mod wp_fractional_scaling;
pub mod wp_security_context;
pub mod wp_viewporter;

impl ShmHandler for GlobalState {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.client_state.shm_state
    }
}

impl ProvidesRegistryState for GlobalState {
    registry_handlers![OutputState, SeatState,];

    fn registry(&mut self) -> &mut RegistryState {
        &mut self.client_state.registry_state
    }
}

delegate_registry!(GlobalState);
delegate_compositor!(GlobalState);
delegate_output!(GlobalState);
delegate_shm!(GlobalState);
