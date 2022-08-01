// SPDX-License-Identifier: MPL-2.0-only

use std::{cell::RefCell, rc::Rc, time::Instant};

use crate::space::PanelSpace;
use cosmic_panel_config::{CosmicPanelContainerConfig, CosmicPanelOuput};
use itertools::Itertools;
use sctk::reexports::client::protocol::wl_surface as c_wl_surface;
use smithay::{
    desktop::PopupManager,
    reexports::wayland_server::{self, protocol::wl_surface, Resource},
    wayland::output::Output,
};
use xdg_shell_wrapper::{
    client_state::ClientFocus, server_state::ServerPointerFocus, space::WrapperSpace,
};

use super::SpaceContainer;

impl WrapperSpace for SpaceContainer {
    type Config = CosmicPanelContainerConfig;

    fn setup(
        &mut self,
        display: wayland_server::DisplayHandle,
        env: &sctk::environment::Environment<xdg_shell_wrapper::client_state::Env>,
        c_display: sctk::reexports::client::Display,
        c_focused_surface: Rc<RefCell<ClientFocus>>,
        c_hovered_surface: Rc<RefCell<ClientFocus>>,
    ) {
        // create a space for each config profile which is configured for Active output and call setup on each
        self.space_list = self
            .config
            .config_list
            .iter()
            .filter_map(|config| {
                if matches!(config.output, CosmicPanelOuput::Active) {
                    let mut s = PanelSpace::new(config.clone(), self.log.clone());
                    s.setup(
                        display.clone(),
                        env,
                        c_display.clone(),
                        c_focused_surface.clone(),
                        c_hovered_surface.clone(),
                    );
                    let _ = s.handle_output(display.clone(), env, None, None, None);
                    Some(s)
                } else {
                    None
                }
            })
            .collect_vec();
        self.c_display.replace(c_display);
        self.c_focused_surface = c_focused_surface;
        self.c_hovered_surface = c_hovered_surface;
    }

    fn handle_output(
        &mut self,
        display: wayland_server::DisplayHandle,
        env: &sctk::environment::Environment<xdg_shell_wrapper::client_state::Env>,
        c_output: Option<sctk::reexports::client::protocol::wl_output::WlOutput>,
        s_output: Option<Output>,
        output_info: Option<&sctk::output::OutputInfo>,
    ) -> anyhow::Result<()> {
        let c_output = match c_output {
            Some(o) => o,
            None => return Ok(()), // already created and set up
        };

        let s_output = match s_output {
            Some(o) => o,
            None => return Ok(()), // already created and set up
        };

        let output_info = match output_info {
            Some(o) => o,
            None => return Ok(()), // already created and set up
        };

        let c_display = self.c_display.as_ref().unwrap().clone();
        let c_focused_surface = &self.c_focused_surface;
        let c_hovered_surface = &self.c_hovered_surface;

        // TODO error handling
        // create the spaces that are configured to use this output, including spaces configured for All
        let mut new_spaces = self
            .config
            .config_list
            .iter()
            .filter_map(|config| match &config.output {
                CosmicPanelOuput::All => {
                    let mut config = config.clone();
                    config.output = CosmicPanelOuput::Name(output_info.name.clone());
                    let mut s = PanelSpace::new(config.clone(), self.log.clone());
                    s.setup(
                        display.clone(),
                        env,
                        c_display.clone(),
                        c_focused_surface.clone(),
                        c_hovered_surface.clone(),
                    );
                    let _ = s.handle_output(
                        display.clone(),
                        env,
                        Some(c_output.clone()),
                        Some(s_output.clone()),
                        Some(output_info),
                    );
                    Some(s)
                }
                CosmicPanelOuput::Name(name) if name == &output_info.name => {
                    let mut s = PanelSpace::new(config.clone(), self.log.clone());
                    s.setup(
                        display.clone(),
                        env,
                        c_display.clone(),
                        c_focused_surface.clone(),
                        c_hovered_surface.clone(),
                    );
                    let _ = s.handle_output(
                        display.clone(),
                        env,
                        Some(c_output.clone()),
                        Some(s_output.clone()),
                        Some(output_info),
                    );
                    Some(s)
                }
                _ => None,
            })
            .collect_vec();
        self.space_list.append(&mut new_spaces);
        // add output to space
        for s in &mut self.space_list {
            s.space.map_output(&s_output, output_info.location);
        }

        Ok(())
    }

    fn add_window(&mut self, s_top_level: smithay::desktop::Window) {
        // add window to the space with a client that matches the window
        let w_client = s_top_level.toplevel().wl_surface().client_id();

        if let Some(space) = self.space_list.iter_mut().find(|space| {
            space
                .clients_center
                .iter()
                .chain(space.clients_left.iter())
                .chain(space.clients_right.iter())
                .find(|c| Some(c.id()) == w_client)
                .is_some()
        }) {
            space.add_window(s_top_level);
        }
    }

    fn add_popup(
        &mut self,
        env: &sctk::environment::Environment<xdg_shell_wrapper::client_state::Env>,
        xdg_surface_state: &sctk::reexports::client::Attached<
            sctk::reexports::protocols::xdg_shell::client::xdg_wm_base::XdgWmBase,
        >,
        s_surface: smithay::wayland::shell::xdg::PopupSurface,
        positioner: sctk::reexports::client::Main<
            sctk::reexports::protocols::xdg_shell::client::xdg_positioner::XdgPositioner,
        >,
        positioner_state: smithay::wayland::shell::xdg::PositionerState,
    ) {
        // add popup to the space with a client that matches the window
        let p_client = s_surface.wl_surface().client_id();

        if let Some(space) = self.space_list.iter_mut().find(|space| {
            space
                .clients_center
                .iter()
                .chain(space.clients_left.iter())
                .chain(space.clients_right.iter())
                .find(|c| Some(c.id()) == p_client)
                .is_some()
        }) {
            space.add_popup(
                env,
                xdg_surface_state,
                s_surface,
                positioner,
                positioner_state,
            );
        }
    }

