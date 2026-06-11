// SPDX-License-Identifier: MPL-2.0
#![warn(missing_debug_implementations, missing_docs)]

//! Provides the core functionality for cosmic-panel

use std::time::Duration;

use anyhow::Result;
use sctk::shm::multi::MultiPool;
use smithay::reexports::calloop;
use smithay::reexports::wayland_server::Display;

pub use client::handlers::{wp_fractional_scaling, wp_security_context, wp_viewporter};
pub use client::state as client_state;
use client::state::ClientState;
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

    // Pacing is driven by frame callbacks (`wl_surface.frame`), not a fixed
    // timer. The Wayland client connection is a calloop source, so
    // `event_loop.dispatch` returns as soon as the compositor sends the next
    // frame callback. Rendering is gated on `has_frame` (see `Space::render`),
    // so the panel emits at most one frame per callback and animates at the
    // display's presentation rate without needing to know the refresh rate.
    // The timeouts below are only upper bounds for when nothing else wakes us.
    loop {
        // Ceiling on how long to block, not the frame interval: a frame callback
        // wakes `dispatch` earlier while animating, so animation is paced at the
        // display's refresh rate. The embedded applet server is polled once per
        // iteration via `dispatch_clients` (its fd is not a calloop source), so
        // the visible ceiling stays tight (~60 Hz) to keep applet updates
        // responsive; hidden panels can idle longer.
        let dispatch_timeout = if matches!(global_state.space.visibility(), Visibility::Hidden) {
            Duration::from_millis(300)
        } else {
            Duration::from_millis(16)
        };

        event_loop.dispatch(dispatch_timeout, &mut global_state)?;

        // rendering
        {
            let space = &mut global_state.space;

            let _ = space.handle_events(
                &s_dh,
                &global_state.client_state.queue_handle,
                &mut global_state.server_state.popup_manager,
                global_state.start_time.elapsed().as_millis().try_into()?,
                Some(dispatch_timeout),
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
    }
}
