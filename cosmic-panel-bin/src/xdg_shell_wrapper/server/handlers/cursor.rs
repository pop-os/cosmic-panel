use smithay::input::tablet::TabletSeatHandler;
use smithay::reexports::wayland_server::protocol::wl_surface;

use crate::xdg_shell_wrapper::shared_state::GlobalState;

impl TabletSeatHandler for GlobalState {
    type ToolFocus = wl_surface::WlSurface;
}
