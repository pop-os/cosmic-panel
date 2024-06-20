use cctk::{
    cosmic_protocols::{
        toplevel_info::v1::client::zcosmic_toplevel_handle_v1,
        toplevel_management::v1::client::zcosmic_toplevel_manager_v1,
    },
    toplevel_info::{ToplevelInfoHandler, ToplevelInfoState},
    toplevel_management::ToplevelManagerHandler,
    wayland_client::{self, WEnum},
};
use wayland_client::{Connection, QueueHandle};

use crate::xdg_shell_wrapper::{
    shared_state::GlobalState,
    space::{ToplevelInfoSpace, ToplevelManagerSpace},
};

impl ToplevelManagerHandler for GlobalState {
    fn toplevel_manager_state(&mut self) -> &mut cctk::toplevel_management::ToplevelManagerState {
        self.client_state.toplevel_manager_state.as_mut().unwrap()
    }

    fn capabilities(
        &mut self,
        conn: &Connection,
        _: &QueueHandle<Self>,
        capabilities: Vec<
            WEnum<zcosmic_toplevel_manager_v1::ZcosmicToplelevelManagementCapabilitiesV1>,
        >,
    ) {
        self.space.capabilities(conn, capabilities);
    }
}

impl ToplevelInfoHandler for GlobalState {
    fn toplevel_info_state(&mut self) -> &mut ToplevelInfoState {
        self.client_state.toplevel_info_state.as_mut().unwrap()
    }

    fn new_toplevel(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        toplevel: &zcosmic_toplevel_handle_v1::ZcosmicToplevelHandleV1,
    ) {
        let toplevel_state = if let Some(s) = self.client_state.toplevel_info_state.as_mut() {
            s
        } else {
            return;
        };
        let info = if let Some(info) = toplevel_state.info(toplevel) {
            info
        } else {
            return;
        };
        self.space.new_toplevel(_conn, toplevel, info);
    }

    fn update_toplevel(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        toplevel: &zcosmic_toplevel_handle_v1::ZcosmicToplevelHandleV1,
    ) {
        let toplevel_state = if let Some(s) = self.client_state.toplevel_info_state.as_mut() {
            s
        } else {
            return;
        };
        let info = if let Some(info) = toplevel_state.info(toplevel) {
            info
        } else {
            return;
        };
        self.space.update_toplevel(_conn, toplevel, info);
    }

    fn toplevel_closed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        toplevel: &zcosmic_toplevel_handle_v1::ZcosmicToplevelHandleV1,
    ) {
        self.space.toplevel_closed(_conn, toplevel);
    }
}

cctk::delegate_toplevel_info!(GlobalState);
cctk::delegate_toplevel_manager!(GlobalState);
