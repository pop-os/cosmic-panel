use std::io;
use std::os::unix::io::AsFd;
use std::os::unix::net::{SocketAddr, UnixListener, UnixStream};
use std::sync::{Arc, Mutex};

use cctk::wayland_client::globals::{BindError, GlobalList};
use cctk::wayland_client::{Dispatch, Proxy, QueueHandle, delegate_dispatch};
use rand::distr::{Alphanumeric, SampleString};
use sctk::globals::GlobalData;

use wayland_protocols::wp::security_context::v1::client::wp_security_context_manager_v1::WpSecurityContextManagerV1;
use wayland_protocols::wp::security_context::v1::client::wp_security_context_v1::WpSecurityContextV1;

use crate::xdg_shell_wrapper::shared_state::GlobalState;
use crate::xdg_shell_wrapper::space::WrapperSpace;

const NAME_LEN: usize = 50;

#[derive(Debug, Clone)]
pub struct SecurityContextManager {
    pub manager: WpSecurityContextManagerV1,
}

/// Security Context data.
#[derive(Debug)]
pub struct SecurityContext {
    pub conn: Arc<Mutex<Option<(UnixStream, UnixListener)>>>,
}

impl Drop for SecurityContext {
    fn drop(&mut self) {
        let mut guard = self.conn.lock().unwrap();
        if let Some((stream, _)) = guard.take() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

impl SecurityContextManager {
    /// Create new security context manager.
    pub fn new(
        globals: &GlobalList,
        queue_handle: &QueueHandle<GlobalState>,
    ) -> Result<Self, BindError> {
        let manager = globals.bind(queue_handle, 1..=1, GlobalData)?;
        Ok(Self { manager })
    }

    /// Create a new security context.
    pub fn create_listener<T: 'static + WrapperSpace>(
        &self,
        qh: &QueueHandle<GlobalState>,
    ) -> io::Result<WpSecurityContextV1> {
        // create a close fd that we can use to close the listener
        let (close_fd_ours, close_fd) = smithay::reexports::rustix::pipe::pipe()?;
        let (listener, conn) = self.security_sock_listener()?;
        let wp_security_context: WpSecurityContextV1 = self.manager.create_listener(
            listener.as_fd(),
            close_fd.as_fd(),
            qh,
            SecurityContext { conn: Arc::new(Mutex::new(None)) },
        );
        // XXX make sure no one else can connect to the listener
        drop(close_fd_ours);

        // we need to store the connection somewhere
        {
            let data = wp_security_context.data::<SecurityContext>().unwrap();
            let mut guard = data.conn.lock().unwrap();
            *guard = Some((conn, listener));
        }

        Ok(wp_security_context)
    }

    #[cfg(target_os = "linux")]
    fn security_sock_listener(&self) -> io::Result<(UnixListener, UnixStream)> {
        use std::os::linux::net::SocketAddrExt;

        let s: String = Alphanumeric.sample_string(&mut rand::rng(), NAME_LEN);
        let addr = SocketAddr::from_abstract_name(s)?;
        // this also listens on the socket
        let listener = UnixListener::bind_addr(&addr)?;
        let conn = UnixStream::connect_addr(&addr)?;

        Ok((listener, conn))
    }

    #[cfg(not(target_os = "linux"))]
    fn security_sock_listener(&self) -> io::Result<(UnixListener, UnixStream)> {
        let mut socket_path = std::env::var("XDG_RUNTIME_DIR")
            .map_err(|_| io::Error::new(io::ErrorKind::NotFound, "XDG_RUNTIME_DIR unset"))?;

        let prefix = ".cosmic-sec-ctx-";
        socket_path.reserve(prefix.len() + NAME_LEN);
        socket_path.push_str(prefix);
        Alphanumeric.append_string(&mut rand::rng(), &mut socket_path, NAME_LEN);

        // Bind to path, connect, and then unlink.
        // The listener stays alive because we own the descriptor and didn't call
        // close on it. The unlink prevents other clients connecting to it, like the
        // anonymous sock in the Linux code, BUT with the small possibility of TOCTOU.
        //
        // For FreeBSD specifically, there is a syscall, connectat, that offers a commiserate
        // experience to Linux but it would require bringing in libc/rustix and isn't portable to
        // other Unixes.
        let listener = UnixListener::bind(&socket_path)?;
        let conn = UnixStream::connect(&socket_path).inspect_err(|_| {
            let _ = std::fs::remove_file(&socket_path);
        })?;
        std::fs::remove_file(&socket_path)?;

        Ok((listener, conn))
    }
}

impl Dispatch<WpSecurityContextManagerV1, GlobalData, GlobalState> for WpSecurityContextManagerV1 {
    fn event(
        _state: &mut GlobalState,
        _proxy: &WpSecurityContextManagerV1,
        _event: <WpSecurityContextManagerV1 as cctk::wayland_client::Proxy>::Event,
        _data: &GlobalData,
        _conn: &cctk::wayland_client::Connection,
        _qhandle: &cctk::wayland_client::QueueHandle<GlobalState>,
    ) {
        // No events.
        unimplemented!()
    }
}

impl Dispatch<WpSecurityContextV1, SecurityContext, GlobalState> for WpSecurityContextManagerV1 {
    fn event(
        _state: &mut GlobalState,
        _proxy: &WpSecurityContextV1,
        _event: <WpSecurityContextV1 as cctk::wayland_client::Proxy>::Event,
        _data: &SecurityContext,
        _conn: &cctk::wayland_client::Connection,
        _qhandle: &cctk::wayland_client::QueueHandle<GlobalState>,
    ) {
        // No events.
        unimplemented!()
    }
}

delegate_dispatch!(GlobalState: [WpSecurityContextManagerV1: GlobalData] => WpSecurityContextManagerV1);
delegate_dispatch!( GlobalState: [WpSecurityContextV1: SecurityContext] => WpSecurityContextManagerV1);
