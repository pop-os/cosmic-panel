use std::{cell::RefCell, rc::Rc, time::Instant};

use cosmic_panel_config::{CosmicPanelBackground, CosmicPanelContainerConfig, CosmicPanelOuput};
use itertools::Itertools;
use sctk::{
    compositor::CompositorState,
    output::OutputInfo,
    reexports::client::{
        protocol::{wl_output::WlOutput, wl_surface as c_wl_surface},
        Connection, QueueHandle,
    },
    shell::{
        wlr_layer::{LayerShell, LayerSurface, LayerSurfaceConfigure},
        WaylandSurface,
    },
};
use smithay::{
    desktop::PopupManager,
    output::Output,
    reexports::wayland_server::{self, protocol::wl_surface, Resource},
};
use xdg_shell_wrapper::{
    client_state::{ClientFocus, FocusStatus},
    server_state::ServerPointerFocus,
    shared_state::GlobalState,
    space::{Visibility, WrapperSpace},
    wp_fractional_scaling::FractionalScalingManager,
    wp_security_context::SecurityContextManager,
    wp_viewporter::ViewporterState,
};

use crate::space::PanelSpace;

use super::SpaceContainer;

impl WrapperSpace for SpaceContainer {
    type Config = CosmicPanelContainerConfig;

    /// set the display handle of the space
    fn set_display_handle(&mut self, display: wayland_server::DisplayHandle) {
        self.s_display.replace(display);
    }

    /// get the client hovered surface of the space
    fn get_client_hovered_surface(&self) -> Rc<RefCell<ClientFocus>> {
        self.c_hovered_surface.clone()
    }

    /// get the client focused surface of the space
    fn get_client_focused_surface(&self) -> Rc<RefCell<ClientFocus>> {
        self.c_focused_surface.clone()
    }

    /// run after the connection is ready
    fn setup<W: WrapperSpace>(
        &mut self,
        compositor_state: &CompositorState,
        fractional_scale_manager: Option<&FractionalScalingManager<W>>,
        security_context_manager: Option<SecurityContextManager>,
        viewport: Option<&ViewporterState<W>>,
        layer_state: &mut LayerShell,
        conn: &Connection,
        qh: &QueueHandle<GlobalState<W>>,
    ) {
        self.connection = Some(conn.clone());
        self.security_context_manager = security_context_manager.clone();

        // create a space for each config profile which is configured for Active output
        // and call setup on each
        self.space_list.append(
            &mut self
                .config
                .config_list
                .iter()
                .filter_map(|config| {
                    if matches!(config.output, CosmicPanelOuput::Active) {
                        let mut s = PanelSpace::new(
                            config.clone(),
                            self.c_focused_surface.clone(),
                            self.c_hovered_surface.clone(),
                            self.applet_tx.clone(),
                            match config.background {
                                CosmicPanelBackground::ThemeDefault
                                | CosmicPanelBackground::Color(_) => self.cur_theme(),
                                CosmicPanelBackground::Dark => self.dark_theme.clone(),
                                CosmicPanelBackground::Light => self.light_theme.clone(),
                            },
                            self.s_display.clone().unwrap(),
                            self.security_context_manager.clone(),
                            conn,
                            self.panel_tx.clone(),
                            if config.autohide.is_some() {
                                Visibility::Hidden
                            } else {
                                Visibility::Visible
                            },
                            self.loop_handle.clone(),
                        );
                        s.setup(
                            compositor_state,
                            fractional_scale_manager,
                            security_context_manager.clone(),
                            viewport,
                            layer_state,
                            conn,
                            qh,
                        );
                        if let Some(s_display) = self.s_display.as_ref() {
                            s.set_display_handle(s_display.clone());
                        }
                        let _ = s.new_output(
                            compositor_state,
                            fractional_scale_manager,
                            viewport,
                            layer_state,
                            conn,
                            qh,
                            None,
                            None,
                            None,
                        );
                        Some(s)
                    } else {
                        None
                    }
                })
                .collect_vec(),
        );
    }

