use cstk::{
    delegate_workspace,
    toplevel_info::{ToplevelInfoHandler, ToplevelInfoState},
    workspace::{WorkspaceClientHandler, WorkspaceClientState, WorkspaceHandler, WorkspaceState},
};
use smithay::reexports::wayland_server::{
    backend::{ClientData, ClientId, DisconnectReason},
    DisplayHandle,
};

use crate::xdg_shell_wrapper::{
    server_state::WrapperWorkspaceClientState, shared_state::GlobalState, space::WrapperSpace,
};

impl WorkspaceClientHandler for WrapperWorkspaceClientState {
    fn workspace_state(&self) -> &WorkspaceClientState {
        &self.workspace_client_state
    }
}

impl WorkspaceHandler for GlobalState {
    type Client = WrapperWorkspaceClientState;

    fn workspace_state(&self) -> &WorkspaceState<Self> {
        &self.server_state.workspace_state
    }

    fn workspace_state_mut(&mut self) -> &mut WorkspaceState<Self> {
        &mut self.server_state.workspace_state
    }

    fn commit_requests(&mut self, dh: &DisplayHandle, requests: Vec<cstk::workspace::Request>) {
        // TODO
    }
}

impl ClientData for WrapperWorkspaceClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

delegate_workspace!(GlobalState);
