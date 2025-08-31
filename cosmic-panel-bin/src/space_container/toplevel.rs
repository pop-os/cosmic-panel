use std::collections::HashSet;

use crate::xdg_shell_wrapper::space::{ToplevelInfoSpace, ToplevelManagerSpace};
use cctk::{
    cosmic_protocols::{
        toplevel_info::v1::client::zcosmic_toplevel_handle_v1,
        toplevel_management::v1::client::zcosmic_toplevel_manager_v1,
    },
    toplevel_info::ToplevelInfo,
    wayland_client::{Connection, protocol::wl_output::WlOutput},
    wayland_protocols::ext::{
        foreign_toplevel_list::v1::client::ext_foreign_toplevel_handle_v1,
        workspace::v1::client::ext_workspace_handle_v1,
    },
};
use cosmic_panel_config::PanelAnchor;
use itertools::Itertools;
use sctk::reexports::client::Proxy;

use super::SpaceContainer;

impl ToplevelInfoSpace for SpaceContainer {
    /// A new toplevel was created
    fn new_toplevel(&mut self, _conn: &Connection, info: &ToplevelInfo) {
        self.toplevels.push(info.clone());
        self.apply_toplevel_changes();
        _ = self
            .panel_tx
            .send(crate::PanelCalloopMsg::UpdateToplevel(info.foreign_toplevel.clone()));

        let is_maximized = info.state.contains(&zcosmic_toplevel_handle_v1::State::Maximized);
        if is_maximized {
            self.add_maximized(info);
        }
    }

    /// A toplevel was updated
    fn update_toplevel(&mut self, _conn: &Connection, info: &ToplevelInfo) {
        if let Some(info_1) = self
            .toplevels
            .iter_mut()
            .find(|info_1| info_1.foreign_toplevel == info.foreign_toplevel)
        {
            *info_1 = info.clone();
        }
        _ = self
            .panel_tx
            .send(crate::PanelCalloopMsg::UpdateToplevel(info.foreign_toplevel.clone()));
        self.apply_toplevel_changes();

        let is_maximized = info.state.contains(&zcosmic_toplevel_handle_v1::State::Maximized);

        let was_maximized =
            self.maximized_toplevels.iter().any(|t| t.foreign_toplevel == info.foreign_toplevel);
        if is_maximized && !was_maximized {
            self.add_maximized(info);
        } else if !is_maximized && was_maximized {
            self.remove_maximized(&info.foreign_toplevel);
        }
    }

    /// A toplevel was closed
    fn toplevel_closed(
        &mut self,
        _conn: &Connection,
        toplevel: &ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
    ) {
        self.toplevels.retain(|t| t.foreign_toplevel != *toplevel);
        self.apply_toplevel_changes();

        if self.maximized_toplevels.iter().any(|t| t.foreign_toplevel == *toplevel) {
            self.remove_maximized(toplevel);
        }
    }
}

impl ToplevelManagerSpace for SpaceContainer {
    /// Supported capabilities
    fn capabilities(
        &mut self,
        _: &Connection,
        _: Vec<
            cctk::wayland_client::WEnum<
                zcosmic_toplevel_manager_v1::ZcosmicToplelevelManagementCapabilitiesV1,
            >,
        >,
    ) {
    }
}

impl SpaceContainer {
    fn add_maximized(&mut self, info: &ToplevelInfo) {
        let pre_maximixed_outputs = self.maximized_outputs();
        self.maximized_toplevels.push(info.clone());
        let post_maximized_outputs = self.maximized_outputs();
        let outputs = self.outputs.clone();
        for (o, ..) in &outputs {
            let max_pre = pre_maximixed_outputs.iter().contains(o);
            let max_post = post_maximized_outputs.iter().contains(o);
            if max_post && !max_pre {
                self.apply_maximized(o, true);
            } else if !max_post && max_pre {
                self.apply_maximized(o, false);
            }
        }
    }

    fn remove_maximized(
        &mut self,
        toplevel: &ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
    ) {
        let pre_maximixed_outputs = self.maximized_outputs();
        if let Some(pos) =
            self.maximized_toplevels.iter().position(|t| t.foreign_toplevel == *toplevel)
        {
            self.maximized_toplevels.remove(pos);
        } else {
            return;
        };
        let post_maximized_outputs = self.maximized_outputs();
        let outputs = self.outputs.clone();
        for (o, ..) in &outputs {
            let max_pre = pre_maximixed_outputs.iter().contains(o);
            let max_post = post_maximized_outputs.iter().contains(o);
            if max_post && !max_pre {
                self.apply_maximized(o, true);
            } else if !max_post && max_pre {
                self.apply_maximized(o, false);
            }
        }
        self.apply_toplevel_changes();
    }

    pub(crate) fn apply_maximized(&mut self, output: &WlOutput, maximized: bool) {
        let s_list = self
            .space_list
            .iter_mut()
            .filter(|s| s.output.as_ref().iter().any(|(o, ..)| o == output));
        for s in s_list.sorted_by(|a, b| a.config.get_priority().cmp(&b.config.get_priority())) {
            let c = self.config.config_list.iter().find(|c| c.name == s.config.name);
            let mut config = s.config.clone();

            let opacity = if maximized {
                config.maximize();
                1.0
            } else {
                if let Some(c) = c {
                    config = c.clone();
                }
                config.opacity
            };

            s.set_maximized(maximized, config, opacity)
        }
    }

    pub(crate) fn apply_toplevel_changes(&mut self) {
        for output in self.outputs.iter().map(|o| (o.0.clone(), o.1.name())).collect::<Vec<_>>() {
            for anchor in
                [PanelAnchor::Top, PanelAnchor::Bottom, PanelAnchor::Left, PanelAnchor::Right]
            {
                for s in self.space_list.iter_mut().filter(|s| {
                    s.output.as_ref().is_some_and(|o| o.1.name() == output.1)
                        && s.config.anchor == anchor
                }) {
                    s.minimized_toplevels.clear();
                    for t in &self.toplevels {
                        if !t.output.contains(&output.0) {
                            continue;
                        }

                        if t.state.contains(&zcosmic_toplevel_handle_v1::State::Minimized) {
                            s.minimized_toplevels.insert(t.foreign_toplevel.id());
                        }
                    }
                    s.handle_focus();
                }
            }
        }
    }

    pub(crate) fn maximized_outputs(&self) -> Vec<WlOutput> {
        let outputs = self
            .workspace_groups
            .iter()
            .filter_map(|g| {
                if g.workspaces.iter().any(|w| {
                    if let Some(workspace_info) = self.workspaces.iter().find(|i| i.handle == *w) {
                        workspace_info.state.contains(ext_workspace_handle_v1::State::Active)
                            && self
                                .maximized_toplevels
                                .iter()
                                .any(|info| info.workspace.contains(w))
                    } else {
                        false
                    }
                }) {
                    Some(g.outputs.clone())
                } else {
                    None
                }
            })
            .flatten();

        let sticky_outputs: HashSet<WlOutput> = self
            .maximized_toplevels
            .iter()
            .filter_map(|t| {
                if t.state.contains(&zcosmic_toplevel_handle_v1::State::Maximized)
                    && t.state.contains(&zcosmic_toplevel_handle_v1::State::Sticky)
                {
                    Some(t.output.clone().into_iter())
                } else {
                    None
                }
            })
            .flatten()
            .collect();
        outputs.chain(sticky_outputs.into_iter()).collect()
    }
}
