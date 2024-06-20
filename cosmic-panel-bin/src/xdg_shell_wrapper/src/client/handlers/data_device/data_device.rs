use std::time::Instant;

use cctk::wayland_client::protocol::wl_surface::WlSurface;
use sctk::{
    data_device_manager::{
        data_device::{DataDeviceData, DataDeviceHandler},
        data_offer::DataOfferData,
    },
    reexports::client::{
        protocol::{
            wl_data_device::WlDataDevice, wl_data_device_manager::DndAction as ClientDndAction,
        },
        Proxy,
    },
    seat::pointer::{PointerEvent, PointerEventKind, PointerHandler},
};
use smithay::{
    input::pointer::GrabStartData,
    reexports::wayland_server::{protocol::wl_data_device_manager::DndAction, Resource},
    utils::SERIAL_COUNTER,
    wayland::selection::data_device::{
        set_data_device_focus, set_data_device_selection, start_dnd, SourceMetadata,
    },
};

use crate::xdg_shell_wrapper::{client_state::FocusStatus, shared_state::GlobalState, space::WrapperSpace};

impl DataDeviceHandler for GlobalState {
    fn selection(
        &mut self,
        _conn: &sctk::reexports::client::Connection,
        _qh: &sctk::reexports::client::QueueHandle<Self>,
        data_device: &WlDataDevice,
    ) {
        let seat = match self
            .server_state
            .seats
            .iter_mut()
            .find(|sp| sp.client.data_device.inner() == data_device)
        {
            Some(sp) => sp,
            None => return,
        };

        // ignore our own selection offer
        if seat.client.next_selection_offer_is_mine {
            seat.client.next_selection_offer_is_mine = false;
            return;
        }

        let offer = match data_device
            .data::<DataDeviceData>()
            .unwrap()
            .selection_offer()
        {
            Some(offer) => offer,
            None => return,
        };
        let wl_offer = offer.inner();

        let mime_types = wl_offer
            .data::<DataOfferData>()
            .unwrap()
            .with_mime_types(|m| m.to_vec());

        set_data_device_selection(
            &self.server_state.display_handle,
            &seat.server.seat,
            mime_types,
            (),
        )
    }

    fn enter(
        &mut self,
        _conn: &sctk::reexports::client::Connection,
        _qh: &sctk::reexports::client::QueueHandle<Self>,
        data_device: &WlDataDevice,
        _x: f64,
        _y: f64,
        surface: &WlSurface,
    ) {
        let seat = match self
            .server_state
            .seats
            .iter_mut()
            .find(|sp| sp.client.data_device.inner() == data_device)
        {
            Some(sp) => sp,
            None => return,
        };

        if let Some(f) = self
            .client_state
            .focused_surface
            .borrow_mut()
            .iter_mut()
            .find(|f| f.1 == seat.name)
        {
            f.0 = surface.clone();
            f.2 = FocusStatus::Focused;
        }

        let offer = match data_device.data::<DataDeviceData>().unwrap().drag_offer() {
            Some(offer) => offer,
            None => return,
        };

        {
            let mut c_hovered_surface = self.client_state.hovered_surface.borrow_mut();
            if let Some(i) = c_hovered_surface.iter().position(|f| f.1 == seat.name) {
                c_hovered_surface[i].0 = surface.clone();
                c_hovered_surface[i].2 = FocusStatus::Focused;
            } else {
                c_hovered_surface.push((
                    offer.surface.clone(),
                    seat.name.clone(),
                    FocusStatus::Focused,
                ));
            }
        }

        let wl_offer = offer.inner();

        let mime_types = wl_offer
            .data::<DataOfferData>()
            .unwrap()
            .with_mime_types(|m| m.to_vec());
        let mut dnd_action = DndAction::empty();
        let c_action = offer.source_actions;
        if c_action.contains(ClientDndAction::Copy) {
            dnd_action |= DndAction::Copy;
        } else if c_action.contains(ClientDndAction::Move) {
            dnd_action |= DndAction::Move;
        } else if c_action.contains(ClientDndAction::Ask) {
            dnd_action |= DndAction::Ask;
        }

        let metadata = SourceMetadata {
            mime_types,
            dnd_action,
        };
        let (x, y) = (offer.x, offer.y);

        let server_focus =
            self.space
                .update_pointer((x as i32, y as i32), &seat.name, offer.surface.clone());

        seat.client.dnd_offer = Some(offer);
        // TODO: touch vs pointer start data
        if !seat.client.next_dnd_offer_is_mine {
            start_dnd(
                &self.server_state.display_handle.clone(),
                &seat.server.seat.clone(),
                self,
                SERIAL_COUNTER.next_serial(),
                Some(GrabStartData {
                    focus: server_focus.map(|f| (f.surface, f.s_pos.to_f64())),
                    button: 0x110, // assume left button for now, maybe there is another way..
                    location: (x, y).into(),
                }),
                None,
                metadata,
            );
        }
    }

