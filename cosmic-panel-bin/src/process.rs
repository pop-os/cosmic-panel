// SPDX-License-Identifier: GPL-3.0-only
use anyhow::{bail, Result};
use smithay::reexports::nix::fcntl;
use std::os::unix::{net::UnixStream, prelude::*};

pub(crate) fn mark_as_not_cloexec(file: &impl AsFd) -> Result<()> {
    let raw_fd = file.as_fd().as_raw_fd();
    let Some(fd_flags) = fcntl::FdFlag::from_bits(fcntl::fcntl(raw_fd, fcntl::FcntlArg::F_GETFD)?) else {
        bail!("failed to get fd flags from file");
    };
    fcntl::fcntl(
        raw_fd,
        fcntl::FcntlArg::F_SETFD(fd_flags.difference(fcntl::FdFlag::FD_CLOEXEC)),
    )?;
    Ok(())
}

pub(crate) fn mark_as_cloexec(file: &impl AsFd) -> Result<()> {
    let raw_fd = file.as_fd().as_raw_fd();
    let Some(fd_flags) = fcntl::FdFlag::from_bits(fcntl::fcntl(raw_fd, fcntl::FcntlArg::F_GETFD)?) else {
        bail!("failed to get fd flags from file");
    };
    fcntl::fcntl(
        raw_fd,
        fcntl::FcntlArg::F_SETFD(fd_flags.union(fcntl::FdFlag::FD_CLOEXEC)),
    )?;
    Ok(())
}

pub fn create_socket() -> Result<(OwnedFd, OwnedFd)> {
    // Create a new pair of unnamed Unix sockets
    let (sock_1, sock_2) = UnixStream::pair()?;

    // Turn the other socket into a non-blocking fd, which we can pass to the child
    // process
    sock_1.set_nonblocking(false)?;
    let fd_1 = OwnedFd::from(sock_1);

    sock_2.set_nonblocking(false)?;
    let fd_2 = OwnedFd::from(sock_2);
    Ok((fd_1, fd_2))
}