    fn reposition_popup(
        &mut self,
        popup: smithay::wayland::shell::xdg::PopupSurface,
        positioner: sctk::reexports::client::Main<
            sctk::reexports::protocols::xdg_shell::client::xdg_positioner::XdgPositioner,
        >,
        positioner_state: smithay::wayland::shell::xdg::PositionerState,
        token: u32,
    ) -> anyhow::Result<()> {
        // add popup to the space with a client that matches the window
        let p_client = popup.wl_surface().client_id();

        if let Some(space) = self.space_list.iter_mut().find(|space| {
            space
                .clients_center
                .iter()
                .chain(space.clients_left.iter())
                .chain(space.clients_right.iter())
                .find(|c| Some(c.id()) == p_client)
                .is_some()
        }) {
            space.reposition_popup(popup, positioner, positioner_state, token)?
        }
        anyhow::bail!("Failed to find popup with matching client id")
    }

    fn handle_events(
        &mut self,
        dh: &smithay::reexports::wayland_server::DisplayHandle,
        popup_manager: &mut PopupManager,
        time: u32,
    ) -> std::time::Instant {
        self.space_list
            .iter_mut()
            .fold(None, |acc, s| {
                let last_dirtied = s.handle_events(dh, popup_manager, time, &mut self.renderer);
                acc.map(|i| last_dirtied.max(i))
            })
            .unwrap_or_else(|| Instant::now())
    }

    fn config(&self) -> Self::Config {
        self.config.clone()
    }

    fn spawn_clients(
        &mut self,
        display: smithay::reexports::wayland_server::DisplayHandle,
    ) -> anyhow::Result<Vec<std::os::unix::net::UnixStream>> {
        Ok(self
            .space_list
            .iter_mut()
            .map(|space| {
                // TODO better error handling
                space.spawn_clients(display.clone()).unwrap_or_default()
            })
            .flatten()
            .collect())
    }

    fn log(&self) -> Option<slog::Logger> {
        Some(self.log.clone())
    }

    fn destroy(&mut self) {
        for s in &mut self.space_list {
            s.destroy();
        }
    }

    fn dirty_window(
        &mut self,
        dh: &smithay::reexports::wayland_server::DisplayHandle,
        w: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    ) {
        // add window to the space with a client that matches the window
        let w_client = w.client_id();

        if let Some(space) = self.space_list.iter_mut().find(|space| {
            space
                .clients_center
                .iter()
                .chain(space.clients_left.iter())
                .chain(space.clients_right.iter())
                .find(|c| Some(c.id()) == w_client)
                .is_some()
        }) {
            space.dirty_window(dh, w);
        }
    }

    fn dirty_popup(
        &mut self,
        dh: &smithay::reexports::wayland_server::DisplayHandle,
        w: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    ) {
        // add window to the space with a client that matches the window
        let p_client = w.client_id();

        if let Some(space) = self.space_list.iter_mut().find(|space| {
            space
                .clients_center
                .iter()
                .chain(space.clients_left.iter())
                .chain(space.clients_right.iter())
                .find(|c| Some(c.id()) == p_client)
                .is_some()
        }) {
            space.dirty_popup(dh, w);
        }
    }

    fn renderer(&mut self) -> Option<&mut smithay::backend::renderer::gles2::Gles2Renderer> {
        self.renderer.as_mut()
    }

    // FIXME
    // all pointer / keyboard handling should be called on the active space first, then on the rest
    // Eg: likely opening a popup on one panel, then without clicking anywhere else, opening a popup on another panel will crash
    fn update_pointer(
        &mut self,
        dim: (i32, i32),
        seat_name: &str,
        c_wl_surface: c_wl_surface::WlSurface,
    ) -> Option<ServerPointerFocus> {
        self.space_list
            .iter_mut()
            .find_map(|s| s.update_pointer(dim, seat_name, c_wl_surface.clone()))
    }

    fn handle_press(&mut self, seat_name: &str) -> Option<wl_surface::WlSurface> {
        self.space_list
            .iter_mut()
            .find_map(|s| s.handle_press(seat_name))
    }

    fn keyboard_leave(&mut self, seat_name: &str, surface: Option<c_wl_surface::WlSurface>) {
        for s in &mut self.space_list {
            s.keyboard_leave(seat_name, surface.clone());
        }
    }

    fn keyboard_enter(
        &mut self,
        seat_name: &str,
        surface: c_wl_surface::WlSurface,
    ) -> Option<wl_surface::WlSurface> {
        self.space_list
            .iter_mut()
            .find_map(|s| s.keyboard_enter(seat_name, surface.clone()))
    }

    fn pointer_leave(&mut self, seat_name: &str, surface: Option<c_wl_surface::WlSurface>) {
        for s in &mut self.space_list {
            s.pointer_leave(seat_name, surface.clone());
        }
    }

    fn pointer_enter(
        &mut self,
        dim: (i32, i32),
        seat_name: &str,
        c_wl_surface: c_wl_surface::WlSurface,
    ) -> Option<ServerPointerFocus> {
        self.space_list
            .iter_mut()
            .find_map(|s| s.pointer_enter(dim, seat_name, c_wl_surface.clone()))
    }
}