    fn new_output<W: WrapperSpace>(
        &mut self,
        compositor_state: &sctk::compositor::CompositorState,
        fractional_scale_manager: Option<&FractionalScalingManager<W>>,
        viewport: Option<&ViewporterState<W>>,
        layer_state: &mut LayerShell,
        conn: &sctk::reexports::client::Connection,
        qh: &QueueHandle<GlobalState<W>>,
        c_output: Option<WlOutput>,
        s_output: Option<Output>,
        output_info: Option<OutputInfo>,
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

        let output_name = match output_info.name.clone() {
            Some(n) => n,
            None => anyhow::bail!("Output missing name"),
        };
        self.outputs.push((c_output.clone(), s_output.clone(), output_info.clone()));

        let cur = self.cur_theme();
        let dark = self.dark_theme.clone();
        let light = self.light_theme.clone();
        // TODO error handling
        // create the spaces that are configured to use this output, including spaces
        // configured for All
        let mut new_spaces = self
            .config
            .configs_for_output(&output_name)
            .into_iter()
            .filter_map(|config| {
                let visible = if config.autohide.is_some() {
                    Visibility::Hidden
                } else {
                    Visibility::Visible
                };
                match &config.output {
                    CosmicPanelOuput::All => {
                        let c = match config.background {
                            CosmicPanelBackground::ThemeDefault
                            | CosmicPanelBackground::Color(_) => cur.clone(),
                            CosmicPanelBackground::Dark => dark.clone(),
                            CosmicPanelBackground::Light => light.clone(),
                        };
                        let mut s = if let Some(s) = self.space_list.iter_mut().position(|s| {
                            s.config.name == config.name
                                && Some(&c_output) == s.output.as_ref().map(|o| &o.0)
                        }) {
                            self.space_list.remove(s)
                        } else {
                            let mut s = PanelSpace::new(
                                config.clone(),
                                self.c_focused_surface.clone(),
                                self.c_hovered_surface.clone(),
                                self.applet_tx.clone(),
                                c,
                                self.s_display.clone().unwrap(),
                                self.security_context_manager.clone(),
                                conn,
                                self.panel_tx.clone(),
                                visible,
                                self.loop_handle.clone(),
                            );
                            s.setup(
                                compositor_state,
                                fractional_scale_manager,
                                self.security_context_manager.clone(),
                                viewport,
                                layer_state,
                                conn,
                                qh,
                            );
                            if let Some(s_display) = self.s_display.as_ref() {
                                s.set_display_handle(s_display.clone());
                            }
                            s
                        };

                        if s.new_output(
                            compositor_state,
                            fractional_scale_manager,
                            viewport,
                            layer_state,
                            conn,
                            qh,
                            Some(c_output.clone()),
                            Some(s_output.clone()),
                            Some(output_info.clone()),
                        )
                        .is_ok()
                        {
                            Some(s)
                        } else {
                            None
                        }
                    },
                    CosmicPanelOuput::Name(name) if name == &output_name => {
                        let mut s = if let Some(s) = self.space_list.iter_mut().position(|s| {
                            s.config.name == config.name && config.output == s.config.output
                        }) {
                            self.space_list.remove(s)
                        } else {
                            let mut s = PanelSpace::new(
                                config.clone(),
                                self.c_focused_surface.clone(),
                                self.c_hovered_surface.clone(),
                                self.applet_tx.clone(),
                                match config.background {
                                    CosmicPanelBackground::ThemeDefault
                                    | CosmicPanelBackground::Color(_) => cur.clone(),
                                    CosmicPanelBackground::Dark => dark.clone(),
                                    CosmicPanelBackground::Light => light.clone(),
                                },
                                self.s_display.clone().unwrap(),
                                self.security_context_manager.clone(),
                                conn,
                                self.panel_tx.clone(),
                                visible,
                                self.loop_handle.clone(),
                            );

                            if let Some(s_display) = self.s_display.as_ref() {
                                s.set_display_handle(s_display.clone());
                            }
                            s
                        };
                        if s.new_output(
                            compositor_state,
                            fractional_scale_manager,
                            viewport,
                            layer_state,
                            conn,
                            qh,
                            Some(c_output.clone()),
                            Some(s_output.clone()),
                            Some(output_info.clone()),
                        )
                        .is_ok()
                        {
                            Some(s)
                        } else {
                            None
                        }
                    },
                    _ => None,
                }
            })
            .collect_vec();
        self.space_list.append(&mut new_spaces);
        // add output to space
        for s in &mut self.space_list {
            s.space.map_output(&s_output, output_info.location);
        }
        if self.maximized_outputs().iter().any(|o| o == &c_output) {
            self.apply_maximized(&c_output, true);
        }
        self.apply_toplevel_changes();

        Ok(())
    }

