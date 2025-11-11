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
    backend::input::ButtonState,
    input::{touch::{self, TouchHandle}, pointer::ButtonEvent},
    utils::{Point, SERIAL_COUNTER},
};

// Timeout in milliseconds for converting touch to click
const TOUCH_CLICK_TIMEOUT_MS: u32 = 200;

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
            touch.down(
                self,
                Some((surface, s_pos)),
                &touch::DownEvent {
                    slot: Some(id as u32).into(),
                    location: c_pos.to_f64() + Point::from(location),
                    serial: SERIAL_COUNTER.next_serial(),
                    time: time.try_into().unwrap(),
                },
            );
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
        let (seat_name, touch_handle) = get_touch_handle(self, touch);

        // Check if we should generate a synthetic click event
        let seat_index = self
            .server_state
            .seats
            .iter()
            .position(|seat_pair| {
                seat_pair.client.touch.as_ref().map(|t| t == touch).unwrap_or(false)
            })
            .unwrap();
        
        let seat_pair = &self.server_state.seats[seat_index];
        let (_, touch_down_time) = seat_pair.client.last_touch_down;
        let time_diff = time.saturating_sub(touch_down_time);
        
        // If touch up happened quickly after touch down, generate a synthetic click
        if time_diff <= TOUCH_CLICK_TIMEOUT_MS {
            // Get the pointer handle to generate synthetic button events
            let ptr = seat_pair.server.seat.get_pointer().unwrap();
            
            // Generate synthetic button press followed by button release
            ptr.button(
                self,
                &ButtonEvent {
                    serial: SERIAL_COUNTER.next_serial(),
                    time,
                    button: 0x110, // BTN_LEFT (272 decimal, 0x110 hex)
                    state: ButtonState::Pressed,
                },
            );
            ptr.button(
                self,
                &ButtonEvent {
                    serial: SERIAL_COUNTER.next_serial(),
                    time,
                    button: 0x110, // BTN_LEFT 
                    state: ButtonState::Released,
                },
            );
            ptr.frame(self);
        }

        // Handle the regular touch up event
        touch_handle.up(
            self,
            &touch::UpEvent {
                slot: Some(id as u32).into(),
                serial: SERIAL_COUNTER.next_serial(),
                time: time.try_into().unwrap(),
            },
        );
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
                touch.motion(
                    self,
                    Some((surface, s_pos)),
                    &touch::MotionEvent {
                        slot: Some(id as u32).into(),
                        location: c_pos.to_f64() + Point::from(location),
                        time: time.try_into().unwrap(),
                    },
                );
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
