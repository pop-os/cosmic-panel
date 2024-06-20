use sctk::{
    delegate_xdg_popup, delegate_xdg_shell, delegate_xdg_window,
    shell::xdg::{popup::PopupHandler, window::WindowHandler},
};

use crate::xdg_shell_wrapper::{shared_state::GlobalState, space::WrapperSpace};

impl PopupHandler for GlobalState {
    fn configure(
        &mut self,
        _conn: &sctk::reexports::client::Connection,
        _qh: &sctk::reexports::client::QueueHandle<Self>,
        popup: &sctk::shell::xdg::popup::Popup,
        config: sctk::shell::xdg::popup::PopupConfigure,
    ) {
        self.space.configure_popup(popup, config);
    }

    fn done(
        &mut self,
        _conn: &sctk::reexports::client::Connection,
        _qh: &sctk::reexports::client::QueueHandle<Self>,
        popup: &sctk::shell::xdg::popup::Popup,
    ) {
        self.space.close_popup(popup)
    }
}

impl WindowHandler for GlobalState {
    fn request_close(
        &mut self,
        _conn: &sctk::reexports::client::Connection,
        _qh: &sctk::reexports::client::QueueHandle<Self>,
        _window: &sctk::shell::xdg::window::Window,
    ) {
        // nothing to be done
    }

    fn configure(
        &mut self,
        _conn: &sctk::reexports::client::Connection,
        _qh: &sctk::reexports::client::QueueHandle<Self>,
        _window: &sctk::shell::xdg::window::Window,
        _configure: sctk::shell::xdg::window::WindowConfigure,
        _serial: u32,
    ) {
        // nothing to be done
    }
}

delegate_xdg_window!(GlobalState);
delegate_xdg_shell!(GlobalState);
delegate_xdg_popup!(GlobalState);
