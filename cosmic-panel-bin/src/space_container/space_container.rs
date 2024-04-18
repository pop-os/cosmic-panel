use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Arc};

use crate::{
    minimize::MinimizeApplet,
    space::{AppletMsg, PanelColors, PanelSpace},
    PanelCalloopMsg,
};
use cctk::{
    cosmic_protocols::toplevel_info::v1::client::zcosmic_toplevel_handle_v1::ZcosmicToplevelHandleV1,
    toplevel_info::ToplevelInfo, workspace::WorkspaceGroup,
};
use cosmic::{cosmic_config::CosmicConfigEntry, iced::id, theme};
use cosmic_panel_config::{
    CosmicPanelBackground, CosmicPanelConfig, CosmicPanelContainerConfig, CosmicPanelOuput,
    PanelAnchor,
};
use cosmic_theme::{Theme, ThemeMode};
use notify::RecommendedWatcher;
use sctk::{
    output::{self, OutputInfo},
    reexports::{
        calloop,
        client::{protocol::wl_output::WlOutput, Connection, QueueHandle},
    },
    shell::wlr_layer::LayerShell,
};
use smithay::{
    backend::renderer::gles::GlesRenderer,
    output::Output,
    reexports::wayland_server::{
        backend::ClientId,
        {self},
    },
};
use tokio::sync::mpsc;
use tracing::{error, info};
use wayland_server::Resource;
use xdg_shell_wrapper::{
    client_state::ClientFocus,
    shared_state::GlobalState,
    space::{Visibility, WrapperSpace},
    wp_fractional_scaling::FractionalScalingManager,
    wp_security_context::SecurityContextManager,
    wp_viewporter::ViewporterState,
};

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
    pub(crate) light_theme: cosmic::Theme,
    pub(crate) dark_theme: cosmic::Theme,
    pub(crate) security_context_manager: Option<SecurityContextManager>,
    /// map from output name to minimized applet info
    pub(crate) minimized_applets: HashMap<String, MinimizeApplet>,
    pub(crate) loop_handle: calloop::LoopHandle<'static, GlobalState<SpaceContainer>>,
}

