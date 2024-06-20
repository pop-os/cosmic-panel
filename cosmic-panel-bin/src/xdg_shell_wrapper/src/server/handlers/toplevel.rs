use cstk::{
    delegate_toplevel_info, delegate_toplevel_management,
    toplevel_info::{ToplevelInfoHandler, ToplevelInfoState},
    toplevel_management::ToplevelManagementHandler,
};

use crate::xdg_shell_wrapper::{shared_state::GlobalState, space::WrapperSpace};

impl ToplevelInfoHandler for GlobalState {
    fn toplevel_info_state(&self) -> &ToplevelInfoState<Self> {
        &self.server_state.toplevel_info_state
    }

    fn toplevel_info_state_mut(&mut self) -> &mut ToplevelInfoState<Self> {
        &mut self.server_state.toplevel_info_state
    }
}

impl ToplevelManagementHandler for GlobalState {
    fn toplevel_management_state(
        &mut self,
    ) -> &mut cstk::toplevel_management::ToplevelManagementState {
        &mut self.server_state.toplevel_management_state
    }
}

delegate_toplevel_info!(GlobalState);
delegate_toplevel_management!(GlobalState);