    fn leave(
        &mut self,
        conn: &sctk::reexports::client::Connection,
        qh: &sctk::reexports::client::QueueHandle<Self>,
        data_device: &WlDataDevice,
    ) {
        let seat = match self
            .server_state
            .seats
            .iter_mut()
            .find(|sp| sp.client.data_device.inner() == data_device)
        {
            Some(sp) => sp,
            None => return,
        };
        let c_ptr = seat.client.ptr.as_ref().map(|p| p.pointer().clone());
        let s_ptr = seat.server.seat.get_pointer();
        let surface = if let Some(f) = self
            .client_state
            .focused_surface
            .borrow_mut()
            .iter_mut()
            .find(|f| f.1 == seat.name)
        {
            f.2 = FocusStatus::LastFocused(Instant::now());
            f.0.clone()
        } else {
            return;
        };

        {
            let mut c_hovered_surface = self.client_state.hovered_surface.borrow_mut();
            if let Some(i) = c_hovered_surface.iter().position(|f| f.0 == surface) {
                c_hovered_surface[i].2 = FocusStatus::LastFocused(Instant::now());
            }
        }

        let duration_since = Instant::now().duration_since(self.start_time).as_millis() as u32;

        let leave_event = PointerEvent {
            surface,
            kind: PointerEventKind::Motion {
                time: duration_since,
            },
            position: (0.0, 0.0),
        };
        if let Some(s) = s_ptr {
            s.unset_grab(self, SERIAL_COUNTER.next_serial().into(), 0);
        }

        if let Some(pointer) = c_ptr {
            self.pointer_frame(conn, qh, &pointer, &[leave_event]);
        }
    }

    fn motion(
        &mut self,
        conn: &sctk::reexports::client::Connection,
        qh: &sctk::reexports::client::QueueHandle<Self>,
        data_device: &WlDataDevice,
        _x: f64,
        _y: f64,
    ) {
        // treat it as pointer motion
        let seat = match self
            .server_state
            .seats
            .iter_mut()
            .find(|sp| sp.client.data_device.inner() == data_device)
        {
            Some(sp) => sp,
            None => return,
        };

        let offer = match data_device.data::<DataDeviceData>().unwrap().drag_offer() {
            Some(offer) => offer,
            None => return,
        };

        let server_focus = self.space.update_pointer(
            (offer.x as i32, offer.y as i32),
            &seat.name,
            offer.surface.clone(),
        );

        set_data_device_focus(
            &self.server_state.display_handle,
            &seat.server.seat,
            server_focus.and_then(|f| f.surface.client()),
        );
        let motion_event = PointerEvent {
            surface: offer.surface.clone(),
            kind: PointerEventKind::Motion {
                time: offer.time.unwrap_or_default(),
            },
            position: (offer.x, offer.y),
        };

        if let Some(pointer) = seat.client.ptr.as_ref().map(|p| p.pointer().clone()) {
            self.pointer_frame(conn, qh, &pointer, &[motion_event]);
        }
    }

    fn drop_performed(
        &mut self,
        conn: &sctk::reexports::client::Connection,
        qh: &sctk::reexports::client::QueueHandle<Self>,
        data_device: &WlDataDevice,
    ) {
        // treat it as pointer button release
        let seat = match self
            .server_state
            .seats
            .iter_mut()
            .find(|sp| sp.client.data_device.inner() == data_device)
        {
            Some(sp) => sp,
            None => return,
        };

        let offer = match data_device.data::<DataDeviceData>().unwrap().drag_offer() {
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
    }
}
