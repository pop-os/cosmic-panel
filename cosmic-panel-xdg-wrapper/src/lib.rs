// SPDX-License-Identifier: MPL-2.0-only
#![warn(missing_debug_implementations, rust_2018_idioms, missing_docs)]

//! Provides the core functionality for cosmic-panel

use anyhow::Result;
use cosmic_panel_config::config::XdgWrapperConfig;
use freedesktop_desktop_entry::{default_paths, DesktopEntry, Iter};
use itertools::Itertools;
use shared_state::GlobalState;
use shlex::Shlex;
use slog::{trace, Logger};
use smithay::{
    reexports::{nix::fcntl, wayland_server::Display},
    wayland::data_device::set_data_device_selection,
};
use space::{CachedBuffers, Visibility};
use std::{
    cell::Cell,
    ffi::OsString,
    fs,
    os::unix::io::AsRawFd,
    process::{Child, Command},
    rc::Rc,
    thread,
    time::{Duration, Instant},
};

mod client;
mod output;
mod seat;
mod server;
mod shared_state;
mod space;
mod util;

/// run the cosmic panel xdg wrapper with the provided config
pub fn xdg_wrapper<C: XdgWrapperConfig + 'static>(
    log: Logger,
    config: C,
    config_name: Option<&str>,
) -> Result<()> {
    let mut event_loop = calloop::EventLoop::<(GlobalState<C>, Display)>::try_new().unwrap();
    let loop_handle = event_loop.handle();
    let (embedded_server_state, mut display, (sockets_left, sockets_center, sockets_right)) =
        server::new_server(loop_handle.clone(), config.clone(), log.clone())?;
    let (desktop_client_state, outputs) = client::new_client(
        loop_handle.clone(),
        config.clone(),
        log.clone(),
        &mut display,
        &embedded_server_state,
    )?;

    let global_state = GlobalState {
        desktop_client_state,
        embedded_server_state,
        loop_signal: event_loop.get_signal(),
        outputs,
        log: log.clone(),
        start_time: std::time::Instant::now(),
        cached_buffers: CachedBuffers::new(log.clone()),
    };

    let mut children = Iter::new(default_paths())
        .filter_map(|path| {
            config
                .plugins_left()
                .unwrap_or_default()
                .iter()
                .zip(&sockets_left)
                .chain(
                    config
                        .plugins_center()
                        .unwrap_or_default()
                        .iter()
                        .zip(&sockets_center),
                )
                .chain(
                    config
                        .plugins_right()
                        .unwrap_or_default()
                        .iter()
                        .zip(&sockets_right),
                )
                .find(|((app_file_name, _), _)| {
                    Some(OsString::from(&app_file_name).as_os_str()) == path.file_stem()
                })
                .and_then(|(_, (_, client_socket))| {
                    let raw_fd = client_socket.as_raw_fd();
                    let fd_flags = fcntl::FdFlag::from_bits(
                        fcntl::fcntl(raw_fd, fcntl::FcntlArg::F_GETFD).unwrap(),
                    )
                    .unwrap();
                    fcntl::fcntl(
                        raw_fd,
                        fcntl::FcntlArg::F_SETFD(fd_flags.difference(fcntl::FdFlag::FD_CLOEXEC)),
                    )
                    .unwrap();
                    fs::read_to_string(&path).ok().and_then(|bytes| {
                        if let Ok(entry) = DesktopEntry::decode(&path, &bytes) {
                            if let Some(exec) = entry.exec() {
                                let requests_host_wayland_display = entry.desktop_entry("HostWaylandDisplay").is_some();
                                return Some(exec_child(exec, config_name, log.clone(), raw_fd, requests_host_wayland_display));
                            }
                        }
                        None
                    })
                })
        })
        .collect_vec();

    let mut shared_data = (global_state, display);
    let mut last_cleanup = Instant::now();
    let five_min = Duration::from_secs(300);

    // TODO find better place for this
    let set_clipboard_once = Rc::new(Cell::new(false));

    loop {
        // cleanup popup manager
        if last_cleanup.elapsed() > five_min {
            shared_data
                .0
                .embedded_server_state
                .popup_manager
                .borrow_mut()
                .cleanup();
            last_cleanup = Instant::now();
        }

        // dispatch desktop client events
        let dispatch_client_res = event_loop.dispatch(Duration::from_millis(16), &mut shared_data);

        dispatch_client_res.expect("Failed to dispatch events");

        let (shared_data, server_display) = &mut shared_data;

        // rendering
        {
            let display = &mut shared_data.desktop_client_state.display;
            display.flush().unwrap();

            let space = &mut shared_data.desktop_client_state.space;

            // FIXME
            // space_manager.apply_display(server_display);
            let _ = space.handle_events(
                shared_data.start_time.elapsed().as_millis().try_into().unwrap(),
                &mut children,
                &shared_data.desktop_client_state.focused_surface,
            );
        }

        // dispatch server events
        {
            server_display
                .dispatch(Duration::from_millis(16), shared_data)
                .unwrap();
            server_display.flush_clients(shared_data);
        }

        // TODO find better place for this
        // the idea is to forward clipbard as soon as possible just once
        // this method is not ideal...
        if !set_clipboard_once.get() {
            let desktop_client_state = &shared_data.desktop_client_state;
            for s in &desktop_client_state.seats {
                let server_seat = &s.server.0;
                let _ = desktop_client_state.env_handle.with_data_device(
                    &s.client.seat,
                    |data_device| {
                        data_device.with_selection(|offer| {
                            if let Some(offer) = offer {
                                offer.with_mime_types(|types| {
                                    set_data_device_selection(server_seat, types.into());
                                    set_clipboard_once.replace(true);
                                })
                            }
                        })
                    },
                );
            }
        }

        if children
            .iter_mut()
            .map(|c| c.try_wait())
            .all(|r| matches!(r, Ok(Some(_))))
        {
            return Ok(());
        }

        // sleep if not focused...
        if matches!(shared_data.desktop_client_state.space.visibility, Visibility::Hidden) {
            thread::sleep(Duration::from_millis(60));
        }
    }
}

fn exec_child(c: &str, config_name: Option<&str>, log: Logger, raw_fd: i32, requests_host_wayland_display: bool) -> Child {
    let mut exec_iter = Shlex::new(c);
    let exec = exec_iter
        .next()
        .expect("exec parameter must contain at least on word");
    trace!(log, "child: {}", &exec);

    let mut child = Command::new(exec);
    for arg in exec_iter {
        trace!(log, "child argument: {}", &arg);
        child.arg(arg);
    }
    if let Some(config_name) = config_name {
        child.env("COSMIC_DOCK_CONFIG", config_name);
    }

    if requests_host_wayland_display {
        if let Ok(display) = std::env::var("WAYLAND_DISPLAY") {
            child.env("HOST_WAYLAND_DISPLAY", display);
        }
    }

    child
        .env("WAYLAND_SOCKET", raw_fd.to_string())
        .env_remove("WAYLAND_DEBUG")
        // .env("WAYLAND_DEBUG", "1")
        // .stderr(std::process::Stdio::piped())
        // .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("Failed to start child process")
}
