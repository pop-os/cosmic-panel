use crate::{
    space_container::SpaceContainer,
    xdg_shell_wrapper::{
        server_state::ServerState, shared_state::GlobalState, space::WrapperSpace,
    },
};
use cctk::{
    cosmic_protocols::overlap_notify, toplevel_info::ToplevelInfoState,
    toplevel_management::ToplevelManagerState, wayland_client::protocol::wl_pointer::WlPointer,
    workspace::WorkspaceState,
};
use sctk::{
    compositor::CompositorState,
    data_device_manager::{
        data_device::DataDevice,
        data_offer::{DragOffer, SelectionOffer},
        data_source::{CopyPasteSource, DragSource},
        DataDeviceManagerState,
    },
    output::OutputState,
    reexports::{
        calloop_wayland_source::WaylandSource,
        client::{
            globals::registry_queue_init,
            protocol::{
                wl_keyboard,
                wl_output::WlOutput,
                wl_seat::WlSeat,
                wl_surface::{self, WlSurface},
            },
            Connection, QueueHandle,
        },
    },
    registry::RegistryState,
    seat::{
        pointer::{PointerEvent, ThemedPointer},
        SeatState,
    },
    shell::{
        wlr_layer::{LayerShell, LayerSurface},
        xdg::XdgShell,
    },
    shm::{multi::MultiPool, Shm},
    subcompositor::SubcompositorState,
};
use smithay::{
    backend::{
        egl::EGLSurface,
        renderer::{
            damage::OutputDamageTracker,
            element::{surface::WaylandSurfaceRenderElement, AsRenderElements},
            gles::GlesRenderer,
            Bind,
        },
    },
    desktop::LayerSurface as SmithayLayerSurface,
    output::Output,
    reexports::{
        calloop,
        wayland_server::{
            backend::{ClientData, ClientId, DisconnectReason, GlobalId},
            protocol::{wl_output, wl_surface::WlSurface as SmithayWlSurface},
        },
    },
    utils::{Logical, Size},
    wayland::compositor::CompositorClientState,
};
use std::{
    cell::RefCell,
    collections::HashMap,
    fmt::Debug,
    rc::Rc,
    time::{Duration, Instant},
};
use tracing::error;
use wayland_protocols::wp::{
    fractional_scale::v1::client::wp_fractional_scale_v1::WpFractionalScaleV1,
    viewporter::client::wp_viewport::WpViewport,
};

use super::handlers::{
    overlap::OverlapNotifyV1, wp_fractional_scaling::FractionalScalingManager,
    wp_security_context::SecurityContextManager, wp_viewporter::ViewporterState,
};

#[derive(Debug)]
pub(crate) struct ClientSeat {
    pub(crate) _seat: WlSeat,
    pub(crate) kbd: Option<wl_keyboard::WlKeyboard>,
    pub(crate) ptr: Option<ThemedPointer>,
    pub(crate) last_enter: u32,
    pub(crate) last_key_press: (u32, u32),
    pub(crate) last_pointer_press: (u32, u32),
    pub(crate) data_device: DataDevice,
    pub(crate) copy_paste_source: Option<CopyPasteSource>,
    pub(crate) dnd_source: Option<DragSource>,
    pub(crate) selection_offer: Option<SelectionOffer>,
    pub(crate) dnd_offer: Option<DragOffer>,
    pub(crate) next_selection_offer_is_mine: bool,
    pub(crate) next_dnd_offer_is_mine: bool,
    pub(crate) dnd_icon:
        Option<(Option<EGLSurface>, WlSurface, OutputDamageTracker, bool, Option<u32>)>,
}

impl ClientSeat {
    pub fn get_serial_of_last_seat_event(&self) -> u32 {
        let (key_serial, key_time) = self.last_key_press;
        let (pointer_serial, pointer_time) = self.last_pointer_press;
        if key_time > pointer_time {
            key_serial
        } else {
            pointer_serial
        }
    }
}

#[derive(Debug, Copy, Clone)]
/// status of a focus
pub enum FocusStatus {
    /// focused
    Focused,
    /// instant last focused
    LastFocused(Instant),
}
// TODO remove refcell if possible
/// list of focused surfaces and the seats that focus them
pub type ClientFocus = Vec<(wl_surface::WlSurface, String, FocusStatus)>;

