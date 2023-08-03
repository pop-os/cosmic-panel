use anyhow::{Context, Result};
use cosmic_notifications_util::PANEL_NOTIFICATIONS_FD;
use smithay::reexports::nix::{fcntl, unistd};
use std::os::{
    fd::{FromRawFd, RawFd},
    unix::net::UnixStream,
};
use tracing::info;
use zbus::{dbus_proxy, zvariant::OwnedFd, ConnectionBuilder};

#[dbus_proxy(
    default_service = "com.system76.NotificationsSocket",
    interface = "com.system76.NotificationsSocket",
    default_path = "/com/system76/NotificationsSocket"
)]
trait NotificationsSocket {
    /// get an fd for an applet
    fn get_fd(&self) -> zbus::Result<OwnedFd>;
}
pub async fn notifications_conn() -> Result<NotificationsSocketProxy<'static>> {
    info!("Connecting to notifications daemon");
    let fd_num = std::env::var(PANEL_NOTIFICATIONS_FD)?;
    let fd = fd_num.parse::<RawFd>()?;
    // set CLOEXEC
    let flags = fcntl::fcntl(fd, fcntl::FcntlArg::F_GETFD);
    let result = flags
        .map(|f| fcntl::FdFlag::from_bits(f).unwrap() | fcntl::FdFlag::FD_CLOEXEC)
        .and_then(|f| fcntl::fcntl(fd, fcntl::FcntlArg::F_SETFD(f)));
    let daemon_stream = match result {
        // CLOEXEC worked and we can startup with session IPC
        Ok(_) => unsafe { UnixStream::from_raw_fd(fd) },
        // CLOEXEC didn't work, something is wrong with the fd, just close it
        Err(err) => {
            let _ = unistd::close(fd);
            return Err(err).with_context(|| "Failed to setup session socket");
        }
    };
    daemon_stream.set_nonblocking(true)?;

    let stream = tokio::net::UnixStream::from_std(daemon_stream)?;
    let conn = ConnectionBuilder::socket(stream).p2p().build().await?;
    info!("Made socket connection");
    let proxy = NotificationsSocketProxy::new(&conn).await?;
    info!("Connected to notifications");

    Ok(proxy)
}
