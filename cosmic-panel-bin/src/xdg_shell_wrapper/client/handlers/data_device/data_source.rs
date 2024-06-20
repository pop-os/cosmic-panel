use std::os::fd::AsFd;

use crate::xdg_shell_wrapper::{
    client_state::FocusStatus, shared_state::GlobalState, space::WrapperSpace,
};
use sctk::{
    data_device_manager::data_source::DataSourceHandler,
    reexports::client::protocol::{
        wl_data_device_manager::DndAction as ClientDndAction, wl_data_source::WlDataSource,
    },
    seat::pointer::{PointerEvent, PointerEventKind, PointerHandler},
};
use smithay::{
    reexports::wayland_server::protocol::wl_data_device_manager::DndAction, utils::SERIAL_COUNTER,
};

impl DataSourceHandler for GlobalState {
    fn send_request(
        &mut self,
        _conn: &sctk::reexports::client::Connection,
        _qh: &sctk::reexports::client::QueueHandle<Self>,
        source: &WlDataSource,
        mime: String,
        fd: sctk::data_device_manager::WritePipe,
    ) {
        let (seat, is_dnd) = match self.server_state.seats.iter().find_map(|seat| {
            seat.client
                .copy_paste_source
                .as_ref()
                .and_then(
                    |sel_source| {
                        if sel_source.inner() == source {
                            Some((seat, false))
                        } else {
                            None
                        }
                    },
                )
                .or_else(|| {
                    seat.client.dnd_source.as_ref().and_then(|dnd_source| {
                        if dnd_source.inner() == source {
                            Some((seat, true))
                        } else {
                            None
                        }
                    })
                })
        }) {
            Some(seat) => seat,
            None => return,
        };

        // TODO write from server source to fd
        // could be a selection source or a dnd source
        if is_dnd {
            if let Some(dnd_source) = seat.server.dnd_source.as_ref() {
                dnd_source.send(mime, fd.as_fd());
            }
        } else {
            if let Some(selection) = seat.server.selection_source.as_ref() {
                selection.send(mime, fd.as_fd());
            }
        }
    }

    fn accept_mime(
        &mut self,
        _conn: &sctk::reexports::client::Connection,
        _qh: &sctk::reexports::client::QueueHandle<Self>,
        source: &WlDataSource,
        mime: Option<String>,
    ) {
        let seat = match self.server_state.seats.iter().find(|seat| {
            seat.client.dnd_source.iter().any(|dnd_source| dnd_source.inner() == source)
        }) {
            Some(seat) => seat,
            None => return,
        };

        if let Some(dnd_source) = seat.server.dnd_source.as_ref() {
            dnd_source.target(mime);
        }
    }

    fn cancelled(
        &mut self,
        conn: &sctk::reexports::client::Connection,
        qh: &sctk::reexports::client::QueueHandle<Self>,
        source: &WlDataSource,
    ) {
        let seat = match self.server_state.seats.iter_mut().find(|seat| {
            seat.client.dnd_source.iter().any(|dnd_source| dnd_source.inner() == source)
        }) {
            Some(seat) => seat,
            None => return,
        };

        // cancel client DnD
        if let Some(dnd_source) = seat.client.dnd_source.take() {
            dnd_source.inner().destroy();
            seat.client.dnd_icon = None;
            seat.client.next_dnd_offer_is_mine = false;
        }

        // cancel server DnD or drop it
        if self
            .client_state
            .focused_surface
            .borrow()
            .iter()
            .any(|f| f.1 == seat.name && matches!(f.2, FocusStatus::Focused))
        {
            let offer = match seat.client.dnd_offer.take() {
                Some(offer) => offer,
                None => return,
            };

            let pointer_event = PointerEvent {
                surface: offer.surface,
                kind: PointerEventKind::Release {
                    serial: offer.serial,
                    time: offer.time.unwrap_or_default(),
                    button: 0x110,
                },
                position: (offer.x, offer.y),
            };
            if let Some(pointer) = seat.client.ptr.as_ref().map(|p| p.pointer().clone()) {
                self.pointer_frame(conn, qh, &pointer, &[pointer_event]);
            }
        } else if let Some(dnd_source) = seat.server.dnd_source.take() {
            dnd_source.cancelled();
            seat.server.dnd_icon = None;
            seat.server.seat.get_pointer().unwrap().unset_grab(
                self,
                SERIAL_COUNTER.next_serial().into(),
                0,
            );
        }
    }

    // TODO: DnD
    fn dnd_dropped(
        &mut self,
        _conn: &sctk::reexports::client::Connection,
        _qh: &sctk::reexports::client::QueueHandle<Self>,
        source: &WlDataSource,
    ) {
        let seat = match self.server_state.seats.iter().find(|seat| {
            seat.client.dnd_source.iter().any(|dnd_source| dnd_source.inner() == source)
        }) {
            Some(seat) => seat,
            None => return,
        };

        if let Some(dnd_source) = seat.server.dnd_source.as_ref() {
            dnd_source.dnd_drop_performed();
        }
    }

    fn dnd_finished(
        &mut self,
        _conn: &sctk::reexports::client::Connection,
        _qh: &sctk::reexports::client::QueueHandle<Self>,
        source: &WlDataSource,
    ) {
        let seat = match self.server_state.seats.iter_mut().find(|seat| {
            seat.client.dnd_source.iter().any(|dnd_source| dnd_source.inner() == source)
        }) {
            Some(seat) => seat,
            None => return,
        };

        if let Some(dnd_source) = seat.server.dnd_source.take() {
            dnd_source.dnd_finished();
            seat.server.dnd_icon = None;
            seat.client.dnd_icon = None;
            seat.client.dnd_source = None;
            seat.client.next_dnd_offer_is_mine = false;
            seat.server.seat.get_pointer().unwrap().unset_grab(
                self,
                SERIAL_COUNTER.next_serial().into(),
                0,
            );
        }
    }

    fn action(
        &mut self,
        _conn: &sctk::reexports::client::Connection,
        _qh: &sctk::reexports::client::QueueHandle<Self>,
        source: &WlDataSource,
        action: ClientDndAction,
    ) {
        let seat = match self.server_state.seats.iter_mut().find(|seat| {
            seat.client.dnd_source.iter().any(|dnd_source| dnd_source.inner() == source)
        }) {
            Some(seat) => seat,
            None => return,
        };

        let mut dnd_action = DndAction::empty();
        if action.contains(ClientDndAction::Copy) {
            dnd_action |= DndAction::Copy;
        }
        if action.contains(ClientDndAction::Move) {
            dnd_action |= DndAction::Move;
        }
        if action.contains(ClientDndAction::Ask) {
            dnd_action |= DndAction::Ask;
        }

        if let Some(dnd_source) = seat.server.dnd_source.as_ref() {
            dnd_source.action(dnd_action);
        }
    }
}
