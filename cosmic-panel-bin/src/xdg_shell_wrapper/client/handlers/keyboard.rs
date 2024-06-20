use std::time::Instant;

use crate::xdg_shell_wrapper::{
    client_state::FocusStatus, server_state::SeatPair, shared_state::GlobalState,
    space::WrapperSpace,
};
use sctk::{
    delegate_keyboard,
    seat::keyboard::{KeyboardHandler, Keysym, RepeatInfo},
    shell::WaylandSurface,
};
use smithay::{backend::input::KeyState, input::keyboard::FilterResult, utils::SERIAL_COUNTER};

impl KeyboardHandler for GlobalState {
    fn enter(
        &mut self,
        _conn: &sctk::reexports::client::Connection,
        _qh: &sctk::reexports::client::QueueHandle<Self>,
        keyboard: &sctk::reexports::client::protocol::wl_keyboard::WlKeyboard,
        surface: &sctk::reexports::client::protocol::wl_surface::WlSurface,
        _serial: u32,
        _raw: &[u32],
        _keysyms: &[Keysym],
    ) {
        let _ = _keysyms;
        let (seat_name, kbd) = if let Some((name, Some(kbd))) = self
            .server_state
            .seats
            .iter()
            .find(|SeatPair { client, .. }| {
                client.kbd.as_ref().map(|k| k == keyboard).unwrap_or(false)
            })
            .map(|seat| (seat.name.as_str(), seat.server.seat.get_keyboard()))
        {
            (name.to_string(), kbd)
        } else {
            return;
        };

        {
            let mut c_focused_surface = self.client_state.focused_surface.borrow_mut();
            if let Some(i) = c_focused_surface.iter().position(|f| f.1 == seat_name) {
                c_focused_surface[i].0 = surface.clone();
                c_focused_surface[i].2 = FocusStatus::Focused;
            } else {
                c_focused_surface.push((
                    surface.clone(),
                    seat_name.to_string(),
                    FocusStatus::Focused,
                ));
            }
        }
        let s_surface = self.client_state.proxied_layer_surfaces.iter_mut().find_map(
            |(_, _, s, c, _, _, ..)| {
                if c.wl_surface() == surface {
                    Some(s.wl_surface().clone())
                } else {
                    None
                }
            },
        );

        if let Some(s_surface) = s_surface {
            kbd.set_focus(self, Some(s_surface), SERIAL_COUNTER.next_serial());
        } else {
            let s = self.space.keyboard_enter(&seat_name, surface.clone());
            kbd.set_focus(self, s, SERIAL_COUNTER.next_serial());
        }
    }

    fn leave(
        &mut self,
        _conn: &sctk::reexports::client::Connection,
        _qh: &sctk::reexports::client::QueueHandle<Self>,
        keyboard: &sctk::reexports::client::protocol::wl_keyboard::WlKeyboard,
        surface: &sctk::reexports::client::protocol::wl_surface::WlSurface,
        _serial: u32,
    ) {
        let (name, kbd) = if let Some((name, Some(kbd))) = self
            .server_state
            .seats
            .iter()
            .find(|SeatPair { client, .. }| {
                client.kbd.as_ref().map(|k| k == keyboard).unwrap_or(false)
            })
            .map(|seat| (seat.name.as_str(), seat.server.seat.get_keyboard()))
        {
            (name.to_string(), kbd)
        } else {
            return;
        };

        let kbd_focus = {
            let mut c_focused_surface = self.client_state.focused_surface.borrow_mut();
            if let Some(i) = c_focused_surface.iter().position(|f| &f.0 == surface) {
                c_focused_surface[i].2 = FocusStatus::LastFocused(Instant::now());
                true
            } else {
                false
            }
        };

        let s_surface = self
            .client_state
            .proxied_layer_surfaces
            .iter_mut()
            .any(|(_, _, _, c, _, _, ..)| c.wl_surface() == surface);

        if kbd_focus {
            if !s_surface {
                self.space.keyboard_leave(&name, Some(surface.clone()));
            }
        }
        kbd.set_focus(self, None, SERIAL_COUNTER.next_serial());
    }

