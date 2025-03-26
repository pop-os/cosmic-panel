use std::{
    os::{
        linux::net::SocketAddrExt,
        unix::net::{SocketAddr, UnixListener, UnixStream},
    },
    sync::{Arc, Mutex},
};

use cctk::wayland_client::{
    delegate_dispatch,
    globals::{BindError, GlobalList},
    Dispatch, Proxy, QueueHandle,
};
use rand::distributions::{Alphanumeric, DistString};
use rustix::fd::AsFd;
use sctk::globals::GlobalData;

use wayland_protocols::wp::security_context::v1::client::{
    wp_security_context_manager_v1::WpSecurityContextManagerV1,
    wp_security_context_v1::WpSecurityContextV1,
};

use crate::xdg_shell_wrapper::{shared_state::GlobalState, space::WrapperSpace};

#[derive(Debug, Clone)]
pub struct SecurityContextManager {
    pub manager: WpSecurityContextManagerV1,
}

/// Security Context data.
#[derive(Debug)]
pub struct SecurityContext {
    pub conn: Arc<Mutex<Option<UnixStream>>>,
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
    ) -> std::io::Result<WpSecurityContextV1> {
        // create a close fd that we can use to close the listener
        let (close_fd_ours, close_fd) = rustix::pipe::pipe()?;
        let s: String = Alphanumeric.sample_string(&mut rand::thread_rng(), 50);
        let addr = SocketAddr::from_abstract_name(s)?;
        // this also listens on the socket
        let listener = UnixListener::bind_addr(&addr)?;
        let wp_security_context = self.manager.create_listener(
            listener.as_fd(),
            close_fd.as_fd(),
            qh,
            SecurityContext { conn: Arc::new(Mutex::new(None)) },
        );
        let conn = UnixStream::connect_addr(&addr)?;
        // XXX make sure no one else can connect to the listener
        drop(close_fd_ours);

        // we need to store the connection somewhere
        {
            let data = wp_security_context.data::<SecurityContext>().unwrap();
            let mut guard = data.conn.lock().unwrap();
            *guard = Some(conn);
        }

        // XXX the compositor will close the listener fd
        Box::leak(Box::new(listener));

        Ok(wp_security_context)
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
