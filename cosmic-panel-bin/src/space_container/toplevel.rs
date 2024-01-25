use cctk::{
    cosmic_protocols::{toplevel_info::v1::client::zcosmic_toplevel_handle_v1, workspace},
    toplevel_info::ToplevelInfo,
    wayland_client::{protocol::wl_output::WlOutput, Connection},
};
use cosmic_panel_config::AutoHide;
use itertools::max;
use xdg_shell_wrapper::space::ToplevelInfoSpace;

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

        let is_maximized = info
            .state
            .contains(&zcosmic_toplevel_handle_v1::State::Maximized);
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
        self.apply_toplevel_changes();

        let is_maximized = info
            .state
            .contains(&zcosmic_toplevel_handle_v1::State::Maximized);

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

impl SpaceContainer {
    fn add_maximized(
        &mut self,
        toplevel: &zcosmic_toplevel_handle_v1::ZcosmicToplevelHandleV1,
        info: &ToplevelInfo,
    ) {
        self.maximized_toplevels
            .push((toplevel.clone(), info.clone()));
        for output in &info.output {
            self.apply_maximized(output, true);
        }
    }

    fn remove_maximized(&mut self, toplevel: &zcosmic_toplevel_handle_v1::ZcosmicToplevelHandleV1) {
        let (_, info) = if let Some(pos) = self
            .maximized_toplevels
            .iter()
            .position(|(h, _)| h == toplevel)
        {
            self.maximized_toplevels.remove(pos)
        } else {
            return;
        };

        for output in &info.output {
            self.apply_maximized(output, false);
        }
    }

    pub(crate) fn apply_maximized(&mut self, output: &WlOutput, maximized: bool) {
        let mut bg_color = self.cur_bg_color(); // TODO

        for s in self
            .space_list
            .iter_mut()
            .filter(|s| s.output.as_ref().iter().any(|(o, _, _)| o == output))
        {
            let c = self
                .config
                .config_list
                .iter()
                .find(|c| c.name == s.config.name);
            let mut config = s.config.clone();
            if maximized {
                bg_color[3] = 1.0;
                config.anchor_gap = false;
                config.expand_to_edges = true;
                config.autohide = Some(AutoHide::default());
                config.border_radius = 0;
                config.exclusive_zone = false;
            } else {
                if let Some(c) = c {
                    config = c.clone();
                }
                bg_color[3] = config.opacity;
            }

            s.set_maximized(maximized, config, bg_color)
        }
    }

    pub(crate) fn apply_toplevel_changes(&mut self) {
        for output in &self.outputs {
            let has_toplevel = self.toplevels.iter().any(|(_, info)| {
                info.output.contains(&output.0)
                    && !info
                        .state
                        .contains(&zcosmic_toplevel_handle_v1::State::Minimized)
                    && self.workspace_groups.iter().any(|g| {
                        g.workspaces.iter().any(|w| {
                            w.state.contains(&cctk::wayland_client::WEnum::Value(
                                workspace::v1::client::zcosmic_workspace_handle_v1::State::Active,
                            )) && info.workspace.contains(&w.handle)
                        })
                    })
            });
            for s in &mut self.space_list {
                if s.output.as_ref().map(|o| &o.0) == Some(&output.0) {
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
