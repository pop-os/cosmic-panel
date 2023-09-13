use std::{
    cell::{Cell, RefCell},
    os::{fd::RawFd, unix::net::UnixStream},
    rc::Rc,
    time::{Duration, Instant},
};

use cosmic_config::{Config, CosmicConfigEntry};
use launch_pad::process::Process;
use sctk::{
    compositor::Region,
    output::OutputInfo,
    reexports::client::{
        backend::ObjectId,
        protocol::{wl_display::WlDisplay, wl_output as c_wl_output},
        Proxy, QueueHandle,
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
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                surface::WaylandSurfaceRenderElement,
            },
            Bind, ImportAll, ImportMem, Unbind,
        },
    },
    output::Output,
    reexports::{
        wayland_protocols::xdg::shell::client::xdg_positioner::{Anchor, Gravity},
        wayland_server::{backend::ClientId, DisplayHandle},
    },
    render_elements,
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
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::WpFractionalScaleV1;
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
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

pub enum AppletMsg {
    NewProcess(ObjectId, Process),
    NewNotificationsProcess(ObjectId, Process, Vec<(String, String)>),
    NeedNewNotificationFd(oneshot::Sender<RawFd>),
    ClientSocketPair(String, ClientId, Client, UnixStream),
    Cleanup(ObjectId),
}

render_elements! {
    pub(crate) MyRenderElements<R> where R: ImportMem + ImportAll;
    Memory=MemoryRenderBufferRenderElement<R>,
    WaylandSurface=WaylandSurfaceRenderElement<R>
}

/// space for the cosmic panel
#[derive(Debug)]
pub(crate) struct PanelSpace {
    // XXX implicitly drops egl_surface first to avoid segfault
    pub(crate) egl_surface: Option<Rc<EGLSurface>>,
    pub(crate) c_display: Option<WlDisplay>,
    pub config: CosmicPanelConfig,
    pub(crate) space: Space<Window>,
    pub(crate) damage_tracked_renderer: Option<OutputDamageTracker>,
    pub(crate) clients_left: Vec<(String, Client, UnixStream)>,
    pub(crate) clients_center: Vec<(String, Client, UnixStream)>,
    pub(crate) clients_right: Vec<(String, Client, UnixStream)>,
    pub(crate) last_dirty: Option<Instant>,
    // pending size of the panel
    pub(crate) pending_dimensions: Option<Size<i32, Logical>>,
    // suggested length of the panel
    pub(crate) suggested_length: Option<u32>,
    // size of the panel
    pub(crate) actual_size: Size<i32, Logical>,
    // dimensions of the layer surface
    pub(crate) dimensions: Size<i32, Logical>,
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
    pub(crate) bg_color: [f32; 4],
    pub(crate) applet_tx: mpsc::Sender<AppletMsg>,
    pub(crate) input_region: Option<Region>,
    pub(crate) old_buff: Option<MemoryRenderBuffer>,
    pub(crate) buffer: Option<MemoryRenderBuffer>,
    pub(crate) buffer_changed: bool,
    pub(crate) has_frame: bool,
    pub(crate) scale: f64,
}

