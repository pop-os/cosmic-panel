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
pub(crate) mod server;
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
    server_display: Display<GlobalState>,
) -> Result<()> {
    let start = std::time::Instant::now();

    let mut s_dh = server_display.handle();
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

    // Register the embedded applet server's poll fd as an event source, so
    // that applet requests wake `event_loop.dispatch` and are handled
    // immediately instead of being polled once per loop iteration. This is
    // what allows the loop timeouts below to be generous without hurting
    // applet responsiveness.
    handle
        .insert_source(
            calloop::generic::Generic::new(
                server_display,
                calloop::Interest::READ,
                calloop::Mode::Level,
            ),
            |_, display, state| {
                // SAFETY: the display is neither dropped nor replaced
                let display = unsafe { display.get_mut() };
                display.dispatch_clients(state)?;
                display.flush_clients()?;
                Ok(calloop::PostAction::Continue)
            },
        )
        .expect("Failed to insert embedded wayland server source.");

    // TODO find better place for this
    // let set_clipboard_once = Rc::new(Cell::new(false));

    // Pacing is driven by frame callbacks (`wl_surface.frame`), not a fixed
    // timer. The Wayland client connection and the embedded applet server are
    // both calloop sources, so `event_loop.dispatch` returns as soon as the
    // compositor sends the next frame callback or an applet sends a request.
    // Rendering is gated on `has_frame` (see `Space::render`), so the panel
    // emits at most one frame per callback and animates at the display's
    // presentation rate without needing to know the refresh rate.
    loop {
        // The timeout is only a ceiling for time-based state that is polled in
        // `handle_events` rather than event-driven: the autohide
        // `hide_wait`/show-delay checks in `PanelSpace::handle_focus` and the
        // debounced iced updates. Everything latency-sensitive (input, frame
        // callbacks, applet requests) wakes `dispatch` through its own source.
        let dispatch_timeout = if matches!(global_state.space.visibility(), Visibility::Hidden) {
            Duration::from_millis(300)
        } else {
            Duration::from_millis(100)
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
                // Fallback frame-callback throttle for embedded applets;
                // panels override it with the frame duration of their own
                // output (see `PanelSpace::render`).
                Some(Duration::from_millis(16)),
            );
        }
        global_state.draw_dnd_icon();

        if let Some(renderer) = global_state.space.renderer() {
            global_state.client_state.draw_layer_surfaces(
                renderer,
                global_state.start_time.elapsed().as_millis().try_into()?,
            );
        }

        // flush events generated for embedded clients while rendering (e.g.
        // frame callbacks); their requests are dispatched by the server
        // display's event source above
        s_dh.flush_clients()?;
        global_state.iter_count += 1;
    }
}