/// Wrapper client state
pub struct ClientState {
    /// state
    pub registry_state: RegistryState,
    /// state
    pub seat_state: SeatState,
    /// state
    pub output_state: OutputState,
    /// state
    pub compositor_state: CompositorState,
    pub subcompositor: SubcompositorState,
    /// state
    pub shm_state: Shm,
    /// state
    pub xdg_shell_state: XdgShell,
    /// state
    pub layer_state: LayerShell,
    /// data device manager state
    pub data_device_manager: DataDeviceManagerState,
    /// fractional scaling manager
    pub fractional_scaling_manager: Option<FractionalScalingManager>,
    /// viewporter
    pub viewporter_state: Option<ViewporterState>,
    /// toplevel_info_state
    pub toplevel_info_state: Option<ToplevelInfoState>,
    /// toplevel_manager_state
    pub toplevel_manager_state: Option<ToplevelManagerState>,
    /// toplevel_manager_state
    pub workspace_state: Option<WorkspaceState>,
    /// security context manager
    pub security_context_manager: Option<SecurityContextManager>,
    /// overlap notifications subscription
    pub overlap_notify: Option<OverlapNotifyV1>,

    pub(crate) connection: Connection,
    /// queue handle
    pub queue_handle: QueueHandle<GlobalState>, // TODO remove if never used
    /// state regarding the last embedded client surface with keyboard focus
    pub focused_surface: Rc<RefCell<ClientFocus>>,
    /// state regarding the last embedded client surface with keyboard focus
    pub hovered_surface: Rc<RefCell<ClientFocus>>,
    pub(crate) cursor_surface: Option<wl_surface::WlSurface>,
    pub(crate) multipool: Option<MultiPool<(WlSurface, usize)>>,
    pub(crate) multipool_ctr: usize,
    pub(crate) last_key_pressed: Vec<(String, (u32, u32), wl_surface::WlSurface)>,
    pub(crate) outputs: Vec<(WlOutput, Output, GlobalId)>,

    pub delayed_surface_motion: HashMap<SmithayWlSurface, (PointerEvent, WlPointer, u128)>,

    pub(crate) pending_layer_surfaces: Vec<(
        smithay::wayland::shell::wlr_layer::LayerSurface,
        Option<wl_output::WlOutput>,
        String,
    )>,
    pub(crate) proxied_layer_surfaces: Vec<(
        EGLSurface,
        OutputDamageTracker,
        SmithayLayerSurface,
        LayerSurface,
        SurfaceState,
        f64,
        Option<WpFractionalScaleV1>,
        Option<WpViewport>,
    )>,
}

impl Debug for ClientState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientState")
            .field("registry_state", &self.registry_state)
            .field("seat_state", &self.seat_state)
            .field("output_state", &self.output_state)
            .field("compositor_state", &self.compositor_state)
            .field("shm_state", &self.shm_state)
            .field("xdg_shell_state", &self.xdg_shell_state)
            .field("layer_state", &self.layer_state)
            .field("data_device_manager", &self.data_device_manager)
            .field("fractional_scaling_manager", &self.fractional_scaling_manager)
            .field("viewporter_state", &self.viewporter_state)
            .field("toplevel_info_state", &self.toplevel_info_state)
            .field("toplevel_manager_state", &())
            .field("connection", &self.connection)
            .field("queue_handle", &self.queue_handle)
            .field("focused_surface", &self.focused_surface)
            .field("hovered_surface", &self.hovered_surface)
            .field("cursor_surface", &self.cursor_surface)
            .field("multipool", &self.multipool)
            .field("multipool_ctr", &self.multipool_ctr)
            .field("last_key_pressed", &self.last_key_pressed)
            .field("outputs", &self.outputs)
            .field("pending_layer_surfaces", &self.pending_layer_surfaces)
            .field("proxied_layer_surfaces", &self.proxied_layer_surfaces)
            .finish()
    }
}

#[derive(Debug, Default)]
/// client compositor state
pub struct WrapperClientCompositorState {
    /// compositor state
    pub compositor_state: CompositorClientState,
}
impl ClientData for WrapperClientCompositorState {
    /// Notification that a client was initialized
    fn initialized(&self, _client_id: ClientId) {}

