use std::{
    cell::{Cell, RefCell},
    os::{fd::OwnedFd, unix::net::UnixStream},
    rc::Rc,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use cctk::wayland_client::Connection;
use launch_pad::process::Process;
use sctk::{
    compositor::Region,
    output::OutputInfo,
    reexports::{
        calloop,
        client::{
            protocol::{wl_display::WlDisplay, wl_output as c_wl_output},
            Proxy, QueueHandle,
        },
    },
    shell::{
        wlr_layer::{LayerSurface, LayerSurfaceConfigure},
        xdg::XdgPositioner,
        WaylandSurface,
    },
};
use smithay::{
    backend::{
        egl::{
            context::{GlAttributes, PixelFormatRequirements},
            ffi::egl::SwapInterval,
            EGLContext,
        },
        renderer::{
            damage::OutputDamageTracker,
            element::{
                surface::WaylandSurfaceRenderElement, Element, RenderElement, UnderlyingStorage,
            },
            gles::{GlesError, GlesFrame},
            Bind, Unbind,
        },
    },
    output::Output,
    reexports::{
        wayland_protocols::xdg::shell::client::xdg_positioner::{Anchor, Gravity},
        wayland_server::{backend::ClientId, DisplayHandle},
    },
    utils::{Buffer, Physical, Rectangle},
    wayland::{
        seat::WaylandFocus,
        shell::xdg::{PopupSurface, PositionerState},
    },
};
use smithay::{
    backend::{
        egl::{display::EGLDisplay, surface::EGLSurface},
        renderer::gles::GlesRenderer,
    },
    desktop::{PopupManager, Space, Window},
    reexports::wayland_server::Client,
    utils::{Logical, Size},
};
use tokio::sync::{mpsc, oneshot};
use tracing::{error, info};
use wayland_egl::WlEglSurface;
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wayland_protocols::wp::{
    fractional_scale::v1::client::wp_fractional_scale_v1::WpFractionalScaleV1,
    security_context::v1::client::wp_security_context_v1::WpSecurityContextV1,
};
use xdg_shell_wrapper::wp_security_context::SecurityContextManager;
use xdg_shell_wrapper::{
    client_state::{ClientFocus, FocusStatus},
    server_state::{ServerFocus, ServerPtrFocus},
    shared_state::GlobalState,
    space::{
        ClientEglDisplay, ClientEglSurface, SpaceEvent, Visibility, WrapperPopup, WrapperSpace,
    },
    util::smootherstep,
};

use cosmic_panel_config::{CosmicPanelBackground, CosmicPanelConfig, PanelAnchor};

use crate::PanelCalloopMsg;

use super::corner_element::{
    init_shaders, RoundedRectangleSettings, RoundedRectangleShaderElement,
};

pub enum AppletMsg {
    NewProcess(String, Process),
    NewNotificationsProcess(String, Process, Vec<(String, String)>, Vec<OwnedFd>),
    NeedNewNotificationFd(oneshot::Sender<OwnedFd>),
    ClientSocketPair(ClientId),
    Cleanup(String),
}

pub(crate) enum PanelRenderElement {
    Wayland(WaylandSurfaceRenderElement<GlesRenderer>),
    RoundedRectangle(RoundedRectangleShaderElement),
}

impl Element for PanelRenderElement {
    fn id(&self) -> &smithay::backend::renderer::element::Id {
        match self {
            Self::Wayland(e) => e.id(),
            Self::RoundedRectangle(e) => e.id(),
        }
    }

    fn current_commit(&self) -> smithay::backend::renderer::utils::CommitCounter {
        match self {
            Self::Wayland(e) => e.current_commit(),
            Self::RoundedRectangle(e) => e.current_commit(),
        }
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        match self {
            Self::Wayland(e) => e.src(),
            Self::RoundedRectangle(e) => e.src(),
        }
    }

    fn geometry(&self, scale: smithay::utils::Scale<f64>) -> Rectangle<i32, Physical> {
        match self {
            Self::Wayland(e) => e.geometry(scale),
            Self::RoundedRectangle(e) => e.geometry(scale),
        }
    }
}

impl RenderElement<GlesRenderer> for PanelRenderElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        match self {
            Self::Wayland(e) => e.draw(frame, src, dst, damage),
            Self::RoundedRectangle(e) => e.draw(frame, src, dst, damage),
        }
    }

    fn underlying_storage(&self, renderer: &mut GlesRenderer) -> Option<UnderlyingStorage> {
        match self {
            PanelRenderElement::Wayland(e) => e.underlying_storage(renderer),
            PanelRenderElement::RoundedRectangle(e) => e.underlying_storage(renderer),
        }
    }
}

pub type Clients = Arc<Mutex<Vec<PanelClient>>>;

