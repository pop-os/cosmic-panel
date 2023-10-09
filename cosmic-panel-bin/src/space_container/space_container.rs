use std::{cell::RefCell, collections::HashMap, os::unix::net::UnixStream, rc::Rc};

use crate::{
    space::{AppletMsg, PanelSpace},
    PanelCalloopMsg,
};
use cctk::{
    cosmic_protocols::toplevel_info::v1::client::zcosmic_toplevel_handle_v1::ZcosmicToplevelHandleV1,
    toplevel_info::ToplevelInfo, workspace::WorkspaceGroup,
};
use cosmic_config::CosmicConfigEntry;
use cosmic_panel_config::{
    CosmicPanelBackground, CosmicPanelConfig, CosmicPanelContainerConfig, CosmicPanelOuput,
};
use cosmic_theme::{palette, Theme, ThemeMode};
use notify::RecommendedWatcher;
use sctk::{
    output::OutputInfo,
    reexports::{
        calloop,
        client::{protocol::wl_output::WlOutput, Connection, QueueHandle},
    },
    shell::wlr_layer::LayerShell,
};
use smithay::{
    backend::renderer::gles::GlesRenderer,
    output::Output,
    reexports::wayland_server::{self, backend::ClientId, Client},
};
use tokio::sync::mpsc;
use tracing::{error, info};
use wayland_server::Resource;
use xdg_shell_wrapper::{
    client_state::ClientFocus, shared_state::GlobalState, space::WrapperSpace,
    wp_fractional_scaling::FractionalScalingManager, wp_viewporter::ViewporterState,
};

#[derive(Debug)]
pub struct SpaceContainer {
    pub(crate) connection: Option<Connection>,
    pub(crate) config: CosmicPanelContainerConfig,
    pub(crate) space_list: Vec<PanelSpace>,
    pub(crate) renderer: Option<GlesRenderer>,
    pub(crate) s_display: Option<wayland_server::DisplayHandle>,
    pub(crate) c_focused_surface: Rc<RefCell<ClientFocus>>,
    pub(crate) c_hovered_surface: Rc<RefCell<ClientFocus>>,
    pub applet_tx: mpsc::Sender<AppletMsg>,
    pub panel_tx: calloop::channel::SyncSender<PanelCalloopMsg>,
    pub(crate) outputs: Vec<(WlOutput, Output, OutputInfo)>,
    pub(crate) watchers: HashMap<String, RecommendedWatcher>,
    pub(crate) maximized_toplevels: Vec<(ZcosmicToplevelHandleV1, ToplevelInfo)>,
    pub(crate) toplevels: Vec<(ZcosmicToplevelHandleV1, ToplevelInfo)>,
    pub(crate) workspace_groups: Vec<WorkspaceGroup>,
    pub(crate) is_dark: bool,
    pub(crate) light_bg: [f32; 4],
    pub(crate) dark_bg: [f32; 4],
}

impl SpaceContainer {
    pub fn new(
        config: CosmicPanelContainerConfig,
        tx: mpsc::Sender<AppletMsg>,
        panel_tx: calloop::channel::SyncSender<PanelCalloopMsg>,
    ) -> Self {
        let is_dark = ThemeMode::config()
            .ok()
            .and_then(|c| ThemeMode::get_entry(&c).ok())
            .unwrap_or_default()
            .is_dark;

        let light = Theme::<palette::Srgba>::light_config()
            .ok()
            .and_then(|c| Theme::get_entry(&c).ok())
            .unwrap_or_else(|| Theme::light_default());
        let dark = Theme::<palette::Srgba>::dark_config()
            .ok()
            .and_then(|c| Theme::get_entry(&c).ok())
            .unwrap_or_else(|| Theme::dark_default());
        let light = light.background.base;
        let dark = dark.background.base;

        Self {
            connection: None,
            config,
            space_list: Vec::with_capacity(1),
            renderer: None,
            s_display: None,
            c_focused_surface: Default::default(),
            c_hovered_surface: Default::default(),
            applet_tx: tx,
            panel_tx,
            outputs: vec![],
            watchers: HashMap::new(),
            maximized_toplevels: Vec::with_capacity(1),
            toplevels: Vec::new(),
            workspace_groups: Vec::new(),
            is_dark,
            light_bg: [light.red, light.green, light.blue, light.alpha],
            dark_bg: [dark.red, dark.green, dark.blue, dark.alpha],
        }
    }

