use std::time::Instant;

use crate::xdg_shell_wrapper::{
    client_state::FocusStatus,
    server_state::{SeatPair, ServerPointerFocus},
    shared_state::GlobalState,
    space::WrapperSpace,
};
use sctk::{
    delegate_pointer,
    seat::pointer::{PointerEvent, PointerHandler},
};
use smithay::{
    backend::input::{self, Axis, ButtonState},
    input::pointer::{AxisFrame, ButtonEvent, MotionEvent},
    reexports::wayland_server::protocol::wl_pointer::AxisSource,
    utils::{Point, SERIAL_COUNTER},
    wayland::seat::WaylandFocus,
};

impl PointerHandler for GlobalState {
    fn pointer_frame(
        &mut self,
        conn: &sctk::reexports::client::Connection,
        _qh: &sctk::reexports::client::QueueHandle<Self>,
        pointer: &sctk::reexports::client::protocol::wl_pointer::WlPointer,
        events: &[sctk::seat::pointer::PointerEvent],
    ) {
        self.pointer_frame_inner(conn, pointer, events);
    }
}

impl GlobalState {
    fn update_generated_event_serial(&self, events: &mut Vec<PointerEvent>) {
        for e in events {
            match &mut e.kind {
                sctk::seat::pointer::PointerEventKind::Enter { serial } => {
                    *serial = SERIAL_COUNTER.next_serial().into();
                },
                sctk::seat::pointer::PointerEventKind::Leave { serial } => {
                    *serial = SERIAL_COUNTER.next_serial().into();
                },
                sctk::seat::pointer::PointerEventKind::Motion { time } => {
                    *time = self.start_time.elapsed().as_millis().try_into().unwrap();
                },
                sctk::seat::pointer::PointerEventKind::Press { time, serial, .. } => {
                    *time = self.start_time.elapsed().as_millis().try_into().unwrap();
                    *serial = SERIAL_COUNTER.next_serial().into();
                },
                sctk::seat::pointer::PointerEventKind::Release { time, serial, .. } => {
                    *time = self.start_time.elapsed().as_millis().try_into().unwrap();
                    *serial = SERIAL_COUNTER.next_serial().into();
                },
                sctk::seat::pointer::PointerEventKind::Axis { time, .. } => {
                    *time = self.start_time.elapsed().as_millis().try_into().unwrap();
                },
            }
        }
    }