    fn add_window(&mut self, s_top_level: smithay::desktop::Window) {
        // add window to the space with a client that matches the window
        let w_client = s_top_level.toplevel().and_then(|t| t.wl_surface().client().map(|c| c.id()));

        if let Some(space) = self.space_list.iter_mut().find(|space| {
            space
                .clients_center
                .lock()
                .unwrap()
                .iter()
                .chain(space.clients_left.lock().unwrap().iter())
                .chain(space.clients_right.lock().unwrap().iter())
                .any(|c| Some(c.client.id()) == w_client)
        }) {
            space.add_window(s_top_level);
        }
    }

    fn add_popup<W: WrapperSpace>(
        &mut self,
        compositor_state: &CompositorState,
        fractional_scale_manager: Option<&FractionalScalingManager<W>>,
        viewport: Option<&ViewporterState<W>>,
        conn: &Connection,
        qh: &QueueHandle<GlobalState<W>>,
        xdg_shell_state: &mut sctk::shell::xdg::XdgShell,
        s_surface: smithay::wayland::shell::xdg::PopupSurface,
        positioner: sctk::shell::xdg::XdgPositioner,
        positioner_state: smithay::wayland::shell::xdg::PositionerState,
    ) -> anyhow::Result<()> {
        // add popup to the space with a client that matches the window
        let p_client = s_surface.wl_surface().client().map(|c| c.id());

        if let Some(space) = self.space_list.iter_mut().find(|space| {
            space
                .clients_center
                .lock()
                .unwrap()
                .iter()
                .chain(space.clients_left.lock().unwrap().iter())
                .chain(space.clients_right.lock().unwrap().iter())
                .any(|c| Some(c.client.id()) == p_client)
        }) {
            space.add_popup(
                compositor_state,
                fractional_scale_manager,
                viewport,
                conn,
                qh,
                xdg_shell_state,
                s_surface,
                positioner,
                positioner_state,
            )
        } else {
            anyhow::bail!("failed to find a matching panel space for this popup.")
        }
    }

    fn reposition_popup(
        &mut self,
        popup: smithay::wayland::shell::xdg::PopupSurface,
        positioner_state: smithay::wayland::shell::xdg::PositionerState,
        token: u32,
    ) -> anyhow::Result<()> {
        // add popup to the space with a client that matches the window
        let p_client = popup.wl_surface().client().map(|c| c.id());

        if let Some(space) = self.space_list.iter_mut().find(|space| {
            space
                .clients_center
                .lock()
                .unwrap()
                .iter()
                .chain(space.clients_left.lock().unwrap().iter())
                .chain(space.clients_right.lock().unwrap().iter())
                .any(|c| Some(c.client.id()) == p_client)
        }) {
            space.reposition_popup(popup, positioner_state, token)?
        }
        anyhow::bail!("Failed to find popup with matching client id")
    }

    fn handle_events<W: WrapperSpace>(
        &mut self,
        dh: &smithay::reexports::wayland_server::DisplayHandle,
        qh: &QueueHandle<GlobalState<W>>,
        popup_manager: &mut PopupManager,
        time: u32,
    ) -> std::time::Instant {
        self.space_list
            .iter_mut()
            .fold(None, |mut acc, s| {
                let last_dirtied =
                    s.handle_events(dh, popup_manager, time, self.renderer.as_mut(), qh);
                if let Some(last_dirty) = acc {
                    if last_dirty < last_dirtied {
                        acc = Some(last_dirtied);
                    }
                } else {
                    acc = Some(last_dirtied);
                }
                acc
            })
            .unwrap_or_else(Instant::now)
    }

    fn config(&self) -> Self::Config {
        self.config.clone()
    }