#[derive(Debug)]
pub struct PanelClient {
    pub name: String,
    pub client: Client,
    pub stream: Option<UnixStream>,
    pub security_ctx: Option<WpSecurityContextV1>,
    pub exec: Option<String>,
    pub minimize_priority: Option<u32>,
    pub requests_wayland_display: Option<bool>,
    pub is_notification_applet: Option<bool>,
}

impl PanelClient {
    pub fn new(name: String, client: Client, stream: Option<UnixStream>) -> Self {
        Self {
            name,
            client,
            stream,
            security_ctx: None,
            exec: None,
            minimize_priority: None,
            requests_wayland_display: None,
            is_notification_applet: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AnimatableState {
    bg_color: [f32; 4],
    border_radius: u32,
    pub expanded: f32,
    gap: u16,
}

#[derive(Debug, Clone)]
pub struct AnimateState {
    start: AnimatableState,
    end: AnimatableState,
    pub cur: AnimatableState,
    started_at: Instant,
    progress: f32,
    duration: Duration,
}

/// space for the cosmic panel
#[derive(Debug)]
pub(crate) struct PanelSpace {
    // XXX implicitly drops egl_surface first to avoid segfault
    pub(crate) egl_surface: Option<Rc<EGLSurface>>,
    pub(crate) c_display: Option<WlDisplay>,
    pub config: CosmicPanelConfig,
    pub(crate) space: Space<Window>,
    pub(crate) unmapped: Vec<Window>,
    pub(crate) damage_tracked_renderer: Option<OutputDamageTracker>,
    pub(crate) clients_left: Clients,
    pub(crate) clients_center: Clients,
    pub(crate) clients_right: Clients,
    pub(crate) last_dirty: Option<Instant>,
    // pending size of the panel
    pub(crate) pending_dimensions: Option<Size<i32, Logical>>,
    // suggested length of the panel
    pub(crate) suggested_length: Option<u32>,
    // size of the panel
    pub(crate) actual_size: Size<i32, Logical>,
    // dimensions of the layer surface
    pub(crate) dimensions: Size<i32, Logical>,
    // Logical size of the panel, with the applied animation state
    pub(crate) container_length: i32,
    pub(crate) is_dirty: bool,
    pub(crate) space_event: Rc<Cell<Option<SpaceEvent>>>,
    pub(crate) c_focused_surface: Rc<RefCell<ClientFocus>>,
    pub(crate) c_hovered_surface: Rc<RefCell<ClientFocus>>,
    pub(crate) s_focused_surface: ServerFocus,
    pub(crate) s_hovered_surface: ServerPtrFocus,
    pub(crate) visibility: Visibility,
    pub(crate) output: Option<(c_wl_output::WlOutput, Output, OutputInfo)>,
    pub(crate) s_display: Option<DisplayHandle>,
    pub(crate) layer: Option<LayerSurface>,
    pub(crate) layer_fractional_scale: Option<WpFractionalScaleV1>,
    pub(crate) layer_viewport: Option<WpViewport>,
    pub(crate) popups: Vec<WrapperPopup>,
    pub(crate) start_instant: Instant,
    pub bg_color: [f32; 4],
    pub(crate) applet_tx: mpsc::Sender<AppletMsg>,
    pub(crate) input_region: Option<Region>,
    pub(crate) panel_changed: bool,
    pub(crate) has_frame: bool,
    pub(crate) scale: f64,
    pub(crate) output_has_toplevel: bool,
    pub(crate) security_context_manager: Option<SecurityContextManager>,
    pub(crate) animate_state: Option<AnimateState>,
    pub maximized: bool,
    pub panel_tx: calloop::channel::SyncSender<PanelCalloopMsg>,
    pub(crate) minimize_applet_rect: Rectangle<i32, Logical>,
    pub(crate) panel_rect_settings: RoundedRectangleSettings,
}

impl PanelSpace {
    /// create a new space for the cosmic panel
    pub fn new(
        config: CosmicPanelConfig,
        c_focused_surface: Rc<RefCell<ClientFocus>>,
        c_hovered_surface: Rc<RefCell<ClientFocus>>,
        applet_tx: mpsc::Sender<AppletMsg>,
        mut bg_color: [f32; 4],
        s_display: DisplayHandle,
        security_context_manager: Option<SecurityContextManager>,
        conn: &Connection,
        panel_tx: calloop::channel::SyncSender<PanelCalloopMsg>,
    ) -> Self {
        bg_color[3] = config.opacity;
        let visibility = if config.autohide.is_none() {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };

        Self {
            config,
            space: Space::default(),
            unmapped: Vec::new(),
            clients_left: Default::default(),
            clients_center: Default::default(),
            clients_right: Default::default(),
            last_dirty: Default::default(),
            pending_dimensions: Default::default(),
            space_event: Default::default(),
            dimensions: Default::default(),
            suggested_length: None,
            output: Default::default(),
            s_display: Some(s_display.clone()),
            c_display: Some(conn.display().clone()),
            layer: Default::default(),
            layer_fractional_scale: Default::default(),
            layer_viewport: Default::default(),
            egl_surface: Default::default(),
            popups: Default::default(),
            visibility,
            start_instant: Instant::now(),
            c_focused_surface,
            c_hovered_surface,
            s_focused_surface: Default::default(),
            s_hovered_surface: Default::default(),
            bg_color,
            applet_tx,
            actual_size: (0, 0).into(),
            input_region: None,
            damage_tracked_renderer: None,
            is_dirty: false,
            panel_changed: false,
            has_frame: true,
            scale: 1.0,
            output_has_toplevel: false,
            security_context_manager,
            animate_state: None,
            maximized: false,
            panel_tx,
            minimize_applet_rect: Default::default(),
            container_length: 0,
            panel_rect_settings: RoundedRectangleSettings::default(),
        }
    }

    pub fn bg_color(&self) -> [f32; 4] {
        if let Some(animatable_state) = self.animate_state.as_ref() {
            animatable_state.cur.bg_color
        } else {
            self.bg_color
        }
    }

    pub fn border_radius(&self) -> u32 {
        if let Some(animatable_state) = self.animate_state.as_ref() {
            animatable_state.cur.border_radius
        } else {
            self.config.border_radius
        }
    }

    pub fn gap(&self) -> u16 {
        if let Some(animatable_state) = self.animate_state.as_ref() {
            animatable_state.cur.gap
        } else {
            self.config.get_effective_anchor_gap() as u16
        }
    }

    pub(crate) fn id(&self) -> String {
        let id = format!(
            "panel-{}-{}-{}",
            self.config.name,
            self.config.output,
            self.output
                .as_ref()
                .map(|o| o.2.name.clone().unwrap_or_default())
                .unwrap_or_default()
        );
        id
    }

    pub(crate) fn handle_focus(&mut self) {
        let (layer_surface, layer_shell_wl_surface) =
            if let Some(layer_surface) = self.layer.as_ref() {
                (layer_surface, layer_surface.wl_surface())
            } else {
                return;
            };
        let cur_hover = {
            let c_focused_surface = self.c_focused_surface.borrow();
            let c_hovered_surface = self.c_hovered_surface.borrow();
            // no transition if not configured for autohide
            let no_hover_focus = c_focused_surface
                .iter()
                .all(|f| matches!(f.2, FocusStatus::LastFocused(_)))
                && c_hovered_surface
                    .iter()
                    .all(|f| matches!(f.2, FocusStatus::LastFocused(_)));
            if self.config.autohide().is_none() {
                if no_hover_focus && self.animate_state.is_none() {
                    self.visibility = Visibility::Hidden;
                } else {
                    self.visibility = Visibility::Visible;
                }
                return;
            }

            c_hovered_surface.iter().fold(
                if self.animate_state.is_some() || !self.output_has_toplevel {
                    FocusStatus::Focused
                } else {
                    FocusStatus::LastFocused(self.start_instant)
                },
                |acc, (surface, _, f)| {
                    if self
                        .layer
                        .as_ref()
                        .map(|s| *s.wl_surface() == *surface)
                        .unwrap_or(false)
                        || self.popups.iter().any(|p| {
                            &p.c_popup.wl_surface() == &surface
                                || self
                                    .popups
                                    .iter()
                                    .any(|p| p.c_popup.wl_surface() == surface)
                        })
                    {
                        match (&acc, &f) {
                            (FocusStatus::LastFocused(t_acc), FocusStatus::LastFocused(t_cur)) => {
                                if t_cur > t_acc {
                                    *f
                                } else {
                                    acc
                                }
                            }
                            (FocusStatus::LastFocused(_), FocusStatus::Focused) => *f,
                            _ => acc,
                        }
                    } else {
                        acc
                    }
                },
            )
        };
        match self.visibility {
            Visibility::Hidden => {
                if let FocusStatus::Focused = cur_hover {
                    // start transition to visible
                    let margin = match self.config.anchor() {
                        PanelAnchor::Left | PanelAnchor::Right => -(self.dimensions.w),
                        PanelAnchor::Top | PanelAnchor::Bottom => -(self.dimensions.h),
                    } + self.config.get_hide_handle().unwrap() as i32;
                    self.is_dirty = true;
                    self.visibility = Visibility::TransitionToVisible {
                        last_instant: Instant::now(),
                        progress: Duration::new(0, 0),
                        prev_margin: margin,
                    }
                }
            }
            Visibility::Visible => {
                if let FocusStatus::LastFocused(t) = cur_hover {
                    // start transition to hidden
                    let duration_since_last_focus = match Instant::now().checked_duration_since(t) {
                        Some(d) => d,
                        None => return,
                    };
                    if duration_since_last_focus > self.config.get_hide_wait().unwrap() {
                        self.visibility = Visibility::TransitionToHidden {
                            last_instant: Instant::now(),
                            progress: Duration::new(0, 0),
                            prev_margin: 0,
                        }
                    }
                }
            }
            Visibility::TransitionToHidden {
                last_instant,
                progress,
                prev_margin,
            } => {
                let now = Instant::now();
                let total_t = self.config.get_hide_transition().unwrap();
                let delta_t = match now.checked_duration_since(last_instant) {
                    Some(d) => d,
                    None => return,
                };
                let prev_progress = progress;
                let progress = match prev_progress.checked_add(delta_t) {
                    Some(d) => d,
                    None => return,
                };
                let progress_norm =
                    smootherstep(progress.as_millis() as f32 / total_t.as_millis() as f32);
                let handle = self.config.get_hide_handle().unwrap() as i32;

                if let FocusStatus::Focused = cur_hover {
                    // start transition to visible
                    self.visibility = Visibility::TransitionToVisible {
                        last_instant: now,
                        progress: total_t.checked_sub(progress).unwrap_or_default(),
                        prev_margin,
                    }
                } else {
                    let panel_size = if self.config().is_horizontal() {
                        self.dimensions.h
                    } else {
                        self.dimensions.w
                    };
                    let target = -panel_size + handle;

                    let cur_pix = (progress_norm * target as f32) as i32;
                    let margin = self.config.get_margin() as i32;

                    if progress > total_t {
                        if self.config.exclusive_zone() {
                            layer_surface.set_exclusive_zone(panel_size);
                        }
                        Self::set_margin(self.config.anchor, margin, target, layer_surface);
                        layer_shell_wl_surface.commit();
                        self.is_dirty = true;
                        self.visibility = Visibility::Hidden;
                    } else {
                        if prev_margin != cur_pix {
                            if self.config.exclusive_zone() {
                                layer_surface.set_exclusive_zone(panel_size - cur_pix);
                            }
                            Self::set_margin(self.config.anchor, margin, cur_pix, layer_surface);

                            layer_shell_wl_surface.commit();
                        }
                        self.close_popups();
                        self.visibility = Visibility::TransitionToHidden {
                            last_instant: now,
                            progress,
                            prev_margin: cur_pix,
                        };
                    }
                }
            }
            Visibility::TransitionToVisible {
                last_instant,
                progress,
                prev_margin,
            } => {
                let now = Instant::now();
                let total_t = self.config.get_hide_transition().unwrap();
                let delta_t = match now.checked_duration_since(last_instant) {
                    Some(d) => d,
                    None => return,
                };
                let prev_progress = progress;
                let progress = match prev_progress.checked_add(delta_t) {
                    Some(d) => d,
                    None => return,
                };
                let progress_norm =
                    smootherstep(progress.as_millis() as f32 / total_t.as_millis() as f32);
                let handle = self.config.get_hide_handle().unwrap() as i32;

                if let FocusStatus::LastFocused(_) = cur_hover {
                    // start transition to visible
                    self.close_popups();
                    self.visibility = Visibility::TransitionToHidden {
                        last_instant: now,
                        progress: total_t.checked_sub(progress).unwrap_or_default(),
                        prev_margin,
                    }
                } else {
                    let panel_size = if self.config().is_horizontal() {
                        self.dimensions.h
                    } else {
                        self.dimensions.w
                    };
                    let start = -panel_size + handle;

                    let cur_pix = ((1.0 - progress_norm) * start as f32) as i32;

                    if progress > total_t {
                        if self.config.exclusive_zone() {
                            layer_surface.set_exclusive_zone(panel_size);
                        }
                        Self::set_margin(
                            self.config.anchor,
                            self.config.get_margin() as i32,
                            0,
                            layer_surface,
                        );
                        layer_shell_wl_surface.commit();
                        self.visibility = Visibility::Visible;
                    } else {
                        if prev_margin != cur_pix {
                            if self.config.exclusive_zone() {
                                layer_surface.set_exclusive_zone(panel_size - cur_pix);
                            }
                            let margin = self.config.get_margin() as i32;
                            Self::set_margin(self.config.anchor, margin, cur_pix, layer_surface);

                            layer_shell_wl_surface.commit();
                        }
                        self.visibility = Visibility::TransitionToVisible {
                            last_instant: now,
                            progress,
                            prev_margin: cur_pix,
                        };
                    }
                }
            }
        }
        return;
    }

    fn set_margin(anchor: PanelAnchor, margin: i32, target: i32, layer_surface: &LayerSurface) {
        match anchor {
            PanelAnchor::Left => layer_surface.set_margin(margin, 0, margin, target),
            PanelAnchor::Right => layer_surface.set_margin(margin, target, margin, 0),
            PanelAnchor::Top => layer_surface.set_margin(target, margin, 0, margin),
            PanelAnchor::Bottom => layer_surface.set_margin(0, margin, target, margin),
        };
    }

    pub(crate) fn constrain_dim(
        &self,
        size: Size<i32, Logical>,
        active_gap: Option<u32>,
    ) -> Size<i32, Logical> {
        let mut w: i32 = size.w;
        let mut h: i32 = size.h;

        let output_dims = self
            .output
            .as_ref()
            .and_then(|(_, _, info)| {
                info.modes
                    .iter()
                    .find_map(|m| if m.current { Some(m.dimensions) } else { None })
            })
            .map(|(w, h)| (w as u32, h as u32));

        let (constrained_w, constrained_h) =
            self.config
                .get_dimensions(output_dims, self.suggested_length, active_gap);
        if let Some(w_range) = constrained_w {
            w = w.clamp(w_range.start as i32, w_range.end as i32 - 1);
        }
        if let Some(h_range) = constrained_h {
            h = h.clamp(h_range.start as i32, h_range.end as i32 - 1);
        }

        (w as i32, h as i32).into()
    }

    fn apply_animation_state(&mut self) {
        if let Some(animation_state) = self.animate_state.as_mut() {
            self.damage_tracked_renderer = Some(OutputDamageTracker::new(
                self.dimensions
                    .to_f64()
                    .to_physical(self.scale)
                    .to_i32_round(),
                1.0,
                smithay::utils::Transform::Flipped180,
            ));
            self.panel_changed = true;
            let progress = (Instant::now()
                .duration_since(animation_state.started_at)
                .as_millis() as f32)
                / animation_state.duration.as_millis() as f32;
            self.is_dirty = true;
            if progress >= 1.0 {
                self.bg_color = animation_state.end.bg_color;
                self.animate_state = None;
                return;
            }

            animation_state.progress = progress;
            let progress = smootherstep(progress);
            let new_cur = AnimatableState {
                // TODO: blend in perceptual color space?
                bg_color: [
                    animation_state.start.bg_color[0]
                        + (animation_state.end.bg_color[0] - animation_state.start.bg_color[0])
                            * progress,
                    animation_state.start.bg_color[1]
                        + (animation_state.end.bg_color[1] - animation_state.start.bg_color[1])
                            * progress,
                    animation_state.start.bg_color[2]
                        + (animation_state.end.bg_color[2] - animation_state.start.bg_color[2])
                            * progress,
                    animation_state.start.bg_color[3]
                        + (animation_state.end.bg_color[3] - animation_state.start.bg_color[3])
                            * progress,
                ],
                border_radius: (animation_state.start.border_radius as f32
                    + ((animation_state.end.border_radius as f32
                        - animation_state.start.border_radius as f32)
                        * progress))
                    .round() as u32,
                expanded: animation_state.start.expanded
                    + ((animation_state.end.expanded - animation_state.start.expanded) as f32
                        * progress),
                gap: (animation_state.start.gap as f32
                    + ((animation_state.end.gap as f32 - animation_state.start.gap as f32)
                        * progress))
                    .round() as u16,
            };
            animation_state.cur = new_cur;
        }
    }

    pub(crate) fn handle_events<W: WrapperSpace>(
        &mut self,
        _dh: &DisplayHandle,
        popup_manager: &mut PopupManager,
        time: u32,
        mut renderer: Option<&mut GlesRenderer>,
        qh: &QueueHandle<GlobalState<W>>,
    ) -> Instant {
        self.space.refresh();
        self.apply_animation_state();

        popup_manager.cleanup();

        self.handle_focus();
        let mut should_render = false;
        match self.space_event.take() {
            Some(SpaceEvent::Quit) => {
                info!("root layer shell surface removed.");
            }
            Some(SpaceEvent::WaitConfigure {
                first,
                width,
                height,
            }) => {
                self.space_event.replace(Some(SpaceEvent::WaitConfigure {
                    first,
                    width,
                    height,
                }));
            }
            None => {
                if let (Some(size), Some(layer_surface)) =
                    (self.pending_dimensions.take(), self.layer.as_ref())
                {
                    let width: u32 = size.w.try_into().unwrap();
                    let height: u32 = size.h.try_into().unwrap();
                    if self.config.is_horizontal() {
                        layer_surface.set_size(0, height);
                    } else {
                        layer_surface.set_size(width, 0);
                    }
                    let list_thickness = match self.config.anchor() {
                        PanelAnchor::Left | PanelAnchor::Right => width,
                        PanelAnchor::Top | PanelAnchor::Bottom => height,
                    };

                    if self.config.autohide.is_none() && self.config.exclusive_zone() {
                        self.layer
                            .as_ref()
                            .unwrap()
                            .set_exclusive_zone(list_thickness as i32);
                        if self.config.get_effective_anchor_gap() > 0 {
                            Self::set_margin(
                                self.config.anchor,
                                self.config.get_effective_anchor_gap() as i32,
                                0,
                                layer_surface,
                            );
                        }
                    } else if self.config.autohide.is_some()
                        && matches!(self.visibility, Visibility::Hidden)
                    {
                        if self.config.exclusive_zone() {
                            layer_surface.set_exclusive_zone(list_thickness as i32);
                        }
                        Self::set_margin(
                            self.config.anchor,
                            self.config.get_margin() as i32,
                            -(list_thickness as i32)
                                + self.config.get_hide_handle().unwrap_or_default() as i32,
                            layer_surface,
                        );
                    }
                    layer_surface.wl_surface().commit();
                    self.space_event.replace(Some(SpaceEvent::WaitConfigure {
                        first: false,
                        width: size.w,
                        height: size.h,
                    }));
                } else if self.layer.is_some() {
                    should_render = if self.is_dirty {
                        let update_res = self.layout();

                        update_res.is_ok()
                    } else {
                        true
                    };
                }
            }
        }

        if let Some(renderer) = renderer.as_mut() {
            let prev = self.popups.len();
            self.popups
                .retain_mut(|p: &mut WrapperPopup| p.handle_events(popup_manager));

            if prev == self.popups.len() && should_render {
                if let Err(e) = self.render(renderer, time, qh) {
                    error!("Failed to render, error: {:?}", e);
                }
            }
        }

        self.last_dirty.unwrap_or_else(Instant::now)
    }

    pub fn configure_panel_layer(
        &mut self,
        _layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        renderer: &mut Option<GlesRenderer>,
    ) {
        let (w, h) = configure.new_size;
        match self.space_event.take() {
            Some(e) => match e {
                SpaceEvent::WaitConfigure {
                    first,
                    mut width,
                    mut height,
                } => {
                    if w != 0 {
                        width = w as i32;
                        if self.config.is_horizontal() {
                            self.suggested_length.replace(w);
                        }
                    }
                    if h != 0 {
                        height = h as i32;
                        if !self.config.is_horizontal() {
                            self.suggested_length.replace(h);
                        }
                    }

                    if width <= 0 {
                        width = 1;
                    }
                    if height <= 0 {
                        height = 1;
                    }
                    let dim = self.constrain_dim(
                        (width as i32, height as i32).into(),
                        Some(self.gap() as u32),
                    );

                    if first {
                        let client_egl_surface = unsafe {
                            ClientEglSurface::new(
                                WlEglSurface::new(
                                    self.layer.as_ref().unwrap().wl_surface().id(),
                                    dim.w,
                                    dim.h,
                                )
                                .unwrap(), // TODO remove unwrap
                                self.layer.as_ref().unwrap().wl_surface().clone(),
                            )
                        };
                        let new_egl_display = if let Some(renderer) = renderer.as_ref() {
                            renderer.egl_context().display().clone()
                        } else {
                            let client_egl_display = ClientEglDisplay {
                                display: self.c_display.as_ref().unwrap().clone(),
                            };
                            EGLDisplay::new(client_egl_display)
                                .expect("Failed to create EGL display")
                        };

                        let egl_context = EGLContext::new_with_config(
                            &new_egl_display,
                            GlAttributes {
                                version: (3, 0),
                                profile: None,
                                debug: cfg!(debug_assertions),
                                vsync: false,
                            },
                            PixelFormatRequirements::_8_bit(),
                        )
                        .unwrap_or_else(|_| {
                            EGLContext::new_with_config(
                                &new_egl_display,
                                GlAttributes {
                                    version: (2, 0),
                                    profile: None,
                                    debug: cfg!(debug_assertions),
                                    vsync: false,
                                },
                                PixelFormatRequirements::_8_bit(),
                            )
                            .expect("Failed to create EGL context")
                        });

                        let mut new_renderer = if let Some(renderer) = renderer.take() {
                            renderer
                        } else {
                            unsafe {
                                GlesRenderer::new(egl_context)
                                    .expect("Failed to create EGL Surface")
                            }
                        };

                        init_shaders(&mut new_renderer).expect("Failed to init shaders...");

                        let egl_surface = Rc::new(unsafe {
                            EGLSurface::new(
                                &new_egl_display,
                                new_renderer
                                    .egl_context()
                                    .pixel_format()
                                    .expect("Failed to get pixel format from EGL context "),
                                new_renderer.egl_context().config_id(),
                                client_egl_surface,
                            )
                            .expect("Failed to create EGL Surface")
                        });

                        // bind before setting swap interval
                        let _ = new_renderer.unbind();
                        let _ = new_renderer.bind(egl_surface.clone());
                        let swap_success =
                            unsafe { SwapInterval(new_egl_display.get_display_handle().handle, 0) }
                                == 1;
                        if !swap_success {
                            error!("Failed to set swap interval");
                        }
                        let _ = new_renderer.unbind();

                        renderer.replace(new_renderer);
                        self.egl_surface.replace(egl_surface);
                    }
                    if let (Some(renderer), Some(egl_surface)) =
                        (renderer.as_mut(), self.egl_surface.as_ref())
                    {
                        let _ = renderer.unbind();
                        let scaled_size = dim.to_f64().to_physical(self.scale).to_i32_round();
                        let _ = renderer.bind(egl_surface.clone());
                        egl_surface.resize(scaled_size.w, scaled_size.h, 0, 0);
                        let _ = renderer.unbind();
                        if let Some(viewport) = self.layer_viewport.as_ref() {
                            viewport.set_destination(dim.w.max(1), dim.h.max(1));
                        }
                    }

                    self.dimensions = (dim.w, dim.h).into();
                    self.damage_tracked_renderer = Some(OutputDamageTracker::new(
                        dim.to_f64().to_physical(self.scale).to_i32_round(),
                        1.0,
                        smithay::utils::Transform::Flipped180,
                    ));
                }
                SpaceEvent::Quit => (),
            },
            None => {
                let mut width = self.dimensions.w;
                let mut height = self.dimensions.h;
                if w != 0 {
                    width = w as i32;
                    if self.config.is_horizontal() {
                        self.suggested_length.replace(w);
                    }
                }
                if h != 0 {
                    height = h as i32;
                    if !self.config.is_horizontal() {
                        self.suggested_length.replace(h);
                    }
                }
                if width == 0 {
                    width = 1;
                }
                if height == 0 {
                    height = 1;
                }
                let dim = self.constrain_dim(
                    (width as i32, height as i32).into(),
                    Some(self.gap() as u32),
                );

                if let (Some(renderer), Some(egl_surface)) =
                    (renderer.as_mut(), self.egl_surface.as_ref())
                {
                    let _ = renderer.unbind();
                    let _ = renderer.bind(egl_surface.clone());
                    let scaled_size = dim.to_f64().to_physical(self.scale).to_i32_round();
                    egl_surface.resize(scaled_size.w, scaled_size.h, 0, 0);
                    let _ = renderer.unbind();
                    if let Some(viewport) = self.layer_viewport.as_ref() {
                        viewport.set_destination(dim.w, dim.h);
                    }
                }
                self.dimensions = (dim.w, dim.h).into();
                self.damage_tracked_renderer = Some(OutputDamageTracker::new(
                    dim.to_f64().to_physical(self.scale).to_i32_round(),
                    1.0,
                    smithay::utils::Transform::Flipped180,
                ));
            }
        }
    }

    pub fn set_theme_window_color(&mut self, mut color: [f32; 4]) {
        if let CosmicPanelBackground::ThemeDefault = self.config.background {
            color[3] = self.config.opacity;
        }
        if let Some(animate_state) = self.animate_state.as_mut() {
            animate_state.end.bg_color = color;
        } else {
            let start = AnimatableState {
                bg_color: self.bg_color,
                border_radius: self.config.border_radius,
                expanded: if self.config.expand_to_edges {
                    1.0
                } else {
                    0.0
                },
                gap: self.config.get_effective_anchor_gap() as u16,
            };
            let cur = start.clone();
            let mut end = start.clone();
            end.bg_color = color;
            self.animate_state = Some(AnimateState {
                start,
                end,
                cur,
                started_at: Instant::now(),
                progress: 0.0,
                duration: Duration::from_millis(300),
            })
        }
        self.bg_color = color;
    }

    /// clear the panel
    pub fn clear(&mut self) {
        self.is_dirty = true;
        self.popups.clear();
        self.damage_tracked_renderer = Some(OutputDamageTracker::new(
            self.dimensions
                .to_f64()
                .to_physical(self.scale)
                .to_i32_round(),
            1.0,
            smithay::utils::Transform::Flipped180,
        ));
    }

    pub fn apply_positioner_state(
        &self,
        positioner: &XdgPositioner,
        pos_state: PositionerState,
        s_surface: &PopupSurface,
    ) {
        let PositionerState {
            rect_size,
            anchor_rect,
            anchor_edges,
            gravity,
            constraint_adjustment,
            offset,
            reactive,
            parent_size,
            parent_configure: _,
        } = pos_state;
        let parent_window = if let Some(s) = self
            .space
            .elements()
            .find(|w| w.wl_surface() == s_surface.get_parent_surface().as_ref().cloned())
        {
            s
        } else {
            return;
        };

        let p_offset = self
            .space
            .element_location(parent_window)
            .unwrap_or_else(|| (0, 0).into());

        positioner.set_size(rect_size.w.max(1), rect_size.h.max(1));
        positioner.set_anchor_rect(
            anchor_rect.loc.x + p_offset.x,
            anchor_rect.loc.y + p_offset.y,
            anchor_rect.size.w,
            anchor_rect.size.h,
        );
        positioner.set_anchor(Anchor::try_from(anchor_edges as u32).unwrap_or(Anchor::None));
        positioner.set_gravity(Gravity::try_from(gravity as u32).unwrap_or(Gravity::None));

        positioner.set_constraint_adjustment(u32::from(constraint_adjustment));
        positioner.set_offset(offset.x, offset.y);
        if positioner.version() >= 3 {
            if reactive {
                positioner.set_reactive();
            }
            if let Some(parent_size) = parent_size {
                positioner.set_parent_size(parent_size.w, parent_size.h);
            }
        }
    }

    pub fn update_config(&mut self, config: CosmicPanelConfig, bg_color: [f32; 4]) {
        // avoid animating if currently maximized
        if self.maximized {
            return;
        }

        let mut needs_commit = false;
        if config.exclusive_zone != self.config.exclusive_zone {
            if let Some(l) = self.layer.as_ref() {
                let list_thickness = if config.exclusive_zone {
                    match self.config.anchor() {
                        PanelAnchor::Left | PanelAnchor::Right => self.dimensions.w,
                        PanelAnchor::Top | PanelAnchor::Bottom => self.dimensions.h,
                    }
                } else {
                    -1
                };

                l.set_exclusive_zone(list_thickness as i32);
                needs_commit = true;
            }
        }

        if config.autohide.is_none() && self.config.autohide.is_some() {
            if let Some(l) = self.layer.as_ref() {
                let margin = config.get_effective_anchor_gap() as i32;
                Self::set_margin(config.anchor, margin, 0, l);
                let list_thickness = match self.config.anchor() {
                    PanelAnchor::Left | PanelAnchor::Right => self.dimensions.w,
                    PanelAnchor::Top | PanelAnchor::Bottom => self.dimensions.h,
                };
                l.set_exclusive_zone(list_thickness as i32);
                let (width, height) = if self.config.is_horizontal() {
                    (0, self.dimensions.h)
                } else {
                    (self.dimensions.w, 0)
                };
                l.set_size(width as u32, height as u32);
                needs_commit = true;
            }
        } else {
            if self.config.get_effective_anchor_gap() != config.get_effective_anchor_gap() {
                if let Some(l) = self.layer.as_ref() {
                    let margin = config.get_effective_anchor_gap() as i32;
                    Self::set_margin(config.anchor, margin, 0, l);
                    needs_commit = true;
                }
            }
        }

        // can't animate anchor changes
        // return early
        if config.anchor() != self.config.anchor() {
            if config.is_horizontal() != self.config.is_horizontal() {
                panic!("Can't apply anchor changes when orientation changes. Requires re-creation of the panel.");
            }
            if let Some(l) = self.layer.as_ref() {
                l.set_anchor(config.anchor().into());
                let (width, height) = if config.is_horizontal() {
                    (0, self.dimensions.h)
                } else {
                    (self.dimensions.w, 0)
                };
                l.set_size(width as u32, height as u32);
                l.commit();
            }
            self.config = config;
            self.clear();
            return;
        }

        if config.anchor_gap != self.config.anchor_gap {
            if self.config.is_horizontal() {
                if let Some(l) = self.suggested_length {
                    self.dimensions.w = l as i32;
                }
            } else {
                if let Some(l) = self.suggested_length {
                    self.dimensions.h = l as i32;
                }
            }
        }

        if needs_commit {
            if let Some(l) = self.layer.as_ref() {
                l.commit();
            }
        }

        let start = AnimatableState {
            bg_color: self.bg_color,
            border_radius: self.config.border_radius,
            expanded: if self.config.expand_to_edges {
                1.0
            } else {
                0.0
            },
            gap: self.config.get_effective_anchor_gap() as u16,
        };

        let end = AnimatableState {
            bg_color,
            border_radius: config.border_radius,
            expanded: if config.expand_to_edges { 1.0 } else { 0.0 },
            gap: config.get_effective_anchor_gap() as u16,
        };
        if let Some(animated_state) = self.animate_state.as_mut() {
            animated_state.start = animated_state.cur.clone();
            animated_state.end = end;
            animated_state.started_at = Instant::now();
            animated_state.progress = 0.0;
        } else {
            self.animate_state = Some(AnimateState {
                cur: start.clone(),
                start,
                end,
                progress: 0.0,
                started_at: Instant::now(),
                duration: Duration::from_millis(300), // TODO make configurable
            });
        }

        self.config = config;

        self.clear();
    }

    pub fn set_maximized(
        &mut self,
        maximized: bool,
        config: CosmicPanelConfig,
        bg_color: [f32; 4],
    ) {
        if self.maximized == maximized {
            return;
        }
        if !self.maximized {
            self.update_config(config, bg_color);
            self.maximized = maximized;
        } else {
            // TODO restore old config
            self.maximized = maximized;
            self.update_config(config, bg_color);
        }
    }
}

impl Drop for PanelSpace {
    fn drop(&mut self) {
        // request processes to stop
        let _ = self.applet_tx.try_send(AppletMsg::Cleanup(self.id()));
    }
}
