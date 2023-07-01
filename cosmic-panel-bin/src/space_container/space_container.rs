use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    os::unix::net::UnixStream,
    rc::Rc,
};

use crate::space::{AppletMsg, PanelSpace};
use cosmic_panel_config::{
    CosmicPanelBackground, CosmicPanelConfig, CosmicPanelContainerConfig, CosmicPanelOuput,
};
use notify::RecommendedWatcher;
use sctk::{
    output::OutputInfo,
    reexports::client::{protocol::wl_output::WlOutput, Connection, QueueHandle},
    shell::wlr_layer::LayerShell,
};
use smithay::{
    backend::renderer::gles::GlesRenderer,
    output::Output,
    reexports::{
        calloop::channel::SyncSender,
        wayland_server::{self, backend::ClientId, Client},
    },
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
    pub(crate) outputs: Vec<(WlOutput, Output, OutputInfo)>,
    pub(crate) watchers: HashMap<String, RecommendedWatcher>,
    /// numerical id of the applet pointing to a oneshot channel and the name of its space
    pub(crate) pending_notification_applet_ids: Vec<(String, UnixStream)>,
    pub(crate) notification_applet_ids: HashMap<u32, UnixStream>,
    pub(crate) notification_applet_spaces: HashSet<String>,
    pub(crate) notification_applet_tx: Option<SyncSender<(String, UnixStream)>>,
    pub(crate) notification_applet_counter: u32,
}

impl SpaceContainer {
    pub fn new(config: CosmicPanelContainerConfig, tx: mpsc::Sender<AppletMsg>) -> Self {
        Self {
            connection: None,
            config,
            space_list: Vec::with_capacity(1),
            renderer: None,
            s_display: None,
            c_focused_surface: Default::default(),
            c_hovered_surface: Default::default(),
            applet_tx: tx,
            outputs: vec![],
            watchers: HashMap::with_capacity(1),
            pending_notification_applet_ids: vec![],
            notification_applet_spaces: HashSet::with_capacity(1),
            notification_applet_ids: HashMap::with_capacity(1),
            notification_applet_tx: None,
            notification_applet_counter: 0,
        }
    }

    pub fn set_theme_window_color(&mut self, color: [f32; 4]) {
        for space in &mut self.space_list {
            if matches!(space.config.background, CosmicPanelBackground::ThemeDefault) {
                space.set_theme_window_color(color);
            }
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
    ) {
        // exit early if the config hasn't actually changed
        if self.space_list.iter_mut().any(|s| s.config == entry) {
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
        self.space_list.retain(|s| s.config.name != entry.name);

        let outputs: Vec<_> = match &entry.output {
            CosmicPanelOuput::Active => {
                let mut space = PanelSpace::new(
                    entry.clone(),
                    self.c_focused_surface.clone(),
                    self.c_hovered_surface.clone(),
                    self.applet_tx.clone(),
                    self.notification_applet_tx.clone(),
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

        for (wl_output, output, info) in outputs {
            let output_name = output.name();

            let mut space = PanelSpace::new(
                entry.clone(),
                self.c_focused_surface.clone(),
                self.c_hovered_surface.clone(),
                self.applet_tx.clone(),
                self.notification_applet_tx.clone(),
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

            // recreate the lower priority panels on the same output
            for c in self.config.configs_for_output(&output_name) {
                if c.get_priority() < entry.get_priority() {
                    self.space_list.retain(|s| {
                        s.config.name != c.name
                            || Some(wl_output) != s.output.as_ref().map(|o| &o.0)
                    });
                    let mut space = PanelSpace::new(
                        c.clone(),
                        self.c_focused_surface.clone(),
                        self.c_hovered_surface.clone(),
                        self.applet_tx.clone(),
                        self.notification_applet_tx.clone(),
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
        }
    }
}
