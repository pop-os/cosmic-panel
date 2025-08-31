use crate::xdg_shell_wrapper::{
    server_state::{SeatPair, ServerPointerFocus},
    shared_state::GlobalState,
    space::WrapperSpace,
};
use sctk::{
    delegate_touch,
    reexports::client::{
        Connection, QueueHandle,
        protocol::{wl_surface::WlSurface, wl_touch::WlTouch},
    },
    seat::touch::TouchHandler,
};
use smithay::{
    input::touch::{self, TouchHandle},
    utils::{Point, SERIAL_COUNTER},
};

fn get_touch_handle(state: &GlobalState, touch: &WlTouch) -> (String, TouchHandle<GlobalState>) {
    let seat_index = state
        .server_state
        .seats
        .iter()
        .position(|SeatPair { client, .. }| {
            client.touch.as_ref().map(|t| t == touch).unwrap_or(false)
        })
        .unwrap();
    let seat_name = state.server_state.seats[seat_index].name.to_string();
    let touch = state.server_state.seats[seat_index].server.seat.get_touch().unwrap();
    (seat_name, touch)
}

impl TouchHandler for GlobalState {
    fn down(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        touch: &WlTouch,
        serial: u32,
        time: u32,
        surface: WlSurface,
        id: i32,
        location: (f64, f64),
    ) {
        let seat_index = self
            .server_state
            .seats
            .iter()
            .position(|SeatPair { client, .. }| {
                client.touch.as_ref().map(|t| t == touch).unwrap_or(false)
            })
            .unwrap();
        let seat_name = self.server_state.seats[seat_index].name.to_string();
        let touch = self.server_state.seats[seat_index].server.seat.get_touch().unwrap();
        self.server_state.seats[seat_index].client.last_touch_down = (serial, time);

        self.client_state.touch_surfaces.insert(id, surface.clone());

        if let Some(ServerPointerFocus { surface, c_pos, s_pos, .. }) =
            self.space.touch_under((location.0 as i32, location.1 as i32), &seat_name, surface)
        {
            touch.down(self, Some((surface, s_pos)), &touch::DownEvent {
                slot: Some(id as u32).into(),
                location: c_pos.to_f64() + Point::from(location),
                serial: SERIAL_COUNTER.next_serial(),
                time: time.try_into().unwrap(),
            });
            touch.frame(self);
        }
    }

    fn up(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        touch: &WlTouch,
        _serial: u32,
        time: u32,
        id: i32,
    ) {
        let (_, touch) = get_touch_handle(self, touch);

        touch.up(self, &touch::UpEvent {
            slot: Some(id as u32).into(),
            serial: SERIAL_COUNTER.next_serial(),
            time: time.try_into().unwrap(),
        });
    }

    fn motion(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        touch: &WlTouch,
        time: u32,
        id: i32,
        location: (f64, f64),
    ) {
        let (seat_name, touch) = get_touch_handle(self, touch);

        if let Some(surface) = self.client_state.touch_surfaces.get(&id) {
            if let Some(ServerPointerFocus { surface, c_pos, s_pos, .. }) = self.space.touch_under(
                (location.0 as i32, location.1 as i32),
                &seat_name,
                surface.clone(),
            ) {
                touch.motion(self, Some((surface, s_pos)), &touch::MotionEvent {
                    slot: Some(id as u32).into(),
                    location: c_pos.to_f64() + Point::from(location),
                    time: time.try_into().unwrap(),
                });
                touch.frame(self);
            }
        }
    }

    fn shape(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _touch: &WlTouch,
        _id: i32,
        _major: f64,
        _minor: f64,
    ) {
        // TODO not supported in smithay
    }

    fn orientation(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _touch: &WlTouch,
        _id: i32,
        _orientation: f64,
    ) {
        // TODO not supported in smithay
    }

    fn cancel(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, touch: &WlTouch) {
        let (_, touch) = get_touch_handle(self, touch);
        touch.cancel(self);
    }
}

delegate_touch!(GlobalState);
