use sctk::shell::xdg::XdgPositioner;
use smithay::{
    delegate_xdg_shell,
    desktop::{PopupKind, Window},
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel, wayland_server::protocol::wl_seat,
    },
    utils::Serial,
    wayland::shell::xdg::{
        PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
    },
};

use crate::xdg_shell_wrapper::{shared_state::GlobalState, space::WrapperSpace};

impl XdgShellHandler for GlobalState {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.server_state.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let window = Window::new_wayland_window(surface.clone());

        self.space.add_window(window);
        surface.send_configure();
    }

    fn new_popup(&mut self, surface: PopupSurface, positioner_state: PositionerState) {
        let positioner = match XdgPositioner::new(&self.client_state.xdg_shell_state) {
            Ok(p) => p,
            Err(_) => return,
        };

        if self
            .space
            .add_popup(
                &self.client_state.compositor_state,
                self.client_state.fractional_scaling_manager.as_ref(),
                self.client_state.viewporter_state.as_ref(),
                &self.client_state.connection,
                &self.client_state.queue_handle,
                &mut self.client_state.xdg_shell_state,
                surface.clone(),
                positioner,
                positioner_state,
            )
            .is_ok()
        {
            self.server_state.popup_manager.track_popup(PopupKind::Xdg(surface.clone())).unwrap();
            self.server_state.popup_manager.commit(surface.wl_surface());
        }
    }

    fn move_request(&mut self, _surface: ToplevelSurface, _seat: wl_seat::WlSeat, _serial: Serial) {
    }

    fn resize_request(
        &mut self,
        _surface: ToplevelSurface,
        _seat: wl_seat::WlSeat,
        _serial: Serial,
        _edges: xdg_toplevel::ResizeEdge,
    ) {
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {
        // FIXME
        // for s in &self.server_state.seats {
        //     if s.server.owns(&seat) {
        //         let popup = PopupKind::Xdg(surface);
        //         if let Err(e) = self.server_state.popup_manager.grab_popup(
        //             popup.wl_surface().clone(),
        //             popup,
        //             &s.server,
        //             SERIAL_COUNTER.next_serial(),
        //         ) {
        //             error!(self.log.clone(), "{}", e);
        //         }
        //         break;
        //     }
        // }
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        let _ = self.space.reposition_popup(surface.clone(), positioner, token);
        self.server_state.popup_manager.commit(surface.wl_surface());
    }

    fn popup_destroyed(&mut self, surface: PopupSurface) {
        self.server_state.popup_manager.commit(surface.wl_surface());
    }
}

// Xdg Shell
delegate_xdg_shell!(GlobalState);
