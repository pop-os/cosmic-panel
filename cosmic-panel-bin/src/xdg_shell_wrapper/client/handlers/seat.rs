// SPDX-License-Identifier: MPL-2.0

use sctk::{
    delegate_seat,
    reexports::client::{protocol::wl_seat, Connection, QueueHandle},
    seat::{pointer::ThemeSpec, SeatHandler},
};

use crate::xdg_shell_wrapper::{
    client_state::ClientSeat,
    server_state::{SeatPair, ServerSeat},
    shared_state::GlobalState,
};

impl SeatHandler for GlobalState {
    fn seat_state(&mut self) -> &mut sctk::seat::SeatState {
        &mut self.client_state.seat_state
    }

    fn new_seat(&mut self, _conn: &Connection, qh: &QueueHandle<Self>, seat: wl_seat::WlSeat) {
        if let Some(info) = self.client_state.seat_state.info(&seat) {
            let name = info.name.unwrap_or_default();

            let mut new_server_seat = self
                .server_state
                .seat_state
                .new_wl_seat(&self.server_state.display_handle, name.clone());

            let kbd = if info.has_keyboard {
                if let Ok(kbd) = self.client_state.seat_state.get_keyboard(qh, &seat, None) {
                    Some(kbd)
                } else {
                    None
                }
            } else {
                None
            };

            let ptr = if info.has_pointer {
                if let Ok(ptr) = self.client_state.seat_state.get_pointer_with_theme(
                    qh,
                    &seat,
                    self.client_state.shm_state.wl_shm(),
                    self.client_state.compositor_state.create_surface(&qh),
                    ThemeSpec::System,
                ) {
                    Some(ptr)
                } else {
                    None
                }
            } else {
                None
            };

            // A lot of clients bind keyboard and pointer unconditionally once on launch..
            // Initial clients might race the compositor on adding periheral and
            // end up in a state, where they are not able to receive input.
            // Additionally a lot of clients don't handle keyboards/pointer objects being
            // removed very well either and we don't want to crash applications, because the
            // user is replugging their keyboard or mouse.
            //
            // So instead of doing the right thing (and initialize these capabilities as
            // matching devices appear), we have to surrender to reality and
            // just always expose a keyboard and pointer.
            new_server_seat.add_keyboard(Default::default(), 200, 20).unwrap();
            new_server_seat.add_pointer();

            let data_device = self.client_state.data_device_manager.get_data_device(qh, &seat);

            self.server_state.seats.push(SeatPair {
                name,
                client: ClientSeat {
                    _seat: seat.clone(),
                    kbd,
                    ptr,
                    data_device,
                    copy_paste_source: None,
                    dnd_source: None,
                    last_enter: 0,
                    last_key_press: (0, 0),
                    last_pointer_press: (0, 0),
                    selection_offer: None,
                    dnd_offer: None,
                    next_dnd_offer_is_mine: false,
                    next_selection_offer_is_mine: false,
                    dnd_icon: None,
                    // TODO forward touch
                },
                server: ServerSeat {
                    seat: new_server_seat,
                    selection_source: None,
                    dnd_source: None,
                    dnd_icon: None,
                },
            });
        }
    }

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: sctk::seat::Capability,
    ) {
        let info = if let Some(info) = self.client_state.seat_state.info(&seat) {
            info
        } else {
            return;
        };
        let sp = if let Some(sp) =
            self.server_state.seats.iter_mut().find(|sp| sp.client._seat == seat)
        {
            sp
        } else {
            let name = info.name.clone().unwrap_or_default();
            let server =
                self.server_state.seat_state.new_wl_seat(&self.server_state.display_handle, &name);
            self.server_state.seats.push(SeatPair {
                name,
                client: ClientSeat {
                    _seat: seat.clone(),
                    kbd: None,
                    ptr: None,
                    data_device: self.client_state.data_device_manager.get_data_device(qh, &seat),
                    copy_paste_source: None,
                    dnd_source: None,
                    selection_offer: None,
                    dnd_offer: None,
                    last_enter: 0,
                    last_key_press: (0, 0),
                    last_pointer_press: (0, 0),
                    next_selection_offer_is_mine: false,
                    next_dnd_offer_is_mine: false,
                    dnd_icon: None, // TODO forward touch
                },
                server: ServerSeat {
                    seat: server,
                    selection_source: None,
                    dnd_source: None,
                    dnd_icon: None,
                },
            });
            self.server_state.seats.last_mut().unwrap()
        };

        match capability {
            sctk::seat::Capability::Keyboard => {
                if info.has_keyboard {
                    sp.server.seat.add_keyboard(Default::default(), 200, 20).unwrap();
                    if let Ok(kbd) = self.client_state.seat_state.get_keyboard(qh, &seat, None) {
                        sp.client.kbd.replace(kbd);
                    }
                }
            },
            sctk::seat::Capability::Pointer => {
                if info.has_pointer {
                    sp.server.seat.add_pointer();
                    if let Ok(ptr) = self.client_state.seat_state.get_pointer_with_theme(
                        qh,
                        &seat,
                        self.client_state.shm_state.wl_shm(),
                        self.client_state.compositor_state.create_surface(&qh),
                        ThemeSpec::System,
                    ) {
                        sp.client.ptr.replace(ptr);
                    }
                }
            },
            sctk::seat::Capability::Touch => {}, // TODO
            _ => unimplemented!(),
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: sctk::seat::Capability,
    ) {
        let sp = if let Some(sp) =
            self.server_state.seats.iter_mut().find(|sp| sp.client._seat == seat)
        {
            sp
        } else {
            return;
        };
        match capability {
            sctk::seat::Capability::Keyboard => {
                sp.server.seat.remove_keyboard();
            },
            sctk::seat::Capability::Pointer => {
                sp.server.seat.remove_pointer();
            },
            sctk::seat::Capability::Touch => {}, // TODO
            _ => unimplemented!(),
        }
    }

    fn remove_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, seat: wl_seat::WlSeat) {
        let _ = if let Some(sp_i) =
            self.server_state.seats.iter().position(|sp| sp.client._seat == seat)
        {
            self.server_state.seats.swap_remove(sp_i)
        } else {
            return;
        };
    }
}

delegate_seat!(GlobalState);
