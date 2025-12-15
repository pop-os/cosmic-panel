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

    let multipool = MultiPool::new(&global_state.client_state.shm_state);

    let cursor_surface = global_state
        .client_state
        .compositor_state
        .create_surface(&global_state.client_state.queue_handle);
    global_state.client_state.multipool = multipool.ok();
    if let Some((scale, vp)) = global_state
        .client_state
        .fractional_scaling_manager
        .as_ref()
        .zip(global_state.client_state.viewporter_state.as_ref())
    {
        global_state.client_state.cursor_scale = Some(
            scale.fractional_scaling(&cursor_surface, &global_state.client_state.queue_handle),
        );
        global_state.client_state.cursor_vp =
            Some(vp.get_viewport(&cursor_surface, &global_state.client_state.queue_handle));
    }

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

    // TODO find better place for this
    // let set_clipboard_once = Rc::new(Cell::new(false));

    let mut prev_dur = Duration::from_millis(16);
    loop {
        let iter_start = Instant::now();

        let visibility = global_state.space.visibility();
        let is_hidden = matches!(visibility, Visibility::Hidden);
        let is_animating = matches!(
            visibility,
            Visibility::TransitionToHidden { .. } | Visibility::TransitionToVisible { .. }
        );
        // dispatch desktop client events
        // Use fast 16ms polling during animations for smooth frames
        let dur = if is_hidden && !is_animating {
            Duration::from_millis(300).max(prev_dur)
        } else {
            // During animation or when visible, always use 16ms for smooth 60fps
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

        let new_visibility = global_state.space.visibility();
        let new_is_hidden = matches!(new_visibility, Visibility::Hidden);
        let new_is_animating = matches!(
            new_visibility,
            Visibility::TransitionToHidden { .. } | Visibility::TransitionToVisible { .. }
        );

        // Reset timing when visibility state changes or animation starts/stops
        if is_hidden != new_is_hidden || is_animating != new_is_animating {
            prev_dur = Duration::from_millis(16);
            continue;
        }
        if let Some(dur) = Instant::now()
            .checked_duration_since(iter_start)
            .and_then(|spent| dur.checked_sub(spent))
        {
            // During animation, don't sleep - process frames as fast as possible
            // When hidden (not animating), can sleep longer to save power
            let max_sleep = if new_is_animating {
                Duration::from_millis(1) // Minimal sleep during animation
            } else if new_is_hidden {
                Duration::from_millis(50)
            } else {
                Duration::from_millis(16)
            };
            std::thread::sleep(dur.min(max_sleep));
        } else {
            // Only increase prev_dur when not animating
            if !new_is_animating {
                prev_dur = prev_dur.checked_mul(2).unwrap_or(prev_dur).min(Duration::from_millis(100));
            }
        }
    }
}