impl SpaceContainer {
    pub fn new(
        config: CosmicPanelContainerConfig,
        tx: mpsc::Sender<AppletMsg>,
        panel_tx: calloop::channel::SyncSender<PanelCalloopMsg>,
        loop_handle: calloop::LoopHandle<'static, GlobalState<SpaceContainer>>,
    ) -> Self {
        let is_dark = ThemeMode::config()
            .ok()
            .and_then(|c| ThemeMode::get_entry(&c).ok())
            .unwrap_or_default()
            .is_dark;

        let light = Theme::light_config()
            .ok()
            .and_then(|c| Theme::get_entry(&c).ok())
            .unwrap_or_else(|| Theme::light_default());
        let dark = Theme::dark_config()
            .ok()
            .and_then(|c| Theme::get_entry(&c).ok())
            .unwrap_or_else(|| Theme::dark_default());

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
            light_theme: cosmic::Theme::system(Arc::new(light)),
            dark_theme: cosmic::Theme::system(Arc::new(dark)),
            security_context_manager: None,
            minimized_applets: HashMap::new(),
            loop_handle,
        }
    }

    pub fn set_dark(&mut self, theme: theme::CosmicTheme) {
        self.dark_theme = cosmic::Theme::system(Arc::new(theme));

        for space in &mut self.space_list {
            let is_dark = space.is_dark(self.is_dark);
            if is_dark {
                space.set_theme(
                    PanelColors::new(self.dark_theme.clone())
                        .with_color_override(space.config.bg_color_override()),
                );
            }
        }
    }

    pub fn set_light(&mut self, theme: theme::CosmicTheme) {
        self.light_theme = cosmic::Theme::system(Arc::new(theme));

        for space in &mut self.space_list {
            let is_dark = space.is_dark(self.is_dark);
            if !is_dark {
                space.set_theme(
                    PanelColors::new(self.light_theme.clone())
                        .with_color_override(space.config.bg_color_override()),
                );
            }
        }
    }

    pub fn cur_theme(&self) -> cosmic::Theme {
        if self.is_dark {
            self.dark_theme.clone()
        } else {
            self.light_theme.clone()
        }
    }

    pub fn cleanup_client(&mut self, old_client_id: ClientId) {
        for s in &mut self.space_list {
            // cleanup leftover windows
            let w = {
                s.space
                    .elements()
                    .find(|w| {
                        w.toplevel().is_some_and(|t| {
                            t.wl_surface().client().map(|c| c.id()).as_ref() == Some(&old_client_id)
                        })
                    })
                    .cloned()
            };
            let mut found_window = false;
            if let Some(w) = w {
                s.space.unmap_elem(&w);
                found_window = true;
            }
            let len = s.popups.len();
            // TODO handle cleanup of nested popups
            s.popups.retain(|p| {
                let Some(client) = p.s_surface.wl_surface().client() else {
                    return false;
                };
                client.id() != old_client_id
            });
            if found_window || len != s.popups.len() {
                s.is_dirty = true;
                break;
            }
        }
    }

    pub(crate) fn set_theme_mode(&mut self, is_dark: bool) {
        let changed = self.is_dark != is_dark;
        self.is_dark = is_dark;
        if changed {
            let cur = self.cur_theme();
            for space in &mut self.space_list {
                if matches!(space.config.background, CosmicPanelBackground::ThemeDefault) {
                    space.set_theme(
                        PanelColors::new(cur.clone())
                            .with_color_override(space.config.bg_color_override()),
                    );
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
        mut entry: CosmicPanelConfig,
        compositor_state: &sctk::compositor::CompositorState,
        fractional_scale_manager: Option<&FractionalScalingManager<W>>,
        viewport: Option<&ViewporterState<W>>,
        layer_state: &mut LayerShell,
        qh: &QueueHandle<GlobalState<W>>,
        force_output: Option<WlOutput>,
    ) {
        // if the output is set to "all", we need to check if the config is the same for
        // all outputs if the output is set to a specific output, we need to
        // make sure it doesn't exist on another output
        let mut output_count = if matches!(entry.output, CosmicPanelOuput::All) {
            self.outputs.len()
        } else {
            self.space_list.iter().filter(|s| s.config.name == entry.name).count()
        };

        if !force_output.is_some()
            && self.space_list.iter_mut().any(|s| {
                let ret = if matches!(entry.output, CosmicPanelOuput::All) {
                    entry.output = s.config.output.clone();
                    let ret = s.config == entry;
                    entry.output = CosmicPanelOuput::All;
                    ret
                } else {
                    s.config == entry
                };
                if ret {
                    output_count -= 1;
                }
                return output_count <= 0;
            })
        {
            info!("config unchanged, skipping");
            return;
        } else {
            info!("config changed, updating");
        }

        let connection = match self.connection.as_ref() {
            Some(c) => c,
            None => return,
        };

        let output_count_mismatch = match entry.output {
            CosmicPanelOuput::All => {
                self.space_list.iter().filter(|s| s.config.name == entry.name).count()
                    != self.outputs.len()
            },
            CosmicPanelOuput::Name(_) => {
                self.space_list.iter().filter(|s| s.config.name == entry.name).count() != 1
            },
            _ => true,
        };
        let new_priority = entry.get_priority();
        let (old_priority, old_anchor) = self
            .config
            .config_list
            .iter()
            .find(|c| c.name == entry.name)
            .map(|c| (c.get_priority(), c.anchor))
            .unwrap_or((0, entry.anchor));

        let opposite_anchor = if old_anchor == entry.anchor {
            None
        } else {
            Some(match entry.anchor {
                PanelAnchor::Top => PanelAnchor::Bottom,
                PanelAnchor::Bottom => PanelAnchor::Top,
                PanelAnchor::Left => PanelAnchor::Right,
                PanelAnchor::Right => PanelAnchor::Left,
            })
        };
        // recreate the original if: output changed
        // or if the output is the same, but the priority changes to conflict with an
        // adjacent panel or if applet size changes
        let must_recreate =
        // implies that there is at least one output which needs to be recreated
        output_count_mismatch
        || self.config.config_list.iter().any(|c| {
            // size changed
            c.name == entry.name && c.size != entry.size
            // output changed
            || (entry.output != CosmicPanelOuput::All &&
            (c.name == entry.name && c.output != entry.output))
            // panel anchor change forces restart
            || opposite_anchor.is_some()
            // applet restarts are required
            || ((c.name == entry.name
                && (c.is_horizontal() != entry.is_horizontal()
                || c.size != entry.size
                || c.background != entry.background
                || c.plugins_center != entry.plugins_center
                || c.plugins_wings != entry.plugins_wings)))
            // Priority change to conflict with adjacent panel
            || c.name != entry.name
                && Some(c.anchor) != opposite_anchor
                && ((old_priority < c.get_priority() && new_priority > c.get_priority() || old_priority > c.get_priority() && new_priority < c.get_priority()))}
            || c.name != entry.name && old_priority != new_priority && c.anchor == entry.anchor
        );

        self.config.config_list.retain(|c| c.name != entry.name);
        self.config.config_list.push(entry.clone());

        if !must_recreate {
            let bg_color = match entry.background {
                CosmicPanelBackground::Color(c) => Some([c[0], c[1], c[2], entry.opacity]),
                _ => None,
            };

            for space in &mut self.space_list {
                if space.config.name != entry.name {
                    continue;
                }

                entry.output = space.config.output.clone();
                space.update_config(entry.clone(), bg_color, true);
            }
            self.apply_toplevel_changes();
            return;
        }

        // remove old one if it exists
        self.space_list.retain(|s| {
            // keep if the name is different or the output is different
            s.config.name != entry.name
                || force_output.is_some()
                    && s.output
                        .as_ref()
                        .map(|(wl_output, ..)| Some(wl_output) != force_output.as_ref())
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
                        CosmicPanelBackground::ThemeDefault | CosmicPanelBackground::Color(_) => {
                            self.cur_theme()
                        },
                        CosmicPanelBackground::Dark => self.dark_theme.clone(),
                        CosmicPanelBackground::Light => self.light_theme.clone(),
                    },
                    self.s_display.clone().unwrap(),
                    self.security_context_manager.clone(),
                    self.connection.as_ref().unwrap(),
                    self.panel_tx.clone(),
                    xdg_shell_wrapper::space::Visibility::Visible,
                    self.loop_handle.clone(),
                );
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
            },
            CosmicPanelOuput::All => self.outputs.iter().collect(),
            CosmicPanelOuput::Name(name) => {
                self.outputs.iter().filter(|(_, output, _)| &output.name() == name).collect()
            },
        };

        let maximized_outputs = self.maximized_outputs();
        for (wl_output, output, info) in outputs {
            let output_name = output.name();
            let has_toplevel = self.toplevels.iter().any(|(_, t)| t.output.contains(wl_output));
            if force_output.as_ref() != Some(wl_output) && force_output.is_some() {
                continue;
            }

            let maximized_output = maximized_outputs.contains(wl_output);
            let mut configs = self.config.configs_for_output(&output_name);
            configs.sort_by(|a, b| b.get_priority().cmp(&a.get_priority()));
            for c in &configs {
                let is_recreated = c.name == entry.name
                    || Some(c.anchor) == opposite_anchor && c.get_priority() < new_priority
                    || configs.iter().any(|other| {
                        let other_opposite_anchor = match other.anchor {
                            PanelAnchor::Top => PanelAnchor::Bottom,
                            PanelAnchor::Bottom => PanelAnchor::Top,
                            PanelAnchor::Left => PanelAnchor::Right,
                            PanelAnchor::Right => PanelAnchor::Left,
                        };
                        c.anchor != other_opposite_anchor && c.get_priority() < other.get_priority()
                    });

                if !is_recreated {
                    continue;
                }
                let visible = if c.autohide.is_none() || !has_toplevel {
                    Visibility::Visible
                } else {
                    Visibility::Hidden
                };
                // remove old one if it exists
                self.space_list.retain(|s| {
                    // keep if the name is different or the output is different
                    s.config.name != c.name
                        || s.output.as_ref().is_some_and(|(_, o, _)| o.name() != output_name)
                });
                let mut new_config = (*c).clone();
                if maximized_output {
                    new_config.maximize();
                }
                new_config.output = CosmicPanelOuput::Name(output_name.clone());
                let mut space = PanelSpace::new(
                    new_config.clone(),
                    self.c_focused_surface.clone(),
                    self.c_hovered_surface.clone(),
                    self.applet_tx.clone(),
                    match entry.background {
                        CosmicPanelBackground::ThemeDefault | CosmicPanelBackground::Color(_) => {
                            self.cur_theme()
                        },
                        CosmicPanelBackground::Dark => self.dark_theme.clone(),
                        CosmicPanelBackground::Light => self.light_theme.clone(),
                    },
                    self.s_display.clone().unwrap(),
                    self.security_context_manager.clone(),
                    self.connection.as_ref().unwrap(),
                    self.panel_tx.clone(),
                    visible,
                    self.loop_handle.clone(),
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

    pub fn stacked_spaces_by_priority(
        &mut self,
        output_id: &str,
        anchor: PanelAnchor,
    ) -> Vec<&mut PanelSpace> {
        let mut spaces = self
            .space_list
            .iter_mut()
            .filter(|s| {
                s.output.as_ref().is_some_and(|o| o.1.name().as_str() == output_id)
                    && &s.config.anchor == &anchor
            })
            .collect::<Vec<_>>();
        if spaces.last().is_some_and(|s| s.config.autohide.is_none()) {
            spaces.remove(spaces.len() - 1);
        }
        spaces.sort_by(|a, b| a.config.get_priority().cmp(&b.config.get_priority()));
        spaces.reverse();
        spaces
    }

    pub fn toggle_overflow_popup(&mut self, id: id::Id) {
        // TODO implement
    }
}
