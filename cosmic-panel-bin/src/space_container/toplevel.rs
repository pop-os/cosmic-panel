use cctk::{
    cosmic_protocols::{
        toplevel_info::v1::client::zcosmic_toplevel_handle_v1,
        toplevel_management::v1::client::zcosmic_toplevel_manager_v1, workspace,
    },
    toplevel_info::ToplevelInfo,
    wayland_client::{protocol::wl_output::WlOutput, Connection},
};

use cosmic_panel_config::{CosmicPanelBackground, PanelAnchor};
use itertools::Itertools;
use sctk::shell::WaylandSurface;
use xdg_shell_wrapper::{
    client_state::FocusStatus,
    space::{ToplevelInfoSpace, ToplevelManagerSpace, Visibility, WrapperSpace},
};

use crate::space::PanelSpace;

use super::SpaceContainer;

impl ToplevelInfoSpace for SpaceContainer {
    /// A new toplevel was created
    fn new_toplevel(
        &mut self,
        _conn: &Connection,
        toplevel: &zcosmic_toplevel_handle_v1::ZcosmicToplevelHandleV1,
        info: &ToplevelInfo,
    ) {
        self.toplevels.push((toplevel.clone(), info.clone()));
        self.apply_toplevel_changes();
        _ = self.panel_tx.send(crate::PanelCalloopMsg::UpdateToplevel(toplevel.clone()));

        let is_maximized = info.state.contains(&zcosmic_toplevel_handle_v1::State::Maximized);
        if is_maximized {
            self.add_maximized(toplevel, info);
        }
    }

    /// A toplevel was updated
    fn update_toplevel(
        &mut self,
        _conn: &Connection,
        toplevel: &zcosmic_toplevel_handle_v1::ZcosmicToplevelHandleV1,
        info: &ToplevelInfo,
    ) {
        if let Some(info_1) =
            self.toplevels
                .iter_mut()
                .find_map(|(t, info_1)| if t == toplevel { Some(info_1) } else { None })
        {
            *info_1 = info.clone();
        }
        _ = self.panel_tx.send(crate::PanelCalloopMsg::UpdateToplevel(toplevel.clone()));
        self.apply_toplevel_changes();

        let is_maximized = info.state.contains(&zcosmic_toplevel_handle_v1::State::Maximized);

        let was_maximized = self.maximized_toplevels.iter().any(|(t, _)| t == toplevel);
        if is_maximized && !was_maximized {
            self.add_maximized(toplevel, info);
        } else if !is_maximized && was_maximized {
            self.remove_maximized(toplevel);
        }
    }

    /// A toplevel was closed
    fn toplevel_closed(
        &mut self,
        _conn: &Connection,
        toplevel: &zcosmic_toplevel_handle_v1::ZcosmicToplevelHandleV1,
    ) {
        self.toplevels.retain(|(t, _)| t != toplevel);
        self.apply_toplevel_changes();

        if self.maximized_toplevels.iter().any(|(h, _)| h == toplevel) {
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
    fn add_maximized(
        &mut self,
        toplevel: &zcosmic_toplevel_handle_v1::ZcosmicToplevelHandleV1,
        info: &ToplevelInfo,
    ) {
        self.maximized_toplevels.push((toplevel.clone(), info.clone()));
        for output in &info.output {
            self.apply_maximized(output, true);
        }
    }

    fn remove_maximized(&mut self, toplevel: &zcosmic_toplevel_handle_v1::ZcosmicToplevelHandleV1) {
        let (_, info) =
            if let Some(pos) = self.maximized_toplevels.iter().position(|(h, _)| h == toplevel) {
                self.maximized_toplevels.remove(pos)
            } else {
                return;
            };

        for output in &info.output {
            if !self.maximized_toplevels.iter().any(|(_, info)| info.output.contains(output)) {
                self.apply_maximized(output, false);
            }
        }
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
            let has_toplevel = self.toplevels.iter().any(|(_, info)| {
                info.output.contains(&output.0)
                    && !info.state.contains(&zcosmic_toplevel_handle_v1::State::Minimized)
                    && self.workspace_groups.iter().any(|g| {
                        g.workspaces.iter().any(|w| {
                            w.state.contains(&cctk::wayland_client::WEnum::Value(
                                workspace::v1::client::zcosmic_workspace_handle_v1::State::Active,
                            )) && info.workspace.contains(&w.handle)
                        })
                    })
            });

            let name = output.1;
            for anchor in
                [PanelAnchor::Top, PanelAnchor::Bottom, PanelAnchor::Left, PanelAnchor::Right]
            {
                let mut additional_gap = 0;
                for s in self.stacked_spaces_by_priority(&name, anchor) {
                    s.set_additional_gap(additional_gap);
                    if s.config.autohide.is_some()
                        && (!has_toplevel
                            || s.c_focused_surface.borrow().iter().any(|c| {
                                matches!(c.2, FocusStatus::Focused)
                                    && s.layer.as_ref().is_some_and(|s| s.wl_surface() == &c.0)
                            }))
                    {
                        additional_gap += s.crosswise();
                    }
                    s.handle_focus();

                    s.output_has_toplevel = has_toplevel;
                }
            }
        }
    }

    pub(crate) fn maximized_outputs(&self) -> Vec<WlOutput> {
        self.workspace_groups
            .iter()
            .filter_map(|g| {
                if g.workspaces.iter().any(|w| {
                    w.state.contains(&cctk::wayland_client::WEnum::Value(
                        workspace::v1::client::zcosmic_workspace_handle_v1::State::Active,
                    )) && self
                        .maximized_toplevels
                        .iter()
                        .any(|(_, info)| info.workspace.contains(&w.handle))
                }) {
                    Some(g.outputs.clone())
                } else {
                    None
                }
            })
            .flatten()
            .collect()
    }
}
