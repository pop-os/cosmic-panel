use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    fmt::Debug,
    os::{fd::OwnedFd, unix::net::UnixStream},
    rc::Rc,
    str::FromStr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use crate::{
    iced::elements::{background::BackgroundElement, PopupMappedInternal},
    xdg_shell_wrapper::{
        client::handlers::overlap::OverlapNotifyV1,
        client_state::{ClientFocus, FocusStatus},
        server_state::{ServerFocus, ServerPtrFocus},
        shared_state::GlobalState,
        space::{
            ClientEglDisplay, ClientEglSurface, PanelPopup, PanelSubsurface, SpaceEvent,
            Visibility, WrapperPopup, WrapperSpace, WrapperSubsurface,
        },
        util::smootherstep,
        wp_fractional_scaling::FractionalScalingManager,
        wp_security_context::SecurityContextManager,
        wp_viewporter::ViewporterState,
    },
};
use cctk::{
    cosmic_protocols::overlap_notify::v1::client::zcosmic_overlap_notification_v1::ZcosmicOverlapNotificationV1,
    wayland_client::{protocol::wl_subcompositor::WlSubcompositor, Connection},
};

use cosmic::iced::id;
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
    subcompositor::{SubcompositorState, SubsurfaceData},
};
use smithay::{
    backend::{
        egl::{
            context::{GlAttributes, PixelFormatRequirements},
            display::EGLDisplay,
            ffi::egl::SwapInterval,
            surface::EGLSurface,
            EGLContext,
        },
        renderer::{damage::OutputDamageTracker, gles::GlesRenderer, Bind},
    },
    desktop::{space::SpaceElement, utils::bbox_from_surface_tree, PopupManager, Space},
    output::Output,
    reexports::{
        wayland_protocols::xdg::shell::client::xdg_positioner::{Anchor, Gravity},
        wayland_server::{backend::ClientId, protocol::wl_seat, Client, DisplayHandle, Resource},
    },
    utils::{Logical, Rectangle, Serial, Size},
    wayland::{
        compositor::{with_states, SurfaceAttributes},
        fractional_scale::with_fractional_scale,
        seat::WaylandFocus,
        shell::xdg::{PopupSurface, PositionerState},
    },
};
use tokio::sync::{mpsc, oneshot};
use tracing::{error, info};
use wayland_egl::WlEglSurface;
use wayland_protocols::{
    wp::{
        fractional_scale::v1::client::wp_fractional_scale_v1::WpFractionalScaleV1,
        security_context::v1::client::wp_security_context_v1::WpSecurityContextV1,
        viewporter::client::wp_viewport::WpViewport,
    },
    xdg::shell::client::xdg_positioner::ConstraintAdjustment,
};

use cosmic_panel_config::{CosmicPanelBackground, CosmicPanelConfig, PanelAnchor};

use crate::{iced::elements::CosmicMappedInternal, PanelCalloopMsg};

use super::{
    corner_element::{init_shaders, RoundedRectangleSettings},
    layout::OverflowSection,
};

pub enum AppletMsg {
    NewProcess(String, Process),
    NewNotificationsProcess(String, Process, Vec<(String, String)>, Vec<OwnedFd>),
    NeedNewNotificationFd(oneshot::Sender<OwnedFd>),
    ClientSocketPair(ClientId),
    Cleanup(String),
}

impl Debug for AppletMsg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NewProcess(arg0, _) => f.debug_tuple("NewProcess").field(arg0).finish(),
            Self::NewNotificationsProcess(arg0, _, arg2, arg3) => f
                .debug_tuple("NewNotificationsProcess")
                .field(arg0)
                .field(arg2)
                .field(arg3)
                .finish(),
            Self::NeedNewNotificationFd(arg0) => {
                f.debug_tuple("NeedNewNotificationFd").field(arg0).finish()
            },
            Self::ClientSocketPair(arg0) => f.debug_tuple("ClientSocketPair").field(arg0).finish(),
            Self::Cleanup(arg0) => f.debug_tuple("Cleanup").field(arg0).finish(),
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
    pub shrink_priority: Option<u32>,
    pub shrink_min_size: Option<ClientShrinkSize>,
    /// If there is an existing popup, this applet with be pressed when hovered.
    pub auto_popup_hover_press: Option<AppletAutoClickAnchor>,
}

#[derive(Debug, Clone, Copy)]
pub enum ClientShrinkSize {
    AppletUnit(u32),
    Pixel(u32),
}

