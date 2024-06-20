// SPDX-License-Identifier: MPL-2.0
#![warn(missing_debug_implementations, rust_2018_idioms, missing_docs)]

//! Provides the core functionality for cosmic-panel

use std::time::{Duration, Instant};

use anyhow::Result;
use sctk::{reexports::client::Proxy, shm::multi::MultiPool};
use smithay::{
    backend::input::KeyState,
    input::keyboard::FilterResult,
    reexports::{calloop, wayland_server::Display},
    utils::SERIAL_COUNTER,
};

use client::state::ClientState;
pub use client::{
    handlers::{output, wp_fractional_scaling, wp_security_context, wp_viewporter},
    state as client_state,
};
pub use server::state as server_state;
use server::state::ServerState;
use shared_state::GlobalState;
use space::{Visibility, WrapperSpace};
pub use xdg_shell_wrapper_config as config;

mod client;
mod server;
/// shared state
pub mod shared_state;
/// wrapper space abstraction
pub mod space;
/// utilities
pub mod util;

/// run the cosmic panel xdg wrapper with the provided config
pub fn run(    mut space: SpaceContainer,
    client_state: ClientState,
    embedded_server_state: ServerState,
    mut event_loop: calloop::EventLoop<'static, GlobalState>,
    mut server_display: Display<GlobalState>,
) -> Result<()> {
    let start = std::time::Instant::now();

    let s_dh = server_display.handle();
    space.set_display_handle(s_dh.clone());

    let mut global_state = GlobalState::new(client_state, embedded_server_state, space, start);

    global_state.space.setup(
        &global_state.client_state.compositor_state,
        global_state.client_state.fractional_scaling_manager.as_ref(),
        global_state.client_state.security_context_manager.clone(),
        global_state.client_state.viewporter_state.as_ref(),
        &mut global_state.client_state.layer_state,
        &global_state.client_state.connection,
        &global_state.client_state.queue_handle,
    );

    // // remove extra looping after launch-pad is integrated
    for _ in 0..10 {
        event_loop.dispatch(Duration::from_millis(16), &mut global_state)?;
    }

    let multipool = MultiPool::new(&global_state.client_state.shm_state);

    let cursor_surface = global_state
        .client_state
        .compositor_state
        .create_surface(&global_state.client_state.queue_handle);
    global_state.client_state.multipool = multipool.ok();
    global_state.client_state.cursor_surface = Some(cursor_surface);

    event_loop.dispatch(Duration::from_millis(30), &mut global_state)?;

    global_state.bind_display(&s_dh);

    let mut last_cleanup = Instant::now();
    let five_min = Duration::from_secs(300);

    // TODO find better place for this
    // let set_clipboard_once = Rc::new(Cell::new(false));

    loop {
        // cleanup popup manager
        if last_cleanup.elapsed() > five_min {
            global_state.server_state.popup_manager.cleanup();
            last_cleanup = Instant::now();
        }

        // handle funky keyboard state.
        // if a client layer shell surface is closed, then it won't receive the release event
        // then the client will keep receiving input
        // so we send the release here instead
        let press = if let Some((key_pressed, kbd)) = global_state
            .client_state
            .last_key_pressed
            .iter()
            .position(|(_, _, layer_shell_wl_surface)| !layer_shell_wl_surface.is_alive())
            .and_then(|key_pressed| {
                global_state
                    .server_state
                    .seats
                    .iter()
                    .find(|s| s.name == global_state.client_state.last_key_pressed[key_pressed].0)
                    .and_then(|s| {
                        s.server.seat.get_keyboard().map(|kbd| {
                            (global_state.client_state.last_key_pressed.remove(key_pressed), kbd)
                        })
                    })
            }) {
            Some((key_pressed, kbd))
        } else {
            None
        };
        if let Some((key_pressed, kbd)) = press {
            kbd.input::<(), _>(
                &mut global_state,
                key_pressed.1 .0,
                KeyState::Released,
                SERIAL_COUNTER.next_serial(),
                key_pressed.1 .1.wrapping_add(1),
                move |_, _modifiers, _keysym| FilterResult::Forward,
            );
        }

        // dispatch desktop client events
        let dur = if matches!(global_state.space.visibility(), Visibility::Hidden) {
            Duration::from_millis(100)
        } else {
            Duration::from_millis(16)
        };
        event_loop.dispatch(dur, &mut global_state)?;

        // rendering
        {
            let space = &mut global_state.space;

            let _ = space.handle_events(
                &s_dh,
                &global_state.client_state.queue_handle,
                &mut global_state.server_state.popup_manager,
                global_state.start_time.elapsed().as_millis().try_into()?,
            );
        }
        if let Some(renderer) = global_state.space.renderer() {
            global_state.client_state.draw_layer_surfaces(
                renderer,
                global_state.start_time.elapsed().as_millis().try_into()?,
            );
        }

        // dispatch server events
        {
            server_display.dispatch_clients(&mut global_state)?;
            server_display.flush_clients()?;
        }
    }
}