impl PanelSpace {
    /// create a new space for the cosmic panel
    pub fn new(
        config: CosmicPanelConfig,
        c_focused_surface: Rc<RefCell<ClientFocus>>,
        c_hovered_surface: Rc<RefCell<ClientFocus>>,
        applet_tx: mpsc::Sender<AppletMsg>,
    ) -> Self {
        let bg_color = match config.background {
            CosmicPanelBackground::ThemeDefault => {
                let t = Config::new("com.system76.CosmicTheme", 1)
                    .map(|helper| match cosmic_theme::Theme::get_entry(&helper) {
                        Ok(c) => c,
                        Err((err, c)) => {
                            for e in err {
                                error!("Error loading cosmic theme for {} {:?}", &config.name, e);
                            }
                            c
                        }
                    })
                    .unwrap_or(cosmic_theme::Theme::dark_default());
                let c = [
                    t.bg_color().red,
                    t.bg_color().green,
                    t.bg_color().blue,
                    config.opacity,
                ];
                c
            }
            CosmicPanelBackground::Dark => {
                let t = cosmic_theme::Theme::dark_default();
                let c = [
                    t.bg_color().red,
                    t.bg_color().green,
                    t.bg_color().blue,
                    config.opacity,
                ];
                c
            }
            CosmicPanelBackground::Light => {
                let t = cosmic_theme::Theme::light_default();
                let c = [
                    t.bg_color().red,
                    t.bg_color().green,
                    t.bg_color().blue,
                    config.opacity,
                ];
                c
            }
            CosmicPanelBackground::Color(c) => [c[0], c[1], c[2], config.opacity],
        };

        let visibility = if config.autohide.is_none() {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };

        Self {
            config,
            space: Space::default(),
            clients_left: Default::default(),
            clients_center: Default::default(),
            clients_right: Default::default(),
            last_dirty: Default::default(),
            pending_dimensions: Default::default(),
            space_event: Default::default(),
            dimensions: Default::default(),
            suggested_length: None,
            output: Default::default(),
            s_display: Default::default(),
            c_display: Default::default(),
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
            damage_tracked_renderer: Default::default(),
            is_dirty: false,
            old_buff: Default::default(),
            buffer: Default::default(),
            buffer_changed: false,
            has_frame: true,
            scale: 1.0,
        }
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
            if self.config.autohide().is_none() {
                if c_focused_surface
                    .iter()
                    .all(|f| matches!(f.2, FocusStatus::LastFocused(_)))
                    && c_hovered_surface
                        .iter()
                        .all(|f| matches!(f.2, FocusStatus::LastFocused(_)))
                {
                    self.visibility = Visibility::Hidden;
                } else {
                    self.visibility = Visibility::Visible;
                }
                return;
            }

            c_hovered_surface.iter().fold(
                FocusStatus::LastFocused(self.start_instant),
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
                            layer_surface.set_exclusive_zone(panel_size + handle);
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
    }

    fn set_margin(anchor: PanelAnchor, margin: i32, target: i32, layer_surface: &LayerSurface) {
        match anchor {
            PanelAnchor::Left => layer_surface.set_margin(margin, 0, margin, target),
            PanelAnchor::Right => layer_surface.set_margin(margin, target, margin, 0),
            PanelAnchor::Top => layer_surface.set_margin(target, margin, 0, margin),
            PanelAnchor::Bottom => layer_surface.set_margin(0, margin, target, margin),
        };
    }

    pub(crate) fn constrain_dim(&self, size: Size<i32, Logical>) -> Size<i32, Logical> {
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

        let (constrained_w, constrained_h) = self
            .config
            .get_dimensions(output_dims, self.suggested_length);
        if let Some(w_range) = constrained_w {
            w = w.clamp(w_range.start as i32, w_range.end as i32 - 1);
        }
        if let Some(h_range) = constrained_h {
            h = h.clamp(h_range.start as i32, h_range.end as i32 - 1);
        }

        (w as i32, h as i32).into()
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
                        if self.config.margin > 0 {
                            Self::set_margin(
                                self.config.anchor,
                                self.config.margin as i32,
                                0,
                                layer_surface,
                            );
                        }
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
        layer: &LayerSurface,
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
                    let _ = self.spawn_clients(self.s_display.clone().unwrap());
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
                    let dim = self.constrain_dim((width as i32, height as i32).into());

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
                            layer.wl_surface().commit();
                        }
                    }

                    self.dimensions = (dim.w, dim.h).into();
                    self.damage_tracked_renderer = Some(OutputDamageTracker::new(
                        dim.to_f64().to_physical(self.scale).to_i32_round(),
                        1.0,
                        smithay::utils::Transform::Flipped180,
                    ));
                    self.layer.as_ref().unwrap().wl_surface().commit();
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
                let dim = self.constrain_dim((width as i32, height as i32).into());

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
                        layer.wl_surface().commit();
                    }
                }
                self.dimensions = (dim.w, dim.h).into();
                self.damage_tracked_renderer = Some(OutputDamageTracker::new(
                    dim.to_f64().to_physical(self.scale).to_i32_round(),
                    1.0,
                    smithay::utils::Transform::Flipped180,
                ));
                self.layer.as_ref().unwrap().wl_surface().commit();
            }
        }
    }

    pub fn set_theme_window_color(&mut self, mut color: [f32; 4]) {
        if let CosmicPanelBackground::ThemeDefault = self.config.background {
            color[3] = self.config.opacity;
        }
        self.bg_color = color;
        self.clear();
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
}

impl Drop for PanelSpace {
    fn drop(&mut self) {
        // request processes to stop
        if let Some(id) = self.layer.as_ref().map(|l| l.wl_surface().id()) {
            let _ = self.applet_tx.try_send(AppletMsg::Cleanup(id));
        }
    }
}