impl ClientShrinkSize {
    pub fn to_pixels(&self, applet_size: u32) -> u32 {
        match self {
            // TODO get spacing / padding from the theme or panel config?
            Self::AppletUnit(units) => 4 + applet_size * units + 4 * units.saturating_sub(1),
            Self::Pixel(pixels) => *pixels,
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub enum AppletAutoClickAnchor {
    #[default]
    Auto,
    Left,
    Right,
    Top,
    Bottom,
    Center,
    Start,
    End,
}

impl FromStr for AppletAutoClickAnchor {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "left" => Ok(Self::Left),
            "right" => Ok(Self::Right),
            "top" => Ok(Self::Top),
            "bottom" => Ok(Self::Bottom),
            "center" => Ok(Self::Center),
            "start" => Ok(Self::Start),
            "end" => Ok(Self::End),
            _ => Err(()),
        }
    }
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
            auto_popup_hover_press: None,
            shrink_priority: None,
            shrink_min_size: None,
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

#[derive(Debug, Clone)]
pub struct PanelColors {
    pub theme: cosmic::Theme,
    pub color_override: Option<[f32; 4]>,
}

impl PanelColors {
    pub fn new(theme: cosmic::Theme) -> Self {
        Self { theme, color_override: None }
    }

    pub fn with_color_override(mut self, color_override: Option<[f32; 4]>) -> Self {
        self.color_override = color_override;
        self
    }

    pub fn bg_color(&self, alpha: f32) -> [f32; 4] {
        self.color_override.unwrap_or_else(|| {
            let c = self.theme.cosmic().bg_color();
            [c.red, c.green, c.blue, alpha]
        })
    }
}

#[derive(Default, Debug, Clone, PartialEq, Eq, Hash)]
pub struct HoverTrack {
    pub hover_id: Option<HoverId>,
    pub generation: u32,
}
impl HoverTrack {
    pub fn set_hover_id(&mut self, hover_id: Option<HoverId>) {
        self.hover_id = hover_id;
        self.generation += 1;
    }
}

// first check if the motion is on a popup's client surface
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HoverId {
    Client(ClientId),
    Overflow(id::Id),
}

// space for the cosmic panel
#[derive(Debug)]
pub struct PanelSpace {
    // XXX implicitly drops egl_surface first to avoid segfault
    pub egl_surface: Option<EGLSurface>,
    pub c_display: Option<WlDisplay>,
    pub config: CosmicPanelConfig,
    pub space: Space<CosmicMappedInternal>,
    pub damage_tracked_renderer: Option<OutputDamageTracker>,
    pub clients_left: Clients,
    pub clients_center: Clients,
    pub clients_right: Clients,
    pub overflow_left: Space<PopupMappedInternal>,
    pub overflow_center: Space<PopupMappedInternal>,
    pub overflow_right: Space<PopupMappedInternal>,
    pub last_dirty: Option<Instant>,
    // pending size of the panel
    pub pending_dimensions: Option<Size<i32, Logical>>,
    // suggested length of the panel
    pub suggested_length: Option<u32>,
    // size of the panel
    pub actual_size: Size<i32, Logical>,
    /// dimensions of the layer surface
    /// this will be the same as the output size on the major axis of the panel
    pub dimensions: Size<i32, Logical>,
    // Logical size of the panel, with the applied animation state
    pub container_length: i32,
    pub is_dirty: bool,
    pub space_event: Rc<Cell<Option<SpaceEvent>>>,
    pub c_focused_surface: Rc<RefCell<ClientFocus>>,
    pub c_hovered_surface: Rc<RefCell<ClientFocus>>,
    pub s_focused_surface: ServerFocus,
    pub s_hovered_surface: ServerPtrFocus,
    pub visibility: Visibility,
    pub output: Option<(c_wl_output::WlOutput, Output, OutputInfo)>,
    pub s_display: Option<DisplayHandle>,
    pub layer: Option<LayerSurface>,
    pub layer_fractional_scale: Option<WpFractionalScaleV1>,
    pub layer_viewport: Option<WpViewport>,
    pub popups: Vec<WrapperPopup>,
    pub subsurfaces: Vec<WrapperSubsurface>,
    pub start_instant: Instant,
    pub colors: PanelColors,
    pub applet_tx: mpsc::Sender<AppletMsg>,
    pub input_region: Option<Region>,
    pub has_frame: bool,
    pub scale: f64,
    pub output_has_toplevel: bool,
    pub security_context_manager: Option<SecurityContextManager>,
    pub animate_state: Option<AnimateState>,
    pub maximized: bool,
    pub panel_tx: calloop::channel::Sender<PanelCalloopMsg>,
    pub minimize_applet_rect: Rectangle<i32, Logical>,
    pub panel_rect_settings: RoundedRectangleSettings,
    pub scale_change_retries: u32,
    /// Extra gap for stacked panels. Logical coordinate space.
    pub additional_gap: i32,
    /// Target gap for the panel on its anchored edge. Logical coordinate space.
    pub anchor_gap: i32,
    pub loop_handle: calloop::LoopHandle<'static, GlobalState>,
    pub left_overflow_button_id: id::Id,
    pub center_overflow_button_id: id::Id,
    pub right_overflow_button_id: id::Id,
    pub left_overflow_popup_id: id::Id,
    pub center_overflow_popup_id: id::Id,
    pub right_overflow_popup_id: id::Id,
    pub overflow_popup: Option<(PanelPopup, OverflowSection)>,
    pub remap_attempts: u32,
    pub background_element: Option<BackgroundElement>,
    pub last_minimize_update: Instant,
    pub(crate) toplevel_overlaps: HashSet<wayland_backend::client::ObjectId>,
    pub(crate) notification_subscription: Option<ZcosmicOverlapNotificationV1>,
    pub(crate) overlap_notify: Option<OverlapNotifyV1>,
    pub(crate) hover_track: HoverTrack,
}

impl PanelSpace {
    /// create a new space for the cosmic panel
    pub fn new(
        config: CosmicPanelConfig,
        c_focused_surface: Rc<RefCell<ClientFocus>>,
        c_hovered_surface: Rc<RefCell<ClientFocus>>,
        applet_tx: mpsc::Sender<AppletMsg>,
        theme: cosmic::Theme,
        s_display: DisplayHandle,
        security_context_manager: Option<SecurityContextManager>,
        conn: &Connection,
        panel_tx: calloop::channel::Sender<PanelCalloopMsg>,
        visibility: Visibility,
        loop_handle: calloop::LoopHandle<'static, GlobalState>,
    ) -> Self {
        let name = format!("{}-{}", config.name, config.output);
        Self {
            config,
            space: Space::default(),
            overflow_left: Space::default(),
            overflow_center: Space::default(),
            overflow_right: Space::default(),
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
            subsurfaces: Default::default(),
            visibility,
            start_instant: Instant::now(),
            c_focused_surface,
            c_hovered_surface,
            s_focused_surface: Default::default(),
            s_hovered_surface: Default::default(),
            colors: PanelColors::new(theme),
            applet_tx,
            actual_size: (0, 0).into(),
            input_region: None,
            damage_tracked_renderer: None,
            is_dirty: false,
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
            scale_change_retries: 0,
            additional_gap: 0,
            loop_handle,
            left_overflow_button_id: id::Id::new(format!("{}-left-overflow-button", name)),
            center_overflow_button_id: id::Id::new(format!("{}-center-overflow-button", name)),
            right_overflow_button_id: id::Id::new(format!("{}-right-overflow-button", name)),
            left_overflow_popup_id: id::Id::new(format!("{}-left-overflow-popup", name)),
            center_overflow_popup_id: id::Id::new(format!("{}-center-overflow-popup", name)),
            right_overflow_popup_id: id::Id::new(format!("{}-right-overflow-popup", name)),
            overflow_popup: None,
            remap_attempts: 0,
            background_element: None,
            last_minimize_update: Instant::now() - Duration::from_secs(1),
            anchor_gap: 0,
            toplevel_overlaps: HashSet::new(),
            notification_subscription: None,
            overlap_notify: None,
            hover_track: HoverTrack::default(),
        }
    }

    pub fn crosswise(&self) -> i32 {
        if self.config.is_horizontal() {
            self.dimensions.h
        } else {
            self.dimensions.w
        }
    }

    pub fn bg_color(&self) -> [f32; 4] {
        if let Some(animatable_state) = self.animate_state.as_ref() {
            animatable_state.cur.bg_color
        } else {
            self.colors.bg_color(self.config.opacity)
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

    pub fn id(&self) -> String {
        let id = format!(
            "panel-{}-{}-{}",
            self.config.name,
            self.config.output,
            self.output.as_ref().map(|o| o.2.name.clone().unwrap_or_default()).unwrap_or_default()
        );
        id
    }

    pub fn handle_focus(&mut self) {
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
            let no_hover_focus =
                c_focused_surface.iter().all(|f| matches!(f.2, FocusStatus::LastFocused(_)))
                    && c_hovered_surface.iter().all(|f| matches!(f.2, FocusStatus::LastFocused(_)));
            if self.config.autohide().is_none() {
                if no_hover_focus && self.animate_state.is_none() {
                    self.visibility = Visibility::Hidden;
                } else {
                    self.visibility = Visibility::Visible;
                }
                return;
            };

            let f = c_hovered_surface.iter().fold(
                if self.animate_state.is_some() || !self.output_has_toplevel {
                    FocusStatus::Focused
                } else {
                    FocusStatus::LastFocused(self.start_instant)
                },
                |acc, (surface, _, f)| {
                    if surface.is_alive()
                        && (self.layer.as_ref().is_some_and(|s| *s.wl_surface() == *surface)
                            || self.popups.iter().any(|p| {
                                &p.popup.c_popup.wl_surface() == &surface
                                    || self
                                        .popups
                                        .iter()
                                        .any(|p| p.popup.c_popup.wl_surface() == surface)
                            }))
                    {
                        match (&acc, &f) {
                            (FocusStatus::LastFocused(t_acc), FocusStatus::LastFocused(t_cur)) => {
                                if t_cur > t_acc {
                                    *f
                                } else {
                                    acc
                                }
                            },
                            (FocusStatus::LastFocused(_), FocusStatus::Focused) => *f,
                            _ => acc,
                        }
                    } else {
                        acc
                    }
                },
            );
            f
        };

        let intellihide = self.overlap_notify.is_some();
        match self.visibility {
            Visibility::Hidden => {
                if matches!(cur_hover, FocusStatus::Focused)
                    || (intellihide && self.toplevel_overlaps.is_empty())
                {
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
                    };
                    Self::set_margin(
                        self.config.anchor,
                        self.config.get_margin() as i32,
                        self.additional_gap,
                        layer_surface,
                    );
                }
            },
            Visibility::Visible => {
                if let FocusStatus::LastFocused(t) = cur_hover {
                    // start transition to hidden
                    let duration_since_last_focus = match Instant::now().checked_duration_since(t) {
                        Some(d) => d,
                        None => return,
                    };
                    if duration_since_last_focus > self.config.get_hide_wait().unwrap()
                        && (!intellihide || !self.toplevel_overlaps.is_empty())
                    {
                        self.is_dirty = true;
                        self.visibility = Visibility::TransitionToHidden {
                            last_instant: Instant::now(),
                            progress: Duration::new(0, 0),
                            prev_margin: 0,
                        }
                    }
                }
            },
            Visibility::TransitionToHidden { last_instant, progress, prev_margin } => {
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
                self.is_dirty = true;

                if matches!(cur_hover, FocusStatus::Focused)
                    || (intellihide && self.toplevel_overlaps.is_empty())
                {
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

                    if progress > total_t {
                        if self.config.exclusive_zone() {
                            layer_surface.set_exclusive_zone(panel_size);
                        }

                        self.anchor_gap = target;
                        self.visibility = Visibility::Hidden;
                    } else {
                        if prev_margin != cur_pix {
                            if self.config.exclusive_zone() {
                                layer_surface.set_exclusive_zone(panel_size - cur_pix);
                            }

                            self.anchor_gap = cur_pix;
                        }
                        self.close_popups(|_| false);
                        self.visibility = Visibility::TransitionToHidden {
                            last_instant: now,
                            progress,
                            prev_margin: cur_pix,
                        };
                    }
                }
            },
            Visibility::TransitionToVisible { last_instant, progress, prev_margin } => {
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
                self.is_dirty = true;

                if matches!(cur_hover, FocusStatus::LastFocused(_))
                    && (!intellihide || !self.toplevel_overlaps.is_empty())
                {
                    // start transition to hide
                    self.close_popups(|_| false);
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

                        self.anchor_gap = 0;
                        self.visibility = Visibility::Visible;
                        Self::set_margin(
                            self.config.anchor,
                            self.config.get_margin() as i32,
                            self.additional_gap,
                            layer_surface,
                        );
                    } else {
                        if prev_margin != cur_pix {
                            if self.config.exclusive_zone() {
                                layer_surface.set_exclusive_zone(panel_size - cur_pix);
                            }

                            self.anchor_gap = cur_pix;
                        }
                        self.visibility = Visibility::TransitionToVisible {
                            last_instant: now,
                            progress,
                            prev_margin: cur_pix,
                        };
                    }
                }
            },
        }
    }

    fn set_margin(
        anchor: PanelAnchor,
        margin: i32,
        additional_gap: i32,
        layer_surface: &LayerSurface,
    ) {
        match anchor {
            PanelAnchor::Left => layer_surface.set_margin(margin, 0, margin, additional_gap),
            PanelAnchor::Right => layer_surface.set_margin(margin, additional_gap, margin, 0),
            PanelAnchor::Top => layer_surface.set_margin(additional_gap, margin, 0, margin),
            PanelAnchor::Bottom => layer_surface.set_margin(0, margin, additional_gap, margin),
        };
    }

    pub fn constrain_dim(
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
                info.modes.iter().find_map(|m| if m.current { Some(m.dimensions) } else { None })
            })
            .map(|(w, h)| (w as u32, h as u32));

