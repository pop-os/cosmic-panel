// SPDX-License-Identifier: MPL-2.0
#![warn(missing_debug_implementations, missing_docs)]

//! Provides the core functionality for cosmic-panel

use std::time::{Duration, Instant};

use anyhow::Result;
use sctk::shm::multi::MultiPool;
use smithay::reexports::{calloop, wayland_server::Display};

use client::state::ClientState;
pub use client::{
    handlers::{wp_fractional_scaling, wp_security_context, wp_viewporter},
    state as client_state,
};
pub use server::state as server_state;
use server::state::ServerState;
use shared_state::GlobalState;
use space::{Visibility, WrapperSpace};
pub use xdg_shell_wrapper_config as config;

use crate::space_container::SpaceContainer;

pub(crate) mod client;
mod server;
/// shared state
pub mod shared_state;
/// wrapper space abstraction
pub mod space;
/// utilities
pub mod util;

/// run the cosmic panel xdg wrapper with the provided config
pub fn run(
    mut space: SpaceContainer,
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
        global_state.client_state.overlap_notify.clone(),
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

    let handle = event_loop.handle();
    handle
        .insert_source(
            calloop::timer::Timer::from_duration(Duration::from_secs(2)),
            |_, _, state| {
                state.cleanup();
                calloop::timer::TimeoutAction::ToDuration(Duration::from_secs(2))
            },
        )
        .expect("Failed to insert cleanup timer.");
    global_state.bind_display(&s_dh);

    let last_cleanup = Instant::now();
    let five_min = Duration::from_secs(300);

    // TODO find better place for this
    // let set_clipboard_once = Rc::new(Cell::new(false));

    let mut prev_dur = Duration::from_millis(16);
    loop {
        let iter_start = Instant::now();

        let visibility = matches!(global_state.space.visibility(), Visibility::Hidden);
        // dispatch desktop client events
        let dur = if matches!(global_state.space.visibility(), Visibility::Hidden) {
            Duration::from_millis(300)
        } else {
            Duration::from_millis(16)
        }
        .max(prev_dur);

        event_loop.dispatch(dur, &mut global_state)?;

        // rendering
        {
            let space = &mut global_state.space;

            let _ = space.handle_events(
                &s_dh,
                &global_state.client_state.queue_handle,
                &mut global_state.server_state.popup_manager,
                global_state.start_time.elapsed().as_millis().try_into()?,
                Some(dur),
            );
        }
        global_state.draw_dnd_icon();

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
        global_state.iter_count += 1;

        let new_visibility_hidden = matches!(global_state.space.visibility(), Visibility::Hidden);

        if visibility != new_visibility_hidden {
            prev_dur = Duration::from_millis(16);
            continue;
        }
        if let Some(dur) = Instant::now()
            .checked_duration_since(iter_start)
            .and_then(|spent| dur.checked_sub(spent))
        {
            std::thread::sleep(dur.min(Duration::from_millis(if new_visibility_hidden {
                50
            } else {
                16
            })));
        } else {
            prev_dur = prev_dur.checked_mul(2).unwrap_or(prev_dur).min(Duration::from_millis(100));
        }
    }
}
