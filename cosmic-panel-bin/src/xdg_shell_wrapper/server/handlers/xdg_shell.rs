use itertools::Itertools;
use sctk::shell::xdg::XdgPositioner;
use smithay::{
    delegate_xdg_shell,
    desktop::{PopupKind, Window},
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel, wayland_server::protocol::wl_seat,
    },
    utils::{SERIAL_COUNTER, Serial},
    wayland::shell::xdg::{
        PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
    },
};

use crate::{
    iced::elements::target::SpaceTarget,
    xdg_shell_wrapper::{
        client_state::FocusStatus, shared_state::GlobalState, space::WrapperSpace,
    },
};

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
        if let Some(f_seat) = self.server_state.seats.iter().find(|s| {
            self.client_state
                .hovered_surface
                .borrow()
                .iter()
                .chain(self.client_state.focused_surface.borrow().iter())
                .any(|f| f.1 == s.name && matches!(f.2, FocusStatus::Focused))
        }) {
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
                    &f_seat.client._seat,
                    f_seat.client.get_serial_of_last_seat_event(),
                )
                .is_ok()
            {
                self.server_state
                    .popup_manager
                    .track_popup(PopupKind::Xdg(surface.clone()))
                    .unwrap();
                self.server_state.popup_manager.commit(surface.wl_surface());
                for kbd in self
                    .server_state
                    .seats
                    .iter()
                    .filter_map(|s| s.server.seat.get_keyboard())
                    .collect_vec()
                {
                    kbd.set_focus(
                        self,
                        Some(SpaceTarget::Surface(surface.wl_surface().clone())),
                        SERIAL_COUNTER.next_serial(),
                    );
                }
            }
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
        if let Some(cosmic_workspaces) = &self.space.cosmic_workspaces {
            cosmic_workspaces.hide();
        }
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