        let (constrained_w, constrained_h) =
            self.config.get_dimensions(output_dims, self.suggested_length, active_gap);
        if let Some(w_range) = constrained_w {
            w = w.clamp(w_range.start as i32, w_range.end as i32 - 1);
        }
        if let Some(h_range) = constrained_h {
            h = h.clamp(h_range.start as i32, h_range.end as i32 - 1);
        }

        (w, h).into()
    }

    fn apply_animation_state(&mut self) {
        if let Some(animation_state) = self.animate_state.as_mut() {
            self.damage_tracked_renderer = Some(OutputDamageTracker::new(
                self.dimensions.to_f64().to_physical(self.scale).to_i32_round(),
                self.scale,
                smithay::utils::Transform::Flipped180,
            ));
            let progress = (Instant::now().duration_since(animation_state.started_at).as_millis()
                as f32)
                / animation_state.duration.as_millis() as f32;
            self.is_dirty = true;
            if progress >= 1.0 {
                tracing::info!("Animation finished, setting bg_color to end value");
                if let CosmicPanelBackground::Color(c) = self.config.background {
                    self.colors = PanelColors::new(self.colors.theme.clone())
                        .with_color_override(Some([c[0], c[1], c[2], self.config.opacity]));
                } else {
                    self.colors.color_override = None;
                }
                self.animate_state = None;
                self.relax_all();
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
                    + ((animation_state.end.expanded - animation_state.start.expanded) * progress),
                gap: (animation_state.start.gap as f32
                    + ((animation_state.end.gap as f32 - animation_state.start.gap as f32)
                        * progress))
                    .round() as u16,
            };
            animation_state.cur = new_cur;
        }
    }

    pub fn set_additional_gap(&mut self, gap: i32) {
        if self.additional_gap == gap {
            return;
        }
        self.is_dirty = true;
        self.additional_gap = gap;
        if (!self.output_has_toplevel || matches!(self.visibility, Visibility::Visible))
            && !matches!(
                self.space_event.as_ref().get(),
                Some(SpaceEvent::WaitConfigure { first, .. }) if first
            )
        {
            if let Some(layer) = self.layer.as_ref() {
                Self::set_margin(
                    self.config.anchor,
                    self.config.get_margin() as i32,
                    self.additional_gap,
                    layer,
                );
                self.anchor_gap = 0;
            }
        }
    }

    pub fn handle_events(
        &mut self,
        _dh: &DisplayHandle,
        popup_manager: &mut PopupManager,
        time: u32,
        throttle: Option<Duration>,
        mut renderer: Option<&mut GlesRenderer>,
        qh: &QueueHandle<GlobalState>,
    ) -> Instant {
        self.space.refresh();
        self.apply_animation_state();

        self.handle_focus();
        let mut should_render = false;

        match self.space_event.take() {
            Some(SpaceEvent::Quit) => {
                info!("root layer shell surface removed.");
            },
            Some(SpaceEvent::WaitConfigure { first, width, height }) if first => {
                tracing::info!("Waiting for configure event");
                self.space_event.replace(Some(SpaceEvent::WaitConfigure { first, width, height }));
            },
            _ => {
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
                        self.layer.as_ref().unwrap().set_exclusive_zone(list_thickness as i32);
                        if self.config.get_effective_anchor_gap() > 0 {
                            Self::set_margin(
                                self.config.anchor,
                                self.config.get_effective_anchor_gap() as i32,
                                self.additional_gap,
                                layer_surface,
                            );
                            self.anchor_gap = 0;
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
                            self.additional_gap,
                            layer_surface,
                        );
                        self.anchor_gap = -(list_thickness as i32)
                            + self.config.get_hide_handle().unwrap_or_default() as i32;
                    }
                    layer_surface.wl_surface().commit();
                    layer_surface.wl_surface().frame(qh, layer_surface.wl_surface().clone());

                    info!("{:?}", self.space_event);
                } else if self.layer.is_some() {
                    should_render = true;
                    if self.is_dirty {
                        _ = self.layout_();
                    }
                }
            },
        }

        if let Some(renderer) = renderer.as_mut() {
            let prev = self.popups.len();
            self.popups.retain_mut(|p: &mut WrapperPopup| {
                let ret = p.handle_events(popup_manager, renderer);
                if !ret {
                    if let Some(w) = p.popup.fractional_scale.as_ref() {
                        w.destroy();
                    }
                    if let Some(w) = p.popup.viewport.as_ref() {
                        w.destroy();
                    }
                }
                ret
            });
            self.subsurfaces.retain_mut(|s: &mut WrapperSubsurface| s.handle_events());
            self.handle_overflow_popup_events(renderer);

            if prev == self.popups.len() && should_render {
                if let Err(e) = self.render(renderer, time, throttle, qh) {
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
        self.is_dirty = true;
        let (w, h) = configure.new_size;
        match self.space_event.take() {
            Some(e) => match e {
                SpaceEvent::WaitConfigure { first, mut width, mut height } => {
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
                    let dim = self.constrain_dim((width, height).into(), Some(self.gap() as u32));

                    if first {
                        if self.additional_gap != 0 {
                            let additional_gap = std::mem::take(&mut self.additional_gap);
                            // force update of the margin
                            self.set_additional_gap(additional_gap);
                        }
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
                            unsafe {
                                EGLDisplay::new(client_egl_display)
                                    .expect("Failed to create EGL display")
                            }
                        };

                        let egl_context = EGLContext::new_with_config(
                            &new_egl_display,
                            GlAttributes {
                                version: (2, 0),
                                profile: None,
                                debug: cfg!(debug_assertions),
                                vsync: false,
                            },
                            PixelFormatRequirements::_8_bit(),
                        )
                        .expect("Failed to create EGL context");

                        let mut new_renderer = if let Some(renderer) = renderer.take() {
                            renderer
                        } else {
                            unsafe {
                                let mut capabilities =
                                    GlesRenderer::supported_capabilities(&egl_context)
                                        .expect("Failed to query EGL Context");
                                // capabilities.retain(|cap| *cap != Capability::);
                                GlesRenderer::with_capabilities(egl_context, capabilities)
                                    .expect("Failed to create EGL Surface")
                            }
                        };

                        init_shaders(&mut new_renderer).expect("Failed to init shaders...");

                        let mut egl_surface = unsafe {
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
                        };
                        _ = unsafe {
                            new_renderer.egl_context().make_current_with_surface(&egl_surface)
                        };

                        // bind before setting swap interval
                        let _ = new_renderer.bind(&mut egl_surface);
                        let swap_success =
                            unsafe { SwapInterval(new_egl_display.get_display_handle().handle, 0) }
                                == 1;
                        if !swap_success {
                            error!("Failed to set swap interval");
                        }

                        renderer.replace(new_renderer);
                        self.egl_surface.replace(egl_surface);
                    }
                    if let (Some(renderer), Some(egl_surface)) =
                        (renderer.as_mut(), self.egl_surface.as_mut())
                    {
                        let scaled_size = dim.to_f64().to_physical(self.scale).to_i32_round();
                        _ = unsafe {
                            renderer.egl_context().make_current_with_surface(egl_surface)
                        };
                        let _ = renderer.bind(egl_surface);

                        egl_surface.resize(scaled_size.w, scaled_size.h, 0, 0);
                        if let Some(viewport) = self.layer_viewport.as_ref() {
                            viewport.set_destination(dim.w.max(1), dim.h.max(1));
                        }
                    }

                    self.dimensions = (dim.w, dim.h).into();
                    self.damage_tracked_renderer = Some(OutputDamageTracker::new(
                        dim.to_f64().to_physical(self.scale).to_i32_round(),
                        self.scale,
                        smithay::utils::Transform::Flipped180,
                    ));
                },
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
                let dim = self.constrain_dim((width, height).into(), Some(self.gap() as u32));

                if let (Some(renderer), Some(egl_surface)) =
                    (renderer.as_mut(), self.egl_surface.as_mut())
                {
                    _ = unsafe { renderer.egl_context().make_current_with_surface(egl_surface) };
                    let _ = renderer.bind(egl_surface);
                    let scaled_size = dim.to_f64().to_physical(self.scale).to_i32_round();
                    egl_surface.resize(scaled_size.w, scaled_size.h, 0, 0);

                    if let Some(viewport) = self.layer_viewport.as_ref() {
                        viewport.set_destination(dim.w, dim.h);
                    }
                }
                self.dimensions = (dim.w, dim.h).into();
                self.damage_tracked_renderer = Some(OutputDamageTracker::new(
                    dim.to_f64().to_physical(self.scale).to_i32_round(),
                    self.scale,
                    smithay::utils::Transform::Flipped180,
                ));
            },
        }
    }

    pub fn is_dark(&self, system_is_dark: bool) -> bool {
        match &self.config.background {
            CosmicPanelBackground::ThemeDefault | CosmicPanelBackground::Color(_) => system_is_dark,
            CosmicPanelBackground::Dark => true,
            CosmicPanelBackground::Light => false,
        }
    }

    pub fn set_theme(&mut self, colors: PanelColors) {
        let color = colors.bg_color(self.config.opacity);
        if let Some(animate_state) = self.animate_state.as_mut() {
            animate_state.end.bg_color = color;
        } else {
            let start = AnimatableState {
                bg_color: self.colors.bg_color(self.config.opacity),
                border_radius: self.config.border_radius,
                expanded: if self.config.expand_to_edges { 1.0 } else { 0.0 },
                gap: self.gap(),
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

        for e in self.space.elements() {
            let CosmicMappedInternal::OverflowButton(b) = e else {
                continue;
            };
            b.set_theme(colors.theme.clone());
            b.force_redraw();
        }
        for e in self
            .overflow_center
            .elements()
            .chain(self.overflow_left.elements())
            .chain(self.overflow_right.elements())
        {
            let PopupMappedInternal::Popup(e) = e else {
                continue;
            };
            e.set_theme(colors.theme.clone());
            e.force_redraw();
        }
        self.colors = colors;
    }

    /// clear the panel
    pub fn clear(&mut self) {
        self.is_dirty = true;
        self.close_popups(|_| false);
        self.overflow_popup = None;
        self.damage_tracked_renderer = Some(OutputDamageTracker::new(
            self.dimensions.to_f64().to_physical(self.scale).to_i32_round(),
            self.scale,
            smithay::utils::Transform::Flipped180,
        ));
        self.background_element = None;
        self.space.refresh();
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
        let p_offset = if let Some(s) = self.space.elements().find(|w| {
            s_surface
                .get_parent_surface()
                .is_some_and(|s| w.wl_surface().is_some_and(|w| w.as_ref() == &s))
        }) {
            self.space.element_location(s).unwrap_or_else(|| (0, 0).into())
        } else if let Some(p) = self.popups.iter().find(|p| {
            s_surface.get_parent_surface().is_some_and(|s| &s == p.s_surface.wl_surface())
        }) {
            p.popup.rectangle.loc
        } else if let Some(p) = self.overflow_popup.as_ref().and_then(|(_, section)| {
            let space = match section {
                OverflowSection::Left => &self.overflow_left,
                OverflowSection::Center => &self.overflow_center,
                OverflowSection::Right => &self.overflow_right,
            };
            space
                .elements()
                .find(|w| {
                    s_surface
                        .get_parent_surface()
                        .is_some_and(|s| w.wl_surface().is_some_and(|w| w.as_ref() == &s))
                })
                .map(|w| space.element_location(w).unwrap_or_else(|| (0, 0).into()))
        }) {
            p
        } else {
            tracing::warn!("No parent surface found for popup");
            (0, 0).into()
        };

        positioner.set_size(rect_size.w.max(1), rect_size.h.max(1));
        positioner.set_anchor_rect(
            anchor_rect.loc.x + p_offset.x,
            anchor_rect.loc.y + p_offset.y,
            anchor_rect.size.w,
            anchor_rect.size.h,
        );
        positioner.set_anchor(Anchor::try_from(anchor_edges as u32).unwrap_or(Anchor::None));
        positioner.set_gravity(Gravity::try_from(gravity as u32).unwrap_or(Gravity::None));

        positioner.set_constraint_adjustment(
            u32::from(constraint_adjustment).try_into().unwrap_or(ConstraintAdjustment::empty()),
        );
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

    pub fn update_config(
        &mut self,
        config: CosmicPanelConfig,
        bg_color: Option<[f32; 4]>,
        animate: bool,
    ) {
        let bg_color = bg_color.unwrap_or_else(|| self.colors.bg_color(config.opacity));
        // avoid animating if currently maximized
        if self.maximized {
            return;
        }

        // can't animate anchor changes
        // return early
        if config.anchor() != self.config.anchor() {
            panic!(
                "Can't apply anchor changes when orientation changes. Requires re-creation of \
                     the panel."
            );
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

                l.set_exclusive_zone(list_thickness);
                needs_commit = true;
            }
        }

        if config.autohide.is_none() && self.config.autohide.is_some() {
            if let Some(l) = self.layer.as_ref() {
                let margin = config.get_effective_anchor_gap() as i32;
                Self::set_margin(config.anchor, margin, self.additional_gap, l);
                self.anchor_gap = 0;
                let list_thickness = match self.config.anchor() {
                    PanelAnchor::Left | PanelAnchor::Right => self.dimensions.w,
                    PanelAnchor::Top | PanelAnchor::Bottom => self.dimensions.h,
                };
                l.set_exclusive_zone(list_thickness);
                let (width, height) = if self.config.is_horizontal() {
                    (0, self.dimensions.h)
                } else {
                    (self.dimensions.w, 0)
                };
                l.set_size(width as u32, height as u32);
                needs_commit = true;
            }
        } else if self.config.get_effective_anchor_gap() != config.get_effective_anchor_gap() {
            if let Some(l) = self.layer.as_ref() {
                let margin = config.get_effective_anchor_gap() as i32;
                Self::set_margin(config.anchor, margin, self.additional_gap, l);
                self.anchor_gap = 0;
                needs_commit = true;
            }
        }

        if config.anchor_gap != self.config.anchor_gap {
            if self.config.is_horizontal() {
                if let Some(l) = self.suggested_length {
                    self.dimensions.w = l as i32;
                }
            } else if let Some(l) = self.suggested_length {
                self.dimensions.h = l as i32;
            }
        }

        if self.config.expand_to_edges != config.expand_to_edges {
            self.reset_overflow();
        }

        if needs_commit {
            if let Some(l) = self.layer.as_ref() {
                l.commit();
            }
        }

        if animate {
            let start = AnimatableState {
                bg_color: self.colors.bg_color(self.config.opacity),
                border_radius: self.config.border_radius,
                expanded: if self.config.expand_to_edges { 1.0 } else { 0.0 },
                gap: self.gap(),
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
        }

        self.config = config;

        self.clear();
    }

    pub fn reset_overflow(&mut self) {
        // re-map all windows to the main space from overflow
        // remove all overflow buttons and popups
        let overflow = self
            .overflow_left
            .elements()
            .cloned()
            .chain(self.overflow_center.elements().cloned())
            .chain(self.overflow_right.elements().cloned())
            .collect::<Vec<_>>();

        for e in overflow {
            self.overflow_left.unmap_elem(&e);
            self.overflow_center.unmap_elem(&e);
            self.overflow_right.unmap_elem(&e);
            let window = match e {
                PopupMappedInternal::Window(w) => w,
                _ => continue,
            };
            let Some(wl_surface) = window.wl_surface() else {
                continue;
            };
            with_states(&wl_surface, |states| {
                with_fractional_scale(states, |fractional_scale| {
                    fractional_scale.set_preferred_scale(self.scale);
                });
            });
            self.space.map_element(CosmicMappedInternal::Window(window), (0, 0), false);
        }
        // remove all button elements from the space
        let buttons = self
            .space
            .elements()
            .cloned()
            .filter_map(|e| {
                if let CosmicMappedInternal::OverflowButton(b) = e {
                    Some(b)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        for b in buttons {
            self.space.unmap_elem(&CosmicMappedInternal::OverflowButton(b.clone()));
        }

        // send None for configure to force re-configure all windows
        let elements = self.space.elements().cloned().collect::<Vec<_>>();
        for e in elements {
            if let CosmicMappedInternal::Window(w) = e {
                if let Some(t) = w.toplevel() {
                    t.with_pending_state(|s| {
                        s.size = None;
                    });
                    t.send_pending_configure();
                }
            }
        }
        self.close_popups(|_| false);
    }

    pub fn set_maximized(&mut self, maximized: bool, config: CosmicPanelConfig, opacity: f32) {
        if self.maximized == maximized {
            return;
        }
        let bg_color = self.colors.bg_color(opacity);
        if !self.maximized {
            self.update_config(config, Some(bg_color), self.config.autohide.is_none());
            self.maximized = maximized;
        } else {
            self.maximized = maximized;
            self.update_config(config, Some(bg_color), self.config.autohide.is_none());
            if let Some(s) = self.animate_state.as_mut() {
                s.end.bg_color[3] = self.config.opacity;
            }
        }
    }

    pub fn cleanup(&mut self) {}

    pub fn dirty_subsurface(
        &mut self,
        renderer: Option<&mut GlesRenderer>,
        compositor_state: &sctk::compositor::CompositorState,
        wl_subcompositor: &SubcompositorState,
        fractional_scale_manager: Option<&FractionalScalingManager>,
        viewport: Option<&ViewporterState>,
        qh: &QueueHandle<GlobalState>,
        wlsurface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    ) {
        let Some(renderer) = renderer else {
            return;
        };
        self.is_dirty = true;
        self.space.refresh();

        let mut s_bbox = bbox_from_surface_tree(&wlsurface, (0, 0));
        s_bbox.size.w += 20;
        if let Some(s) = self.subsurfaces.iter_mut().find(|s| s.s_surface == *wlsurface) {
            let Some(offset) = self.space.element_location(&s.parent) else {
                return;
            };

            if s_bbox != s.subsurface.rectangle && s_bbox.size.w > 0 && s_bbox.size.h > 0 {
                let p_s_bbox = s_bbox.to_f64().to_physical_precise_round(self.scale);
                _ = unsafe {
                    renderer.egl_context().make_current_with_surface(&s.subsurface.egl_surface)
                };
                s.subsurface.egl_surface.resize(p_s_bbox.size.w, p_s_bbox.size.h, 0, 0);
                s.subsurface
                    .c_subsurface
                    .set_position(offset.x + s_bbox.loc.x, offset.y + s_bbox.loc.y);
                s.subsurface.rectangle = s_bbox;

                if let Some(viewport) = &s.subsurface.viewport {
                    viewport.set_destination(s_bbox.size.w.max(1), s_bbox.size.h.max(1));
                }
            }

            s.subsurface.dirty = true;
        } else if let Some(ls) = self.layer.as_ref() {
            if s_bbox.size.w == 0 || s_bbox.size.h == 0 {
                return;
            }
            let Some(parent_id) = self.space.elements().find(|m| {
                let Some(t) = m.toplevel() else {
                    return false;
                };
                t.wl_surface().client() == wlsurface.client()
            }) else {
                return;
            };

            let Some(offset) = self.space.element_location(&parent_id) else {
                return;
            };
            // create and insert subsurface
            // let new_surface = self
            let (c_subsurface, c_surface) =
                wl_subcompositor.create_subsurface(ls.wl_surface().clone(), &qh);
            let fractional_scale =
                fractional_scale_manager.map(|f| f.fractional_scaling(&c_surface, qh));

            let viewport = viewport.map(|v| {
                with_states(wlsurface, |states| {
                    with_fractional_scale(states, |fractional_scale| {
                        fractional_scale.set_preferred_scale(self.scale);
                    });
                });
                let viewport = v.get_viewport(&c_surface, qh);
                viewport.set_destination(s_bbox.size.w.max(1), s_bbox.size.h.max(1));
                viewport
            });
            if fractional_scale.is_none() {
                c_surface.set_buffer_scale(self.scale as i32);
            }
            let input_region = Region::new(compositor_state).ok();

            // TODO: support input in subsurfaces...
            c_surface.set_input_region(input_region.as_ref().map(|r| r.wl_region()));
            let p_s_bbox = s_bbox.to_f64().to_physical_precise_round(self.scale);
            let wl_egl_surface =
                match WlEglSurface::new(c_surface.id(), p_s_bbox.size.w, p_s_bbox.size.h) {
                    Ok(s) => s,
                    Err(err) => {
                        tracing::error!("Failed to create WlEglSurface: {:?}", err);
                        return;
                    },
                };
            let client_egl_surface =
                unsafe { ClientEglSurface::new(wl_egl_surface, c_surface.clone()) };

            c_subsurface.set_position(offset.x + s_bbox.loc.x, offset.y + s_bbox.loc.y);

            c_surface.commit();

            self.subsurfaces.push(WrapperSubsurface {
                parent: parent_id.clone(),
                subsurface: PanelSubsurface {
                    egl_surface: unsafe {
                        EGLSurface::new(
                            renderer.egl_context().display(),
                            renderer
                                .egl_context()
                                .pixel_format()
                                .expect("Failed to get pixel format from EGL context "),
                            renderer.egl_context().config_id(),
                            client_egl_surface,
                        )
                        .expect("Failed to initialize EGL Surface")
                    },
                    c_subsurface,
                    c_surface,
                    dirty: true,
                    rectangle: s_bbox,
                    wrapper_rectangle: offset,
                    has_frame: true,
                    fractional_scale,
                    viewport,
                    scale: self.scale,
                    damage_tracked_renderer: OutputDamageTracker::new(
                        s_bbox.size.to_f64().to_physical(self.scale).to_i32_round(),
                        self.scale,
                        smithay::utils::Transform::Flipped180,
                    ),
                    parent: ls.wl_surface().clone(),
                },
                s_surface: wlsurface.clone(),
            });
        }
    }

    pub(crate) fn grab(&mut self, surface: PopupSurface, seat: wl_seat::WlSeat, serial: Serial) {
        if let Some(p) = self.popups.iter_mut().find(|p| p.s_surface == surface) {
            p.popup.grab = true;
        }
    }
}

impl Drop for PanelSpace {
    fn drop(&mut self) {
        // request processes to stop
        let _ = self.applet_tx.try_send(AppletMsg::Cleanup(self.id()));
    }
}