    pub fn set_dark(&mut self, color: [f32; 4]) {
        self.dark_bg = color;
        if !self.is_dark {
            return;
        }
        for space in &mut self.space_list {
            if matches!(
                space.config.background,
                CosmicPanelBackground::ThemeDefault | CosmicPanelBackground::Dark
            ) {
                space.set_theme_window_color(color);
            }
        }
    }

    pub fn set_light(&mut self, color: [f32; 4]) {
        self.light_bg = color;
        if self.is_dark {
            return;
        }
        for space in &mut self.space_list {
            if matches!(
                space.config.background,
                CosmicPanelBackground::ThemeDefault | CosmicPanelBackground::Light
            ) {
                space.set_theme_window_color(color);
            }
        }
    }

    pub fn cur_bg_color(&self) -> [f32; 4] {
        if self.is_dark {
            self.dark_bg.clone()
        } else {
            self.light_bg.clone()
        }
    }

    pub fn set_opacity(&mut self, opacity: f32, name: String) {
        for space in &mut self.space_list {
            if space.config.name != name {
                continue;
            }
            space.config.opacity = opacity;
            space.bg_color[3] = opacity;
            space.clear();
        }
    }

    pub fn replace_client(
        &mut self,
        id: String,
        old_client_id: ClientId,
        client: Client,
        socket: UnixStream,
    ) {
        for s in &mut self.space_list {
            if let Some((_, s_client, s_socket)) = s
                .clients_left
                .iter_mut()
                .chain(s.clients_center.iter_mut())
                .chain(s.clients_right.iter_mut())
                .find(|(c_id, old_client, _)| c_id == &id && old_client_id == old_client.id())
            {
                // cleanup leftover windows
                let w = {
                    s.space
                        .elements()
                        .find(|w| {
                            w.toplevel().wl_surface().client().map(|c| c.id())
                                == Some(s_client.id())
                        })
                        .cloned()
                };
                if let Some(w) = w {
                    s.space.unmap_elem(&w);
                }
                // TODO Popups?

                *s_client = client;
                *s_socket = socket;
                s.is_dirty = true;
                // s.w_accumulated_damage.clear();
                break;
            }
        }
    }

    pub(crate) fn set_theme_mode(&mut self, is_dark: bool) {
        let changed = self.is_dark == is_dark;
        self.is_dark = is_dark;
        if changed {
            let cur = self.cur_bg_color();
            for space in &mut self.space_list {
                if matches!(space.config.background, CosmicPanelBackground::ThemeDefault) {
                    space.set_theme_window_color(cur);
                }
            }
        }
    }

    /// apply a removed entry to the space list
    pub fn remove_space(&mut self, name: String) {
        self.space_list.retain(|s| s.config.name != name);
        self.config.config_list.retain(|c| c.name != name);
        self.watchers.remove(&name);
    }