    fn press_key(
        &mut self,
        _conn: &sctk::reexports::client::Connection,
        _qh: &sctk::reexports::client::QueueHandle<Self>,
        keyboard: &sctk::reexports::client::protocol::wl_keyboard::WlKeyboard,
        serial: u32,
        event: sctk::seat::keyboard::KeyEvent,
    ) {
        let (seat_name, kbd) = if let Some((name, Some(kbd), last_key_pressed)) = self
            .server_state
            .seats
            .iter_mut()
            .find(|SeatPair { client, .. }| {
                client.kbd.as_ref().map(|k| k == keyboard).unwrap_or(false)
            })
            .map(|seat| {
                (
                    seat.name.as_str(),
                    seat.server.seat.get_keyboard(),
                    &mut seat.client.last_key_press,
                )
            }) {
            *last_key_pressed = (serial, event.time);
            (name.to_string(), kbd)
        } else {
            return;
        };
        let c_kbd_focus = {
            let c_focused_surface = self.client_state.focused_surface.borrow_mut();
            c_focused_surface.iter().find_map(|f| {
                if f.1 == seat_name {
                    Some(f.0.clone())
                } else {
                    None
                }
            })
        };

        if let Some(c_focus) = c_kbd_focus {
            self.client_state.last_key_pressed.push((
                seat_name,
                (event.raw_code, event.time),
                c_focus,
            ))
        }

        let _ = kbd.input::<(), _>(
            self,
            event.raw_code,
            KeyState::Pressed,
            SERIAL_COUNTER.next_serial(),
            event.time,
            move |_, _modifiers, _keysym| FilterResult::Forward,
        );
    }

    fn release_key(
        &mut self,
        _conn: &sctk::reexports::client::Connection,
        _qh: &sctk::reexports::client::QueueHandle<Self>,
        keyboard: &sctk::reexports::client::protocol::wl_keyboard::WlKeyboard,
        _serial: u32,
        event: sctk::seat::keyboard::KeyEvent,
    ) {
        let (name, kbd) = if let Some((name, Some(kbd))) = self
            .server_state
            .seats
            .iter()
            .find(|SeatPair { client, .. }| {
                client.kbd.as_ref().map(|k| k == keyboard).unwrap_or(false)
            })
            .map(|seat| (seat.name.as_str(), seat.server.seat.get_keyboard()))
        {
            (name.to_string(), kbd)
        } else {
            return;
        };

        self.client_state
            .last_key_pressed
            .retain(|(seat_name, raw_code, _s)| seat_name != &name && raw_code.0 == event.raw_code);

        kbd.input::<(), _>(
            self,
            event.raw_code,
            KeyState::Released,
            SERIAL_COUNTER.next_serial(),
            event.time,
            move |_, _modifiers, _keysym| FilterResult::Forward,
        );
    }

    fn update_repeat_info(
        &mut self,
        _conn: &sctk::reexports::client::Connection,
        _qh: &sctk::reexports::client::QueueHandle<Self>,
        kbd: &sctk::reexports::client::protocol::wl_keyboard::WlKeyboard,
        info: RepeatInfo,
    ) {
        if let Some(kbd) =
            self.server_state.seats.iter().find_map(|SeatPair { client, server, .. }| {
                client.kbd.as_ref().and_then(|k| {
                    if k == kbd {
                        server.seat.get_keyboard()
                    } else {
                        None
                    }
                })
            })
        {
            match info {
                RepeatInfo::Repeat { rate, delay } => {
                    kbd.change_repeat_info(u32::from(rate) as i32, delay.try_into().unwrap())
                },
                RepeatInfo::Disable => kbd.change_repeat_info(0, 0),
            };
        }
    }

    fn update_keymap(
        &mut self,
        _conn: &sctk::reexports::client::Connection,
        _qh: &sctk::reexports::client::QueueHandle<Self>,
        keyboard: &sctk::reexports::client::protocol::wl_keyboard::WlKeyboard,
        keymap: sctk::seat::keyboard::Keymap<'_>,
    ) {
        let (name, kbd) = if let Some((name, Some(kbd))) = self
            .server_state
            .seats
            .iter()
            .find(|SeatPair { client, .. }| {
                client.kbd.as_ref().map(|k| k == keyboard).unwrap_or(false)
            })
            .map(|seat| (seat.name.as_str(), seat.server.seat.get_keyboard()))
        {
            (name.to_string(), kbd)
        } else {
            return;
        };

        if let Err(err) = kbd.set_keymap_from_string(self, keymap.as_string()) {
            tracing::error!("Failed to set keymap for seat {}: {}", name, err);
        }
    }

    fn update_modifiers(
        &mut self,
        _conn: &sctk::reexports::client::Connection,
        _qh: &sctk::reexports::client::QueueHandle<Self>,
        _keyboard: &sctk::reexports::client::protocol::wl_keyboard::WlKeyboard,
        _serial: u32,
        _modifiers: sctk::seat::keyboard::Modifiers,
        _: u32,
    ) {
        // TODO should these be handled specially
    }
}

delegate_keyboard!(GlobalState);
