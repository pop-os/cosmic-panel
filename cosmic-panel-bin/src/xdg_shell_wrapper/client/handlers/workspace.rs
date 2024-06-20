use cctk::workspace::WorkspaceHandler;

use crate::xdg_shell_wrapper::{shared_state::GlobalState, space::WorkspaceHandlerSpace};

impl WorkspaceHandler for GlobalState {
    fn workspace_state(&mut self) -> &mut cctk::workspace::WorkspaceState {
        self.client_state.workspace_state.as_mut().unwrap()
    }

    fn done(&mut self) {
        let groups = self.client_state.workspace_state.as_ref().unwrap().workspace_groups();
        WorkspaceHandlerSpace::update(&mut self.space, groups);
    }
}

cctk::delegate_workspace!(GlobalState);
