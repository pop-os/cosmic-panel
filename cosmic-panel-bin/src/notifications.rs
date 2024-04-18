use anyhow::{Context, Result};
use cosmic_notifications_util::PANEL_NOTIFICATIONS_FD;
use smithay::reexports::rustix::{
    io::{fcntl_getfd, fcntl_setfd, FdFlags},
    {self},
};
use std::os::{
    fd::{FromRawFd, OwnedFd, RawFd},
    unix::net::UnixStream,
};
use tracing::info;
use zbus::{connection::Builder, proxy};

#[proxy(
    default_service = "com.system76.NotificationsSocket",
    interface = "com.system76.NotificationsSocket",
    default_path = "/com/system76/NotificationsSocket"
)]
trait NotificationsSocket {
    /// get an fd for an applet
    fn get_fd(&self) -> zbus::Result<zbus::zvariant::OwnedFd>;
}
pub async fn notifications_conn() -> Result<NotificationsSocketProxy<'static>> {
    info!("Connecting to notifications daemon");
    let fd_num = std::env::var(PANEL_NOTIFICATIONS_FD)?;
    let fd = fd_num.parse::<RawFd>()?;
    let fd = unsafe { rustix::fd::OwnedFd::from_raw_fd(fd) };

    let res = fcntl_getfd(&fd).and_then(|flags| fcntl_setfd(&fd, FdFlags::CLOEXEC.union(flags)));

    let daemon_stream = match res {
        // CLOEXEC worked and we can startup with session IPC
        Ok(_) => UnixStream::from(OwnedFd::from(fd)),
        // CLOEXEC didn't work, something is wrong with the fd, just close it
        Err(err) => {
            return Err(err).with_context(|| "Failed to setup session socket");
        },
    };
    daemon_stream.set_nonblocking(true)?;

    let stream = tokio::net::UnixStream::from_std(daemon_stream)?;
    let conn = Builder::socket(stream).p2p().build().await?;
    info!("Made socket connection");
    let proxy = NotificationsSocketProxy::new(&conn).await?;
    info!("Connected to notifications");

    Ok(proxy)
}
