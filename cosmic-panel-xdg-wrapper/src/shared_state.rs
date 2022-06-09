// SPDX-License-Identifier: MPL-2.0-only

use once_cell::sync::OnceCell;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::space::SpaceManager;
use crate::{client::Env, CachedBuffers};
use cosmic_panel_config::config::XdgWrapperConfig;
use sctk::{
    environment::Environment,
    output::OutputStatusListener,
    reexports::{
        client::{
            self,
            protocol::{
                wl_keyboard as c_wl_keyboard, wl_output as c_wl_output, wl_pointer as c_wl_pointer,
                wl_seat as c_wl_seat, wl_shm as c_wl_shm, wl_surface as c_wl_surface,
            },
            Attached,
        },
        protocols::xdg_shell::client::xdg_wm_base::XdgWmBase,
    },
};
use slog::Logger;
use smithay::{
    desktop::{PopupManager, Window},
    reexports::{
        calloop,
        wayland_server::{
            self,
            protocol::{wl_output, wl_pointer::AxisSource, wl_seat::WlSeat, wl_surface::WlSurface},
            Global,
        },
    },
    wayland::{output::Output, seat, shell::xdg::ShellState},
};

#[derive(Debug)]
pub struct Seat {
    pub(crate) name: String,
    pub(crate) client: ClientSeat,
    pub(crate) server: (seat::Seat, Global<WlSeat>),
}

#[derive(Debug)]
pub struct ClientSeat {
    pub(crate) seat: Attached<c_wl_seat::WlSeat>,
    pub(crate) kbd: Option<c_wl_keyboard::WlKeyboard>,
    pub(crate) ptr: Option<c_wl_pointer::WlPointer>,
}

pub type OutputGroup = (
    Output,
    Global<wl_output::WlOutput>,
    String,
    c_wl_output::WlOutput,
);

#[derive(Debug, Default)]
pub struct AxisFrameData {
    pub(crate) frame: Option<seat::AxisFrame>,
    pub(crate) source: Option<AxisSource>,
    pub(crate) h_discrete: Option<i32>,
    pub(crate) v_discrete: Option<i32>,
}

pub struct GlobalState<C: XdgWrapperConfig + 'static> {
    pub(crate) desktop_client_state: DesktopClientState<C>,
    pub(crate) embedded_server_state: EmbeddedServerState,
    pub(crate) loop_signal: calloop::LoopSignal,
    pub(crate) outputs: Vec<OutputGroup>,
    pub(crate) log: Logger,
    pub(crate) start_time: std::time::Instant,
    pub(crate) cached_buffers: CachedBuffers,
}
pub struct SelectedDataProvider {
    pub(crate) seat: Rc<RefCell<Option<Attached<c_wl_seat::WlSeat>>>>,
    pub(crate) env_handle: Rc<OnceCell<Environment<Env>>>,
}

pub struct EmbeddedServerState {
    pub(crate) clients_left: Vec<(u32, wayland_server::Client)>,
    pub(crate) clients_center: Vec<(u32, wayland_server::Client)>,
    pub(crate) clients_right: Vec<(u32, wayland_server::Client)>,
    pub(crate) shell_state: Arc<Mutex<ShellState>>,
    pub(crate) root_window: Option<Rc<RefCell<Window>>>,
    pub(crate) focused_surface: Rc<RefCell<Option<WlSurface>>>,
    pub(crate) popup_manager: Rc<RefCell<PopupManager>>,
    pub(crate) selected_data_provider: SelectedDataProvider,
    pub(crate) last_button: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Focus {
    Current(c_wl_surface::WlSurface),
    LastFocus(Instant),
}

pub struct DesktopClientState<C: XdgWrapperConfig> {
    pub(crate) display: client::Display,
    pub(crate) seats: Vec<Seat>,
    pub(crate) space_manager: SpaceManager<C>,
    pub(crate) cursor_surface: c_wl_surface::WlSurface,
    pub(crate) axis_frame: AxisFrameData,
    pub(crate) kbd_focus: bool,
    pub(crate) shm: Attached<c_wl_shm::WlShm>,
    pub(crate) xdg_wm_base: Attached<XdgWmBase>,
    pub(crate) env_handle: Environment<Env>,
    pub(crate) last_input_serial: Option<u32>,
    pub(crate) focused_surface: Focus,
}
