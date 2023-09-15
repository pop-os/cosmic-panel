use cctk::{
    cosmic_protocols::toplevel_info::v1::client::zcosmic_toplevel_handle_v1,
    toplevel_info::ToplevelInfo, wayland_client::Connection,
};
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
        let state = state(info);
        self.apply_toplevel_changes();

        let is_maximized = matches!(state, Some(zcosmic_toplevel_handle_v1::State::Maximized));
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

        let is_maximized = matches!(
            state(info),
            Some(zcosmic_toplevel_handle_v1::State::Maximized)
        );

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

        let len = self.maximized_toplevels.len();
        self.maximized_toplevels.retain(|(t, _)| t != toplevel);
        if self.maximized_toplevels.len() != len {
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
        let Some(output) = info.output.as_ref() else {
            return;
        };
        let Some(config_name) = self.space_list.iter().find_map(|s| {
            if s.output.as_ref().iter().any(|(o, _, _)| o == output) {
                Some(s.config.name.clone())
            } else {
                None
            }
        }) else {
            return;
        };
        let Some(config) = self.config.config_list.iter().find(|c| c.name == config_name) else {
            return;
        };
        _ = self
            .panel_tx
            .send(crate::PanelCalloopMsg::RestartSpace(config.clone()));
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
        let Some(output) = info.output.as_ref() else {
            return;
        };
        let Some(config_name) = self.space_list.iter().find_map(|s| {
            if s.output.as_ref().iter().any(|(o, _, _)| o == output) {
                Some(s.config.name.clone())
            } else {
                None
            }
        }) else {
            return;
        };
        let Some(config) = self.config.config_list.iter().find(|c| c.name == config_name) else {
            return;
        };
        _ = self
            .panel_tx
            .send(crate::PanelCalloopMsg::RestartSpace(config.clone()));
    }

    pub(crate) fn apply_toplevel_changes(&mut self) {
        for output in &self.outputs {
            let has_toplevel = self.toplevels.iter().any(|(_, info)| {
                info.output.as_ref() == Some(&output.0)
                    && !matches!(
                        state(info),
                        Some(zcosmic_toplevel_handle_v1::State::Minimized)
                    )
            });
            for s in &mut self.space_list {
                if s.output.as_ref().map(|o| &o.0) == Some(&output.0) {
                    s.output_has_toplevel = has_toplevel;
                }
            }
        }
    }
}

pub(crate) fn state(info: &ToplevelInfo) -> Option<zcosmic_toplevel_handle_v1::State> {
    if info.state.len() < 4 {
        return None;
    }
    let Some(state_arr) = info.state.chunks_exact(4).next() else {
        return None;
    };
    let Some(state) = zcosmic_toplevel_handle_v1::State::try_from(u32::from_ne_bytes(state_arr[0..4].try_into().unwrap())).ok() else {
        return None;
    };
    Some(state)
}