    pub fn pointer_frame_inner(
        &mut self,
        conn: &sctk::reexports::client::Connection,
        pointer: &sctk::reexports::client::protocol::wl_pointer::WlPointer,
        events: &[sctk::seat::pointer::PointerEvent],
    ) {
        let start_time = self.start_time;
        let time = start_time.elapsed().as_millis();

        let seat_index = self
            .server_state
            .seats
            .iter()
            .position(|SeatPair { client, .. }| {
                client.ptr.as_ref().map(|p| p.pointer() == pointer).unwrap_or(false)
            })
            .unwrap();
        let seat_name = self.server_state.seats[seat_index].name.to_string();
        let ptr = self.server_state.seats[seat_index].server.seat.get_pointer().unwrap();
        let kbd = self.server_state.seats[seat_index].server.seat.get_keyboard().unwrap();
        for e in events {
            let seat = &mut self.server_state.seats[seat_index];
            match e.kind {
                sctk::seat::pointer::PointerEventKind::Leave { .. } => {
                    ptr.motion(self, None, &MotionEvent {
                        location: (0.0, 0.0).into(),
                        serial: SERIAL_COUNTER.next_serial(),
                        time: time.try_into().unwrap(),
                    });
                    ptr.frame(self);

                    let mut c_hovered_surface = self.client_state.hovered_surface.borrow_mut();
                    for f in c_hovered_surface.iter_mut().filter(|f| f.0 == e.surface) {
                        f.2 = FocusStatus::LastFocused(Instant::now());
                    }
                    drop(c_hovered_surface);
                    self.client_state.delayed_surface_motion.clear();

                    self.space.pointer_leave(&seat_name, Some(e.surface.clone()));
                },
                sctk::seat::pointer::PointerEventKind::Enter { serial } => {
                    seat.client.last_enter = serial;

                    let (surface_x, surface_y) = e.position;

                    {
                        let mut c_hovered_surface = self.client_state.hovered_surface.borrow_mut();
                        c_hovered_surface.clear();
                        c_hovered_surface.push((
                            e.surface.clone(),
                            seat_name.to_string(),
                            FocusStatus::Focused,
                        ));
                    }

                    if let Some((
                        ServerPointerFocus { surface, c_pos, s_pos, .. },
                        mut generated_events,
                    )) = self.space.pointer_enter(
                        (surface_x as i32, surface_y as i32),
                        &seat_name,
                        e.surface.clone(),
                    ) {
                        if generated_events.is_empty() {
                            if let Some(ev) = surface.wl_surface().and_then(|s| {
                                self.client_state.delayed_surface_motion.get_mut(s.as_ref())
                            }) {
                                *ev = (e.clone(), pointer.clone(), ev.2);
                            } else {
                                ptr.motion(self, Some((surface, s_pos)), &MotionEvent {
                                    location: c_pos.to_f64() + Point::from((surface_x, surface_y)),
                                    serial: SERIAL_COUNTER.next_serial(),
                                    time: time.try_into().unwrap(),
                                });
                                ptr.frame(self);
                            }
                        } else {
                            self.update_generated_event_serial(&mut generated_events);
                            self.pointer_frame_inner(conn, pointer, &generated_events);
                            if let Some(s) = surface.wl_surface() {
                                self.client_state.delayed_surface_motion.insert(
                                    s.into_owned(),
                                    (e.clone(), pointer.clone(), self.iter_count),
                                );
                            }
                        }
                    } else {
                        ptr.motion(self, None, &MotionEvent {
                            location: Point::from((surface_x, surface_y)),
                            serial: SERIAL_COUNTER.next_serial(),
                            time: time.try_into().unwrap(),
                        });
                        ptr.frame(self);
                    }
                },
                sctk::seat::pointer::PointerEventKind::Motion { time } => {
                    let (surface_x, surface_y) = e.position;

                    let c_focused_surface = match self
                        .client_state
                        .hovered_surface
                        .borrow()
                        .iter()
                        .find(|f| f.1.as_str() == seat_name)
                    {
                        Some(f) => f.0.clone(),
                        None => continue,
                    };

                    if let Some((
                        ServerPointerFocus { surface, c_pos, s_pos, .. },
                        mut generated_events,
                    )) = self.space.update_pointer(
                        (surface_x as i32, surface_y as i32),
                        &seat_name,
                        c_focused_surface,
                    ) {
                        if generated_events.is_empty() {
                            if let Some(ev) = surface.wl_surface().and_then(|s| {
                                self.client_state.delayed_surface_motion.get_mut(s.as_ref())
                            }) {
                                *ev = (e.clone(), pointer.clone(), ev.2);
                            } else {
                                ptr.motion(self, Some((surface, s_pos)), &MotionEvent {
                                    location: c_pos.to_f64() + Point::from((surface_x, surface_y)),
                                    serial: SERIAL_COUNTER.next_serial(),
                                    time,
                                });
                                ptr.frame(self);
                            }
                        } else {
                            self.update_generated_event_serial(&mut generated_events);
                            self.pointer_frame_inner(conn, pointer, &generated_events);
                            if let Some(s) = surface.wl_surface() {
                                self.client_state.delayed_surface_motion.insert(
                                    s.into_owned(),
                                    (e.clone(), pointer.clone(), self.iter_count),
                                );
                            }
                        }
                    } else {
                        ptr.motion(self, None, &MotionEvent {
                            location: Point::from((surface_x, surface_y)),
                            serial: SERIAL_COUNTER.next_serial(),
                            time,
                        });
                        ptr.frame(self);
                        if let Some(themed_pointer) =
                            &self.server_state.seats[seat_index].client.ptr
                        {
                            _ = themed_pointer
                                .set_cursor(conn, sctk::seat::pointer::CursorIcon::Default);
                        }
                    }
                },
                sctk::seat::pointer::PointerEventKind::Press { time, button, serial, .. } => {
                    self.server_state.last_button.replace(button);
                    seat.client.last_pointer_press = (serial, time);

                    let s = self.space.handle_button(&seat_name, true);

                    kbd.set_focus(self, s, SERIAL_COUNTER.next_serial());
                    ptr.button(self, &ButtonEvent {
                        serial: SERIAL_COUNTER.next_serial(),
                        time,
                        button,
                        state: ButtonState::Pressed,
                    });
                    ptr.frame(self);
                },
                sctk::seat::pointer::PointerEventKind::Release { time, button, .. } => {
                    self.server_state.last_button.replace(button);

                    let s = self.space.handle_button(&seat_name, false);
                    kbd.set_focus(self, s, SERIAL_COUNTER.next_serial());

                    ptr.button(self, &ButtonEvent {
                        serial: SERIAL_COUNTER.next_serial(),
                        time,
                        button,
                        state: ButtonState::Released,
                    });
                    ptr.frame(self);
                },
                sctk::seat::pointer::PointerEventKind::Axis {
                    time,
                    horizontal,
                    vertical,
                    source,
                } => {
                    let source = match source.and_then(|s| {
                        AxisSource::try_from(s as u32).ok().and_then(|s| match s {
                            AxisSource::Wheel => Some(input::AxisSource::Wheel),
                            AxisSource::Finger => Some(input::AxisSource::Finger),
                            AxisSource::Continuous => Some(input::AxisSource::Continuous),
                            AxisSource::WheelTilt => Some(input::AxisSource::WheelTilt),
                            _ => None,
                        })
                    }) {
                        Some(s) => s,
                        _ => continue,
                    };

                    let mut af = AxisFrame::new(time).source(source);

                    if !horizontal.is_none() {
                        if horizontal.discrete.abs() > 0 {
                            af = af.v120(Axis::Horizontal, horizontal.discrete * 120);
                        }
                        if horizontal.absolute.abs() > 0.0 {
                            af = af.value(Axis::Horizontal, horizontal.absolute * 120.);
                        }
                        if horizontal.stop {
                            af = af.stop(Axis::Horizontal);
                        }
                    }

                    if !vertical.is_none() {
                        if vertical.discrete.abs() > 0 {
                            af = af.v120(Axis::Vertical, vertical.discrete * 120);
                        }
                        if vertical.absolute.abs() > 0.0 {
                            af = af.value(Axis::Vertical, vertical.absolute * 120.);
                        }
                        if vertical.stop {
                            af = af.stop(Axis::Vertical);
                        }
                    }

                    ptr.axis(self, af);
                    ptr.frame(self);
                },
            }
        }
    }
}

delegate_pointer!(GlobalState);
