use smithay::{delegate_cursor_shape, wayland::tablet_manager::TabletSeatHandler};

use crate::xdg_shell_wrapper::shared_state::GlobalState;

impl TabletSeatHandler for GlobalState {}
delegate_cursor_shape!(GlobalState);
