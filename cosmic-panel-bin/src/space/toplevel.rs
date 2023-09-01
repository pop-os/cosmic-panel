use cctk::{
    cosmic_protocols::toplevel_info::v1::client::zcosmic_toplevel_handle_v1,
    toplevel_info::ToplevelInfo, wayland_client::Connection,
};
use xdg_shell_wrapper::space::ToplevelInfoSpace;

use crate::space_container::toplevel;

use super::PanelSpace;

fn _update_toplevel(
    space: &mut PanelSpace,
    toplevel: &zcosmic_toplevel_handle_v1::ZcosmicToplevelHandleV1,
    info: &ToplevelInfo,
) {
    if info.output.is_none() || info.output.as_ref() != space.output.as_ref().map(|o| &o.0) {
        return;
    }
}

fn apply_maximized_state(space: &mut PanelSpace) {
    if false {
        // TODO un-expand if it is not configured to be expanded
        // re-enable gaps if it is configured to have gaps
        // Fix border radius if it is configured to have a border radius
        // re-enable exclusive zone if it is configured to be exclusive
    } else {
        // TODO expand if it is not configured to be expanded
        // disable gaps if it is configured to have gaps
        // Fix border radius to be 0, avoiding shine through
        // disable exclusive zone if it is configured to be exclusive
    }
}
