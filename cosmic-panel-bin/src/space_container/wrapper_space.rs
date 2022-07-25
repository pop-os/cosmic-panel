// SPDX-License-Identifier: MPL-2.0-only

use std::time::Instant;

use cosmic_panel_config::{CosmicPanelContainerConfig, CosmicPanelOuput};
use itertools::Itertools;
use smithay::reexports::wayland_server::protocol::wl_surface;
use xdg_shell_wrapper::{space::WrapperSpace, client_state::ClientFocus, server_state::{ServerFocus, ServerPointerFocus}};
use sctk::reexports::client::protocol::{wl_surface as c_wl_surface};
use crate::space::PanelSpace;

use super::SpaceContainer;

impl WrapperSpace for SpaceContainer {
    type Config = CosmicPanelContainerConfig;

    fn setup(
        &mut self,
        env: &sctk::environment::Environment<xdg_shell_wrapper::client_state::Env>,
        c_display: sctk::reexports::client::Display,
        c_focused_surface: ClientFocus,
        c_hovered_surface: ClientFocus,
    ) {
        // create a space for each config profile which is configured for Active output and call setup on each
        self.space_list = self.config.config_list.iter().filter_map(|config| {
            if matches!(config.output, CosmicPanelOuput::Active) {
                let mut s = PanelSpace::new(config.clone(), self.log.clone());
                s.setup(env, c_display.clone(), c_focused_surface.clone(), c_hovered_surface.clone());
                let _ = s.handle_output(env, None, None);
                Some(s)
            } else {
                None
            }
        }).collect_vec();
        self.c_focused_surface = c_focused_surface;
        self.c_hovered_surface = c_hovered_surface;
    }

    fn handle_output(
        &mut self,
        env: &sctk::environment::Environment<xdg_shell_wrapper::client_state::Env>,
        output: Option<&sctk::reexports::client::protocol::wl_output::WlOutput>,
        output_info: Option<&sctk::output::OutputInfo>,
    ) -> anyhow::Result<()> {
        let output = match output {
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
        let mut new_spaces = self.config.config_list.iter().filter_map(|config| {
            match &config.output {
                CosmicPanelOuput::All => {
                    let mut s = PanelSpace::new(config.clone(), self.log.clone());
                    s.setup(env, c_display.clone(), c_focused_surface.clone(), c_hovered_surface.clone());
                    let _ = s.handle_output(env, Some(output), Some(output_info));
                    Some(s)
                },
                CosmicPanelOuput::Name(name) if name == &output_info.name => {
                    let mut s = PanelSpace::new(config.clone(), self.log.clone());
                    s.setup(env, c_display.clone(), c_focused_surface.clone(), c_hovered_surface.clone());
                    let _ = s.handle_output(env, Some(output), Some(output_info));
                    Some(s)
                },
                _ => None,
            }
        }).collect_vec();
        self.space_list.append(&mut new_spaces);
        Ok(())
    }


    fn add_window(&mut self, s_top_level: smithay::desktop::Window) {
        // add window to the space with a client that matches the window
        todo!()
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
        todo!()
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
        todo!()
    }

    fn handle_events(
        &mut self,
        dh: &smithay::reexports::wayland_server::DisplayHandle,
        time: u32,
    ) -> std::time::Instant {
        self.space_list.iter_mut().fold(None, |acc, s| {
            let last_dirtied = s.handle_events(dh, time, &mut self.renderer);
            acc.map(|i| last_dirtied.max(i))
        }).unwrap_or_else(|| Instant::now())
    }

    fn config(&self) -> Self::Config {
        todo!()
    }

    fn spawn_clients(
        &mut self,
        display: &mut smithay::reexports::wayland_server::DisplayHandle,
    ) -> anyhow::Result<Vec<std::os::unix::net::UnixStream>> {
        todo!()
    }

    fn log(&self) -> Option<slog::Logger> {
        todo!()
    }

    fn destroy(&mut self) {
        todo!()
    }

    fn space(&mut self) -> &mut smithay::desktop::Space {
        todo!()
    }

    fn dirty_window(
        &mut self,
        dh: &smithay::reexports::wayland_server::DisplayHandle,
        w: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    ) {
        todo!()
    }

    fn dirty_popup(
        &mut self,
        dh: &smithay::reexports::wayland_server::DisplayHandle,
        w: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    ) {
        todo!()
    }

    fn popup_manager(&mut self) -> &mut smithay::desktop::PopupManager {
        todo!()
    }

    fn popups(&self) -> Vec<&xdg_shell_wrapper::space::Popup> {
        todo!()
    }

    fn renderer(&mut self) -> Option<&mut smithay::backend::renderer::gles2::Gles2Renderer> {
        todo!()
    }

    fn update_pointer(&mut self, dim: (i32, i32), seat_name: &str) -> Option<ServerPointerFocus> {
        todo!()
    }

    fn handle_press(&mut self, seat_name: &str) -> Option<wl_surface::WlSurface> {
        todo!()
    }

    fn keyboard_leave(&mut self, seat_name: &str, surface: Option<c_wl_surface::WlSurface>) {
        todo!()
    }

    fn keyboard_enter(&mut self, seat_name: &str, surface: Option<c_wl_surface::WlSurface>)  -> Option<wl_surface::WlSurface> {
        todo!()
    }

    fn pointer_leave(&mut self, seat_name: &str, surface: Option<c_wl_surface::WlSurface>) {
        todo!()
    }

    fn pointer_enter(&mut self, seat_name: &str, surface: Option<sctk::reexports::client::protocol::wl_surface::WlSurface>) {
        todo!()
    }
    
}
