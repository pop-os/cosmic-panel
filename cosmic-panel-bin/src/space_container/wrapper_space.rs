// SPDX-License-Identifier: MPL-2.0-only

use cosmic_panel_config::CosmicPanelContainerConfig;
use xdg_shell_wrapper::space::WrapperSpace;

use super::SpaceContainer;

impl WrapperSpace for SpaceContainer {
    type Config = CosmicPanelContainerConfig;

    fn setup(
        &mut self,
        env: &sctk::environment::Environment<xdg_shell_wrapper::client_state::Env>,
        c_display: sctk::reexports::client::Display,
        log: slog::Logger,
        focused_surface: std::rc::Rc<std::cell::RefCell<Option<smithay::reexports::wayland_server::protocol::wl_surface::WlSurface>>>,
    ) {
        // create a space for each config profile and call setup on each
        todo!()
    }

    fn handle_output(
        &mut self,
        env: &sctk::environment::Environment<xdg_shell_wrapper::client_state::Env>,
        output: Option<&sctk::reexports::client::protocol::wl_output::WlOutput>,
        output_info: Option<&sctk::output::OutputInfo>,
    ) -> anyhow::Result<()> {
        // call handle output for the PanelSpace which is configured for this output
        todo!()
    }

    fn update_pointer(&mut self, dim: (i32, i32)) {
        // update pointer for the active space
        todo!()
    }

    fn handle_button(&mut self, c_focused_surface: &sctk::reexports::client::protocol::wl_surface::WlSurface) -> bool {
        // handle button for the active space
        todo!()
    }

    fn add_window(&mut self, s_top_level: smithay::desktop::Window) {
        // add window to the space with a client that matches the window
        todo!()
    }

    fn add_popup(
        &mut self,
        env: &sctk::environment::Environment<xdg_shell_wrapper::client_state::Env>,
        xdg_surface_state: &sctk::reexports::client::Attached<sctk::reexports::protocols::xdg_shell::client::xdg_wm_base::XdgWmBase>,
        s_surface: smithay::wayland::shell::xdg::PopupSurface,
        positioner: sctk::reexports::client::Main<sctk::reexports::protocols::xdg_shell::client::xdg_positioner::XdgPositioner>,
        positioner_state: smithay::wayland::shell::xdg::PositionerState,
    ) {
        // add popup to the space with a client that matches the window
        todo!()
    }

    fn keyboard_focus_lost(&mut self) {
        // take keyboard focus from the active space
        todo!()
    }

    fn reposition_popup(
        &mut self,
        popup: smithay::wayland::shell::xdg::PopupSurface,
        positioner: sctk::reexports::client::Main<sctk::reexports::protocols::xdg_shell::client::xdg_positioner::XdgPositioner>,
        positioner_state: smithay::wayland::shell::xdg::PositionerState,
        token: u32,
    ) -> anyhow::Result<()> {
        todo!()
    }

    fn handle_events(&mut self, dh: &smithay::reexports::wayland_server::DisplayHandle, time: u32, focus: &xdg_shell_wrapper::client_state::Focus) -> std::time::Instant {
        todo!()
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

    fn dirty_window(&mut self, dh: &smithay::reexports::wayland_server::DisplayHandle, w: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface) {
        todo!()
    }

    fn dirty_popup(&mut self, dh: &smithay::reexports::wayland_server::DisplayHandle, w: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface) {
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
}
