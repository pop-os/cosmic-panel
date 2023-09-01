use cctk::{
    cosmic_protocols::toplevel_info::v1::client::zcosmic_toplevel_handle_v1,
    toplevel_info::ToplevelInfo, wayland_client::Connection,
};
use xdg_shell_wrapper::space::ToplevelInfoSpace;

use super::SpaceContainer;

impl ToplevelInfoSpace for SpaceContainer {
    /// A new toplevel was created
    fn new_toplevel(
        &mut self,
        _conn: &Connection,
        toplevel: &zcosmic_toplevel_handle_v1::ZcosmicToplevelHandleV1,
        info: &ToplevelInfo,
    ) {
        _update_toplevel(self, toplevel, info);
    }

    /// A toplevel was updated
    fn update_toplevel(
        &mut self,
        _conn: &Connection,
        toplevel: &zcosmic_toplevel_handle_v1::ZcosmicToplevelHandleV1,
        info: &ToplevelInfo,
    ) {
        _update_toplevel(self, toplevel, info);
    }

    /// A toplevel was closed
    fn toplevel_closed(
        &mut self,
        _conn: &Connection,
        toplevel: &zcosmic_toplevel_handle_v1::ZcosmicToplevelHandleV1,
    ) {
        self.maximized_toplevels.retain(|(t, _)| t != toplevel);
    }
}

fn _update_toplevel(
    space: &mut SpaceContainer,
    toplevel: &zcosmic_toplevel_handle_v1::ZcosmicToplevelHandleV1,
    info: &ToplevelInfo,
) {
    if info.state.len() < 4 {
        return;
    }
    let Some(state_arr) = info.state.chunks_exact(4).next() else {
        return;
    };
    let Some(state) = zcosmic_toplevel_handle_v1::State::try_from(u32::from_ne_bytes(state_arr[0..4].try_into().unwrap())).ok() else {
        return;
    };
    if matches!(state, zcosmic_toplevel_handle_v1::State::Maximized) {
        if !space.maximized_toplevels.iter().any(|(t, _)| t == toplevel) {
            space
                .maximized_toplevels
                .push((toplevel.clone(), info.clone()));
        }
    } else {
        space.maximized_toplevels.retain(|(t, _)| t != toplevel);
    }
}