    /// apply a new or updated entry to the space list
    pub fn update_space<W: WrapperSpace>(
        &mut self,
        entry: CosmicPanelConfig,
        compositor_state: &sctk::compositor::CompositorState,
        fractional_scale_manager: Option<&FractionalScalingManager<W>>,
        viewport: Option<&ViewporterState<W>>,
        layer_state: &mut LayerShell,
        qh: &QueueHandle<GlobalState<W>>,
        force_output: Option<WlOutput>,
    ) {
        // exit early if the config hasn't actually changed
        if !force_output.is_some() && self.space_list.iter_mut().any(|s| s.config == entry) {
            info!("config unchanged, skipping");
            return;
        }

        // TODO: Lower priority panel surfaces are recreated on the same output as well after updating the config
        if let Some(config) = self
            .config
            .config_list
            .iter_mut()
            .find(|c| c.name == entry.name)
        {
            *config = entry.clone();
        } else {
            self.config.config_list.push(entry.clone());
        }

        let connection = match self.connection.as_ref() {
            Some(c) => c,
            None => return,
        };

        // remove old one if it exists
        self.space_list.retain(|s| {
            s.config.name != entry.name
                || s.output
                    .as_ref()
                    .map(|(wl_output, _, _)| Some(wl_output) != force_output.as_ref())
                    .unwrap_or_default()
        });

        let outputs: Vec<_> = match &entry.output {
            CosmicPanelOuput::Active => {
                let mut space = PanelSpace::new(
                    entry.clone(),
                    self.c_focused_surface.clone(),
                    self.c_hovered_surface.clone(),
                    self.applet_tx.clone(),
                    match entry.background {
                        CosmicPanelBackground::ThemeDefault => self.cur_bg_color(),
                        CosmicPanelBackground::Dark => self.dark_bg,
                        CosmicPanelBackground::Light => self.light_bg,
                        CosmicPanelBackground::Color(c) => [c[0], c[1], c[1], 1.0],
                    },
                );
                if let Some(s_display) = self.s_display.as_ref() {
                    space.set_display_handle(s_display.clone());
                }
                if let Err(err) = space.new_output(
                    compositor_state,
                    fractional_scale_manager,
                    viewport,
                    layer_state,
                    connection,
                    qh,
                    None,
                    None,
                    None,
                ) {
                    error!("Failed to create space for active output: {}", err);
                } else {
                    self.space_list.push(space);
                }
                vec![]
            }
            CosmicPanelOuput::All => self.outputs.iter().collect(),
            CosmicPanelOuput::Name(name) => self
                .outputs
                .iter()
                .filter(|(_, output, _)| &output.name() == name)
                .collect(),
        };

        let maximized_outputs = self.maximized_outputs();
        for (wl_output, output, info) in outputs {
            if force_output.as_ref() != Some(wl_output) && force_output.is_some() {
                continue;
            }

            let output_name = output.name();
            let maximized_output = maximized_outputs.contains(wl_output);
            let mut configs = self.config.configs_for_output(&output_name);
            configs.sort_by(|a, b| b.get_priority().cmp(&a.get_priority()));
            for c in configs {
                self.space_list.retain(|s| {
                    s.config.name != c.name || Some(wl_output) != s.output.as_ref().map(|o| &o.0)
                });
                let mut new_config = c.clone();
                if maximized_output {
                    new_config.expand_to_edges = true;
                    new_config.margin = 0;
                    new_config.border_radius = 0;
                    new_config.opacity = 1.0;
                }
                let mut space = PanelSpace::new(
                    new_config.clone(),
                    self.c_focused_surface.clone(),
                    self.c_hovered_surface.clone(),
                    self.applet_tx.clone(),
                    match entry.background {
                        CosmicPanelBackground::ThemeDefault => self.cur_bg_color(),
                        CosmicPanelBackground::Dark => self.dark_bg,
                        CosmicPanelBackground::Light => self.light_bg,
                        CosmicPanelBackground::Color(c) => [c[0], c[1], c[1], 1.0],
                    },
                );
                if let Some(s_display) = self.s_display.as_ref() {
                    space.set_display_handle(s_display.clone());
                }
                if let Err(err) = space.new_output(
                    compositor_state,
                    fractional_scale_manager,
                    viewport,
                    layer_state,
                    connection,
                    qh,
                    Some(wl_output.clone()),
                    Some(output.clone()),
                    Some(info.clone()),
                ) {
                    error!("Failed to create space for output: {}", err);
                } else {
                    self.space_list.push(space);
                }
            }
        }
        self.apply_toplevel_changes();
    }
}
