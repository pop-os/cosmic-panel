use std::{
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        unix::net::UnixStream,
    },
};

use crate::{process::mark_as_cloexec, space_container::SpaceContainer};
use anyhow::{Context, Result};
use cosmic_notifications_util::PANEL_NOTIFICATIONS_FD;
use sendfd::{RecvWithFd, SendWithFd};
use smithay::reexports::{
    calloop::{
        self, channel::SyncSender, generic::Generic, Interest, LoopHandle, Mode, PostAction,
    },
    nix::{fcntl, unistd},
};
use tracing::{error, info, trace, warn};
use xdg_shell_wrapper::shared_state::GlobalState;

pub fn init(
    loop_handle: &LoopHandle<GlobalState<SpaceContainer>>,
) -> Result<SyncSender<(String, UnixStream)>> {
    let fd_num = std::env::var(PANEL_NOTIFICATIONS_FD)?;
    let fd = fd_num.parse::<RawFd>()?;
    // set CLOEXEC
    let flags = fcntl::fcntl(fd, fcntl::FcntlArg::F_GETFD);
    let result = flags
        .map(|f| fcntl::FdFlag::from_bits(f).unwrap() | fcntl::FdFlag::FD_CLOEXEC)
        .and_then(|f| fcntl::fcntl(fd, fcntl::FcntlArg::F_SETFD(f)));
    let mut daemon_socket = match result {
        // CLOEXEC worked and we can startup with session IPC
        Ok(_) => unsafe { UnixStream::from_raw_fd(fd) },
        // CLOEXEC didn't work, something is wrong with the fd, just close it
        Err(err) => {
            let _ = unistd::close(fd);
            return Err(err).with_context(|| "Failed to setup session socket");
        }
    };

    daemon_socket
        .set_nonblocking(true)
        .expect("Couldn't set nonblocking");
    // read remaining bytes from socket
    {
        let mut buf = [0u8; 128];
        while let Ok(size) = daemon_socket.read(&mut buf) {
            if size == 0 {
                break;
            }
        }
    }

    info!("Inserting channel into event loop");

    // insert channel for spaces to send pending nortification appplet to
    let (tx, rx) = calloop::channel::sync_channel(10);

    loop_handle
        .insert_source(
            rx,
            |msg: calloop::channel::Event<(String, UnixStream)>, _, state| match msg {
                calloop::channel::Event::Msg(msg) => {
                    info!("Received pending notification applet for space {}", &msg.0);
                    state.space.pending_notification_applet_ids.push(msg);
                }
                calloop::channel::Event::Closed => {
                    warn!("Notification channel closed");
                }
            },
        )
        .map_err(|err| {
            anyhow::anyhow!(
                "Failed to insert notification channel into event loop: {}",
                err
            )
        })?;

    info!("Inserting session socket into event loop");
    // insert source for daemon socket
    loop_handle
        .insert_source(
            Generic::new(daemon_socket, Interest::BOTH, Mode::Level),
            move |interest, stream, state: &mut GlobalState<SpaceContainer>| {
                if interest.error {
                    error!("Error on session socket");
                    return Ok(PostAction::Remove);
                }

                if interest.readable {
                    if let Err(err) = read_socket(stream, state) {
                        error!("Error reading from session socket: {}", err);
                        return Ok(PostAction::Continue);
                    }
                } else if interest.writable {
                    if let Err(err) = write_socket(stream, state) {
                        error!("Error writing to session socket: {}", err);
                        return Ok(PostAction::Continue);
                    }
                }

                Ok(PostAction::Continue)
            },
        )
        .with_context(|| "Failed to init the cosmic session socket source")?;

    Ok(tx)
}

fn read_socket(stream: &mut UnixStream, state: &mut GlobalState<SpaceContainer>) -> Result<()> {
    // every message is a u32 id, and a socket fd
    let mut buf = [0u8; 4];
    let mut fd_buf = [0i32; 1];
    while let Ok((msg_cnt, fd_cnt)) = stream.recv_with_fd(&mut buf, &mut fd_buf) {
        if fd_cnt == 0 {
            break;
        }
        let fd = unsafe { OwnedFd::from_raw_fd(fd_buf[0]) };
        mark_as_cloexec(&fd)?;

        if msg_cnt == 0 {
            break;
        }
        let id = u32::from_ne_bytes(buf);
        let Some(applets_msg_stream) = state.space.notification_applet_ids.remove(&id) else {
            continue;
        };
        // send the fd and the applet id to the applet
        let raw = fd.as_raw_fd();
        info!("Sending fd {} to applet {}", raw, id);
        if let Err(err) = applets_msg_stream.send_with_fd(bytemuck::bytes_of(&id), &[raw]) {
            error!("Failed to send fd to applet: {}", err);
        };
    }
    Ok(())
}

fn write_socket(stream: &mut UnixStream, state: &mut GlobalState<SpaceContainer>) -> Result<()> {
    for (space, applet_stream) in state.space.pending_notification_applet_ids.drain(..) {
        if state.space.notification_applet_spaces.contains(&space) {
            continue;
        }
        info!("Writing to notification socket for space {}", space);
        let id: u32 = state
            .space
            .notification_applet_ids
            .keys()
            .max()
            .unwrap_or(&0)
            + 1;
        state
            .space
            .notification_applet_ids
            .insert(id, applet_stream);
        let mut buf = id.to_ne_bytes();

        stream.write(&mut buf)?;
        stream.flush()?;
        state.space.notification_applet_spaces.insert(space);
    }
    Ok(())
}
