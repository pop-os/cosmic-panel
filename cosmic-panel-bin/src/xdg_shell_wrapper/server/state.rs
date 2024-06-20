use smithay::{
    desktop::PopupManager,
    input::{Seat, SeatState},
    reexports::wayland_server::{
        protocol::{wl_data_source::WlDataSource, wl_surface::WlSurface},
        DisplayHandle,
    },
    utils::{Logical, Point},
    wayland::{
        compositor::CompositorState,
        dmabuf::{DmabufGlobal, DmabufState},
        fractional_scale::FractionalScaleManagerState,
        output::OutputManagerState,
        selection::{data_device::DataDeviceState, primary_selection::PrimarySelectionState},
        shell::{wlr_layer::WlrLayerShellState, xdg::XdgShellState},
        shm::ShmState,
    },
};

use crate::xdg_shell_wrapper::{client_state::ClientSeat, shared_state::GlobalState};

/// list of focused surfaces and the seats that focus them

pub type ServerFocus = Vec<(WlSurface, String)>;
#[allow(missing_debug_implementations)]

/// Information for tracking the server pointer focus
#[derive(Debug, Clone)]
pub struct ServerPointerFocus {
    /// focused wl surface
    pub surface: WlSurface,
    /// name of the seat which is focusing
    pub seat_name: String,
    /// location in compositor space for the layer shell surface or popup
    pub c_pos: Point<i32, Logical>,
    /// location of the focused embedded surface in compositor space
    pub s_pos: Point<f64, Logical>,
}

/// helper type for focus
pub type ServerPtrFocus = Vec<ServerPointerFocus>;

#[allow(missing_debug_implementations)]
/// internal server state
pub struct ServerState {
    /// popup manager
    pub popup_manager: PopupManager,
    pub(crate) display_handle: DisplayHandle,
    // pub(crate) selected_data_provider: SelectedDataProvider,
    pub(crate) last_button: Option<u32>,
    pub(crate) seats: Vec<SeatPair>,
    // Smithay State
    pub(crate) compositor_state: CompositorState,
    pub(crate) xdg_shell_state: XdgShellState,
    pub(crate) shm_state: ShmState,
    pub(crate) _output_manager_state: OutputManagerState,
    pub(crate) seat_state: SeatState<GlobalState>,
    pub(crate) data_device_state: DataDeviceState,
    pub(crate) dmabuf_state: Option<(DmabufState, DmabufGlobal)>,
    pub(crate) primary_selection_state: PrimarySelectionState,
    pub(crate) layer_shell_state: WlrLayerShellState,
    pub(crate) _fractional_scale_state: FractionalScaleManagerState,
}

impl ServerState {
    /// create a new server state
    pub fn new(dh: DisplayHandle) -> ServerState {
        ServerState {
            popup_manager: PopupManager::default(),
            display_handle: dh.clone(),
            last_button: None,
            seats: Vec::new(),
            compositor_state: CompositorState::new::<GlobalState>(&dh),
            xdg_shell_state: XdgShellState::new::<GlobalState>(&dh),
            shm_state: ShmState::new::<GlobalState>(&dh, vec![]),
            _output_manager_state: OutputManagerState::new_with_xdg_output::<GlobalState>(&dh),
            seat_state: SeatState::new(),
            data_device_state: DataDeviceState::new::<GlobalState>(&dh),
            primary_selection_state: PrimarySelectionState::new::<GlobalState>(&dh),
            layer_shell_state: WlrLayerShellState::new::<GlobalState>(&dh),
            _fractional_scale_state: FractionalScaleManagerState::new::<GlobalState>(&dh),
            dmabuf_state: None,
        }
    }
}

pub(crate) struct ServerSeat {
    pub(crate) seat: Seat<GlobalState>,
    pub(crate) selection_source: Option<WlDataSource>,
    pub(crate) dnd_source: Option<WlDataSource>,
    pub(crate) dnd_icon: Option<WlSurface>,
}

pub(crate) struct SeatPair {
    pub(crate) name: String,
    pub(crate) client: ClientSeat,
    pub(crate) server: ServerSeat,
}