    /// Notification that a client is disconnected
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum SurfaceState {
    WaitingFirst(u32, Size<i32, Logical>),
    Waiting(u32, Size<i32, Logical>),
    Dirty(u32),
}

impl ClientState {
    /// Create a new client state
    pub fn new(
        loop_handle: calloop::LoopHandle<'static, GlobalState>,
        space: &mut SpaceContainer,
        _embedded_server_state: &mut ServerState,
    ) -> anyhow::Result<Self> {
        // Initial setup
        let connection = Connection::connect_to_env()?;

        let (globals, event_queue) = registry_queue_init(&connection).unwrap();
        let qh = event_queue.handle();
        let registry_state = RegistryState::new(&globals);

        let (viewporter_state, fractional_scaling_manager) =
            match FractionalScalingManager::new(&globals, &qh) {
                Ok(m) => {
                    let viewporter_state = match ViewporterState::new(&globals, &qh) {
                        Ok(s) => Some(s),
                        Err(why) => {
                            error!(?why, "Failed to initialize viewporter");
                            None
                        },
                    };
                    (viewporter_state, Some(m))
                },
                Err(why) => {
                    error!(?why, "Failed to initialize fractional scaling manager");
                    (None, None)
                },
            };
        let security_context_manager = match SecurityContextManager::new(&globals, &qh) {
            Err(why) => {
                error!(?why, "Failed to initialize security context manager");
                None
            },
            Ok(m) => Some(m),
        };
        let overlap_notify = OverlapNotifyV1::bind(&globals, &qh);
        if let Err(err) = &overlap_notify {
            tracing::warn!("Failed to bind to overlap notify {err:?}");
        }
        let compositor_state =
            CompositorState::bind(&globals, &qh).expect("wl_compositor not available");

        let client_state = ClientState {
            focused_surface: space.get_client_focused_surface(),
            hovered_surface: space.get_client_hovered_surface(),
            proxied_layer_surfaces: Vec::new(),
            pending_layer_surfaces: Vec::new(),

            queue_handle: qh.clone(),
            connection: connection.clone(),
            seat_state: SeatState::new(&globals, &qh),
            output_state: OutputState::new(&globals, &qh),
            subcompositor: SubcompositorState::bind(
                compositor_state.wl_compositor().clone(),
                &globals,
                &qh,
            )
            .expect("wl_subsureface not available"),
            compositor_state,
            shm_state: Shm::bind(&globals, &qh).expect("wl_shm not available"),
            xdg_shell_state: XdgShell::bind(&globals, &qh).expect("xdg shell not available"),
            layer_state: LayerShell::bind(&globals, &qh).expect("layer shell is not available"),
            data_device_manager: DataDeviceManagerState::bind(&globals, &qh)
                .expect("data device manager is not available"),
            overlap_notify: overlap_notify.ok(),

            outputs: Default::default(),
            registry_state,
            multipool: None,
            multipool_ctr: 0,
            cursor_surface: None,
            last_key_pressed: Vec::new(),
            fractional_scaling_manager,
            viewporter_state,
            toplevel_info_state: None,
            toplevel_manager_state: None,
            workspace_state: None,
            security_context_manager,
            delayed_surface_motion: HashMap::new(),
        };

        WaylandSource::new(connection, event_queue).insert(loop_handle).unwrap();

        Ok(client_state)
    }

    /// draw the proxied layer shell surfaces
    pub fn draw_layer_surfaces(&mut self, renderer: &mut GlesRenderer, time: u32) {
        let clear_color = &[0.0, 0.0, 0.0, 0.0];
        for (egl_surface, dmg_tracked_renderer, s_layer, c_layer, state, scale, ..) in
            &mut self.proxied_layer_surfaces
        {
            let gen = match state {
                SurfaceState::WaitingFirst(_, _) => continue,
                SurfaceState::Waiting(_, _) => continue,
                SurfaceState::Dirty(gen) => gen,
            };
            _ = unsafe { renderer.egl_context().make_current_with_surface(egl_surface) };
            let age = egl_surface.buffer_age().unwrap_or_default() as usize;
            let Ok(mut f) = renderer.bind(egl_surface) else {
                continue;
            };
            let elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                s_layer.render_elements(renderer, (0, 0).into(), (*scale).into(), 1.0);
            dmg_tracked_renderer
                .render_output(renderer, &mut f, age, &elements, *clear_color)
                .unwrap();
            drop(f);
            egl_surface.swap_buffers(None).unwrap();
            // FIXME: damage tracking issues on integrated graphics but not nvidia
            // self.egl_surface
            //     .as_ref()
            //     .unwrap()
            //     .swap_buffers(res.0.as_deref_mut())?;

            // TODO what if there is "no output"?
            for o in &self.outputs {
                let output = &o.1;
                s_layer.send_frame(&o.1, Duration::from_millis(time as u64), None, move |_, _| {
                    Some(output.clone())
                })
            }
            *state = SurfaceState::Waiting(*gen, s_layer.bbox().size);
        }
    }

    /// initialize the toplevel info state
    pub fn init_toplevel_info_state(&mut self) {
        self.toplevel_info_state =
            ToplevelInfoState::try_new(&self.registry_state, &self.queue_handle);
    }

    /// initialize the toplevel manager state
    pub fn init_toplevel_manager_state(&mut self) {
        self.toplevel_manager_state =
            ToplevelManagerState::try_new(&self.registry_state, &self.queue_handle);
    }

    /// initialize the toplevel manager state
    pub fn init_workspace_state(&mut self) {
        self.workspace_state = Some(WorkspaceState::new(&self.registry_state, &self.queue_handle));
    }
}