    fn spawn_clients<W: WrapperSpace>(
        &mut self,
        _display: smithay::reexports::wayland_server::DisplayHandle,
        _qh: &QueueHandle<GlobalState<W>>,
        _: Option<SecurityContextManager>,
    ) -> anyhow::Result<()> {
        // spaces spawn their clients when they are created
        Ok(())
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
        let w_client = w.client().map(|c| c.id());

        if let Some(space) = self.space_list.iter_mut().find(|space| {
            space
                .clients_center
                .lock()
                .unwrap()
                .iter()
                .chain(space.clients_left.lock().unwrap().iter())
                .chain(space.clients_right.lock().unwrap().iter())
                .any(|c| Some(c.client.id()) == w_client)
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
        let p_client = w.client().map(|c| c.id());

        if let Some(space) = self.space_list.iter_mut().find(|space| {
            space
                .clients_center
                .lock()
                .unwrap()
                .iter()
                .chain(space.clients_left.lock().unwrap().iter())
                .chain(space.clients_right.lock().unwrap().iter())
                .any(|c| Some(c.client.id()) == p_client)
        }) {
            space.dirty_popup(dh, w);
        }
    }

    fn renderer(&mut self) -> Option<&mut smithay::backend::renderer::gles::GlesRenderer> {
        self.renderer.as_mut()
    }

    // all pointer / keyboard handling should be called on any space with an active
    // popup first, then on the rest Eg: likely opening a popup on one panel,
    // then without clicking anywhere else, opening a popup on another panel will
    // crash
    fn update_pointer(
        &mut self,
        dim: (i32, i32),
        seat_name: &str,
        c_wl_surface: c_wl_surface::WlSurface,
    ) -> Option<ServerPointerFocus> {
        let mut anchor_output = None;
        let ret = if let Some((popup_space_i, popup_space)) =
            self.space_list.iter_mut().enumerate().find(|s| !s.1.popups.is_empty())
        {
            if let Some(p_ret) = popup_space.update_pointer(dim, seat_name, c_wl_surface.clone()) {
                anchor_output = Some((
                    popup_space.config.anchor,
                    popup_space.output.as_ref().map(|o| o.1.name()),
                ));
                Some(p_ret)
            } else {
                self.space_list.iter_mut().enumerate().find_map(|(i, s)| {
                    if i != popup_space_i {
                        let ret = s.update_pointer(dim, seat_name, c_wl_surface.clone());
                        if ret.is_some() {
                            anchor_output =
                                Some((s.config.anchor, s.output.as_ref().map(|o| o.1.name())));
                        }
                        ret
                    } else {
                        None
                    }
                })
            }
        } else {
            self.space_list.iter_mut().find_map(|s| {
                let ret = s.update_pointer(dim, seat_name, c_wl_surface.clone());
                if ret.is_some() {
                    anchor_output = Some((s.config.anchor, s.output.as_ref().map(|o| o.1.name())));
                }
                ret
            })
        };
        if let Some((anchor, output)) = anchor_output {
            // set the pointer focus for any other space with the same anchor
            // and autohide
            let mut additional_gap = 0;
            let Some(output) = output else {
                return ret;
            };
            let stacked = self.stacked_spaces_by_priority(&output, anchor);
            for s in stacked {
                let Some(space_c_wl_surface) = s.layer.as_ref().map(|l| l.wl_surface()) else {
                    continue;
                };
                if s.config.autohide.is_none() {
                    s.set_additional_gap(additional_gap);
                    continue;
                }
                let hovered = s.c_hovered_surface.clone();
                let mut guard = hovered.borrow_mut();
                if let Some(f) =
                    guard.iter_mut().find(|f| space_c_wl_surface == &f.0 && f.1 == seat_name)
                {
                    f.2 = FocusStatus::Focused;
                } else {
                    guard.push((
                        space_c_wl_surface.clone(),
                        seat_name.to_string(),
                        FocusStatus::Focused,
                    ));
                }
                if s.visibility == Visibility::Visible {
                    s.set_additional_gap(additional_gap);
                } else {
                    s.additional_gap = additional_gap;
                }
                additional_gap += s.crosswise();
            }
        }

        ret
    }

    fn handle_button(&mut self, seat_name: &str, press: bool) -> Option<wl_surface::WlSurface> {
        if let Some((popup_space_i, popup_space)) =
            self.space_list.iter_mut().enumerate().find(|(_, s)| !s.popups.is_empty())
        {
            if let Some(p_ret) = popup_space.handle_button(seat_name, press) {
                Some(p_ret)
            } else {
                self.space_list.iter_mut().enumerate().find_map(|(i, s)| {
                    if i != popup_space_i {
                        s.handle_button(seat_name, press)
                    } else {
                        None
                    }
                })
            }
        } else {
            self.space_list.iter_mut().find_map(|s| s.handle_button(seat_name, press))
        }
    }

    fn keyboard_leave(&mut self, seat_name: &str, surface: Option<c_wl_surface::WlSurface>) {
        if let Some((popup_space_i, popup_space)) =
            self.space_list.iter_mut().enumerate().find(|(_, s)| !s.popups.is_empty())
        {
            popup_space.keyboard_leave(seat_name, surface.clone());
            for (i, s) in &mut self.space_list.iter_mut().enumerate() {
                if i != popup_space_i {
                    s.keyboard_leave(seat_name, surface.clone())
                };
            }
        } else {
            for s in &mut self.space_list {
                s.keyboard_leave(seat_name, surface.clone());
            }
        }
    }

    fn keyboard_enter(
        &mut self,
        seat_name: &str,
        surface: c_wl_surface::WlSurface,
    ) -> Option<wl_surface::WlSurface> {
        if let Some((popup_space_i, popup_space)) =
            self.space_list.iter_mut().enumerate().find(|(_, s)| !s.popups.is_empty())
        {
            if let Some(p_ret) = popup_space.keyboard_enter(seat_name, surface.clone()) {
                Some(p_ret);
            }
            self.space_list.iter_mut().enumerate().find_map(|(i, s)| {
                if i != popup_space_i {
                    s.keyboard_enter(seat_name, surface.clone())
                } else {
                    None
                }
            })
        } else {
            self.space_list.iter_mut().find_map(|s| s.keyboard_enter(seat_name, surface.clone()))
        }
    }

    fn pointer_leave(&mut self, seat_name: &str, surface: Option<c_wl_surface::WlSurface>) {
        let mut output_anchor = None;
        if let Some((popup_space_i, popup_space)) =
            self.space_list.iter_mut().enumerate().find(|(_, s)| !s.popups.is_empty())
        {
            popup_space.pointer_leave(seat_name, surface.clone());
            output_anchor =
                popup_space.output.as_ref().map(|o| (o.1.name(), popup_space.config.anchor));
            for (i, s) in &mut self.space_list.iter_mut().enumerate() {
                if i != popup_space_i {
                    s.pointer_leave(seat_name, None)
                };
            }
        } else if let Some(space) = self.space_list.iter_mut().find(|s| {
            surface
                .as_ref()
                .zip(s.layer.as_ref().map(|l| l.wl_surface()))
                .is_some_and(|(s, l)| s == l)
        }) {
            output_anchor = space.output.as_ref().map(|o| (o.1.name(), space.config.anchor));
            for s in &mut self.space_list {
                s.pointer_leave(seat_name, None);
            }
        } else {
            for s in &mut self.space_list {
                s.pointer_leave(seat_name, None);
            }
        }
        let Some(output_anchor) = output_anchor else {
            return;
        };
        for s in self.stacked_spaces_by_priority(output_anchor.0.as_str(), output_anchor.1) {
            s.pointer_leave(seat_name, surface.clone());
            for f in s.c_hovered_surface.borrow_mut().iter_mut() {
                if f.1 == seat_name {
                    f.2 = FocusStatus::LastFocused(Instant::now());
                }
            }
            if s.config.autohide.is_none() {
                s.set_additional_gap(0);
            }
        }
    }

    fn pointer_enter(
        &mut self,
        dim: (i32, i32),
        seat_name: &str,
        c_wl_surface: c_wl_surface::WlSurface,
    ) -> Option<ServerPointerFocus> {
        if let Some((popup_space_i, popup_space)) =
            self.space_list.iter_mut().enumerate().find(|(_, s)| !s.popups.is_empty())
        {
            if let Some(p_ret) = popup_space.pointer_enter(dim, seat_name, c_wl_surface.clone()) {
                Some(p_ret)
            } else {
                self.space_list.iter_mut().enumerate().find_map(|(i, s)| {
                    if i != popup_space_i {
                        s.pointer_enter(dim, seat_name, c_wl_surface.clone())
                    } else {
                        None
                    }
                })
            }
        } else {
            self.space_list
                .iter_mut()
                .find_map(|s| s.pointer_enter(dim, seat_name, c_wl_surface.clone()))
        }
    }

    fn configure_popup(
        &mut self,
        popup: &sctk::shell::xdg::popup::Popup,
        config: sctk::shell::xdg::popup::PopupConfigure,
    ) {
        if let Some(space) = self
            .space_list
            .iter_mut()
            .find(|s| s.popups.iter().any(|p| p.c_popup.wl_surface() == popup.wl_surface()))
        {
            space.configure_panel_popup(popup, config, self.renderer.as_mut());
        }
    }

    fn visibility(&self) -> Visibility {
        let visible = self.space_list.iter().any(|s| {
            self.c_hovered_surface
                .borrow()
                .iter()
                .any(|f| matches!(f.2, FocusStatus::Focused))
                // transitions should try to be smooth
                || !matches!(s.visibility, Visibility::Visible | Visibility::Hidden)
                || s.animate_state.is_some()
                || !s.popups.is_empty()
        });

        if visible {
            Visibility::Visible
        } else {
            Visibility::Hidden
        }
    }

    fn raise_window(&mut self, _: &smithay::desktop::Window, _: bool) {}

    fn close_popup(&mut self, popup: &sctk::shell::xdg::popup::Popup) {
        if let Some(space) = self
            .space_list
            .iter_mut()
            .find(|s| s.popups.iter().any(|p| p.c_popup.wl_surface() == popup.wl_surface()))
        {
            space.close_popup(popup);
        }
    }

    fn configure_layer(&mut self, layer: &LayerSurface, configure: LayerSurfaceConfigure) {
        if let Some(space) = self
            .space_list
            .iter_mut()
            .find(|s| s.layer.as_ref().map(|s| s.wl_surface()) == Some(layer.wl_surface()))
        {
            space.configure_panel_layer(layer, configure, &mut self.renderer);
            if matches!(space.visibility(), Visibility::Visible) || !space.output_has_toplevel {
                space.output.as_ref().map(|o| (o.1.name(), space.config.anchor));
            }
        }
        self.apply_toplevel_changes()
    }

    fn close_layer(&mut self, layer: &LayerSurface) {
        self.space_list
            .retain(|s| s.layer.as_ref().map(|s| s.wl_surface()) != Some(layer.wl_surface()));
    }

    fn output_leave(
        &mut self,
        c_output: sctk::reexports::client::protocol::wl_output::WlOutput,
        _s_output: Output,
    ) -> anyhow::Result<()> {
        self.outputs.retain(|o| o.0 != c_output);
        self.space_list.retain(|s| s.output.as_ref().map(|o| &o.0) != Some(&c_output));
        Ok(())
    }

    fn update_output(
        &mut self,
        c_output: WlOutput,
        s_output: Output,
        info: OutputInfo,
    ) -> anyhow::Result<bool> {
        self.outputs.retain(|o| o.0 != c_output);
        self.outputs.push((c_output.clone(), s_output.clone(), info.clone()));
        let mut found = false;
        for s in &mut self.space_list {
            if s.output.as_ref().map(|o| &o.0) == Some(&c_output) {
                let _ = s.update_output(c_output.clone(), s_output.clone(), info.clone());
                found = true;
            }
        }
        self.apply_toplevel_changes();

        Ok(found)
    }

    fn frame(&mut self, surface: &c_wl_surface::WlSurface, time: u32) {
        for s in self.space_list.iter_mut() {
            s.frame(surface, time);
        }
    }

    fn get_scale_factor(&self, surface: &wl_surface::WlSurface) -> std::option::Option<f64> {
        for s in &self.space_list {
            if let Some(scale) = s.get_scale_factor(surface) {
                return Some(scale);
            }
        }
        None
    }

    fn scale_factor_changed(
        &mut self,
        surface: &c_wl_surface::WlSurface,
        scale: f64,
        legacy: bool,
    ) {
        for s in &mut self.space_list {
            if s.layer.as_ref().map(|l| l.wl_surface()) == Some(surface)
                || s.popups.iter().any(|p| p.c_popup.wl_surface() == surface)
            {
                s.scale_factor_changed(surface, scale, legacy);
                break;
            }
        }
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _surface: &c_wl_surface::WlSurface,
        _new_transform: cctk::sctk::reexports::client::protocol::wl_output::Transform,
    ) {
        // TODO handle the preferred transform
    }

    fn generate_pointer_events(&mut self) -> Vec<sctk::seat::pointer::PointerEvent> {
        let mut events = Vec::new();
        for s in &mut self.space_list {
            events.append(&mut s.generate_pointer_events());
        }
        events
    }
}
