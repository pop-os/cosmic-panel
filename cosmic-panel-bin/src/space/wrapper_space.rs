use std::cell::RefCell;
use std::mem;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::iced::elements::PopupMappedInternal;
use crate::iced::elements::target::SpaceTarget;
use crate::xdg_shell_wrapper::client::handlers::overlap::OverlapNotifyV1;
use crate::xdg_shell_wrapper::client_state::ClientFocus;
use crate::xdg_shell_wrapper::server_state::ServerPointerFocus;
use crate::xdg_shell_wrapper::shared_state::GlobalState;
use crate::xdg_shell_wrapper::space::{
    PanelPopup, Visibility, WrapperPopup, WrapperPopupState, WrapperSpace,
};
use crate::xdg_shell_wrapper::wp_fractional_scaling::FractionalScalingManager;
use crate::xdg_shell_wrapper::wp_security_context::SecurityContextManager;
use crate::xdg_shell_wrapper::wp_viewporter::ViewporterState;
use anyhow::bail;
use calloop::timer::Timer;
use cctk::wayland_client::protocol::wl_pointer::WlPointer;
use cctk::wayland_client::protocol::wl_seat;
use cosmic::iced::id;
use cosmic_panel_config::{CosmicPanelConfig, CosmicPanelOuput};
use itertools::izip;
use sctk::compositor::{CompositorState, Region};
use sctk::output::OutputInfo;
use sctk::reexports::client::protocol::{wl_output as c_wl_output, wl_surface as c_wl_surface};
use sctk::reexports::client::{Connection, Proxy, QueueHandle};
use sctk::seat::pointer::{BTN_LEFT, PointerEvent};
use sctk::shell::WaylandSurface;
use sctk::shell::wlr_layer::{LayerShell, LayerSurface, LayerSurfaceConfigure};
use sctk::shell::xdg::popup;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::desktop::space::SpaceElement;
use smithay::desktop::utils::bbox_from_surface_tree;
use smithay::desktop::{PopupManager, Space, Window};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface as s_WlSurface;
use smithay::reexports::wayland_server::{self, DisplayHandle, Resource};
use smithay::utils::{Logical, Rectangle};
use smithay::wayland::compositor::{SurfaceAttributes, with_states};
use smithay::wayland::fractional_scale::with_fractional_scale;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::xdg::{PopupSurface, PositionerState, SurfaceCachedState};
use tracing::{error, info};

use crate::iced::elements::{CosmicMappedInternal, PanelSpaceElement};
use crate::space::panel_space::AppletAutoClickAnchor;

use super::PanelSpace;
use super::layout::OverflowSection;
use super::panel_space::HoverId;

struct SpaceFocus<T> {
    target: T,
    relative_loc: smithay::utils::Point<i32, Logical>,
    space_target: SpaceTarget,
}

impl<T> SpaceFocus<T>
where
    T: PanelSpaceElement,
    SpaceTarget: TryFrom<T>,
{
    fn geo(&self, scale: f64) -> Rectangle<i32, Logical> {
        // FIXME
        // There has to be a way to avoid messing with the scaling like this...
        self.target.geometry().to_f64().to_physical(1.0).to_logical(scale).to_i32_round()
    }
}

fn space_focus<T>(space: &Space<T>, x: i32, y: i32) -> Option<SpaceFocus<T>>
where
    T: PanelSpaceElement,
    SpaceTarget: TryFrom<T>,
{
    space.elements().rev().find_map(|e| {
        let location = space.element_location(e)?;
        let mut bbox = e.geometry().to_f64();
        bbox.loc += location.to_f64();
        if let Some(configured_size) =
            e.toplevel().and_then(|t| t.with_committed_state(|s| s.as_ref().and_then(|s| s.size)))
        {
            if configured_size.w > 0 {
                bbox.size.w = bbox.size.w.min(configured_size.w as f64);
            }
            if configured_size.h > 0 {
                bbox.size.h = bbox.size.h.min(configured_size.h as f64);
            }
        }

        if bbox.contains((x as f64, y as f64)) {
            SpaceTarget::try_from(e.clone()).ok().map(|s| SpaceFocus {
                target: e.clone(),
                relative_loc: location,
                space_target: s,
            })
        } else {
            None
        }
    })
}

impl WrapperSpace for PanelSpace {
    type Config = CosmicPanelConfig;

    /// set the display handle of the space
    fn set_display_handle(&mut self, s_display: wayland_server::DisplayHandle) {
        self.s_display.replace(s_display);
    }

    /// get the client hovered surface of the space
    fn get_client_hovered_surface(&self) -> Rc<RefCell<ClientFocus>> {
        self.shared.c_hovered_surface.clone()
    }

    /// get the client focused surface of the space
    fn get_client_focused_surface(&self) -> Rc<RefCell<ClientFocus>> {
        self.shared.c_focused_surface.clone()
    }

    fn add_window(&mut self, w: Window) {
        self.is_dirty = true;
        self.needs_layout = true;
        if let Some(t) = w.toplevel() {
            t.with_pending_state(|state| {
                state.size = None;
                state.bounds = None;
            });
            t.send_pending_configure();
        }
        if let Some(s) = w.wl_surface() {
            with_states(&s, |states| {
                with_fractional_scale(states, |fractional_scale| {
                    fractional_scale.set_preferred_scale(self.scale);
                });
            });
        }
        let to_unmap = self
            .space
            .elements()
            .find(|my_window| {
                w.toplevel().and_then(|t| t.wl_surface().client().map(|c| c.id()))
                    == my_window.toplevel().and_then(|t| t.wl_surface().client().map(|c| c.id()))
            })
            .cloned();
        if let Some(w) = to_unmap {
            self.space.unmap_elem(&w);
        }
        self.space.map_element(CosmicMappedInternal::Window(w.clone()), (0, 0), false);
    }

    fn add_popup(
        &mut self,
        compositor_state: &sctk::compositor::CompositorState,
        fractional_scale_manager: Option<&FractionalScalingManager>,
        viewport: Option<&ViewporterState>,
        _conn: &sctk::reexports::client::Connection,
        qh: &QueueHandle<GlobalState>,
        xdg_shell_state: &mut sctk::shell::xdg::XdgShell,
        s_surface: PopupSurface,
        positioner: sctk::shell::xdg::XdgPositioner,
        positioner_state: PositionerState,
    ) -> anyhow::Result<()> {
        self.apply_positioner_state(&positioner, positioner_state, &s_surface);
        let c_wl_surface = compositor_state.create_surface(qh);
        let mut clear_exclude = Vec::new();
        let mut parent_parents = Vec::new();
        let parent = self
            .popups
            .iter()
            .find_map(|p| {
                if s_surface.get_parent_surface().is_some_and(|s| &s == p.s_surface.wl_surface()) {
                    clear_exclude.push(p.popup.c_popup.clone());
                    parent_parents.push(p.popup.parent.clone());
                    Some(p.popup.c_popup.clone())
                } else {
                    None
                }
            })
            .or_else(|| {
                let (p, space) = match self.overflow_popup.as_ref() {
                    Some((p, OverflowSection::Left)) => (p, &self.overflow_left),
                    Some((p, OverflowSection::Center)) => (p, &self.overflow_center),
                    Some((p, OverflowSection::Right)) => (p, &self.overflow_right),
                    _ => return None,
                };
                if space.elements().any(|e| {
                    if let PopupMappedInternal::Window(w) = e {
                        s_surface
                            .get_parent_surface()
                            .zip(w.wl_surface())
                            .is_some_and(|(a, b)| &a == b.as_ref())
                    } else {
                        false
                    }
                }) {
                    clear_exclude.push(p.c_popup.clone());
                    parent_parents.push(p.parent.clone());
                    Some(p.c_popup.clone())
                } else {
                    None
                }
            });

        // TODO maybe extract this to a function if it's needed elsewhere
        while !parent_parents.is_empty() {
            for p in
                self.popups.iter().map(|p| &p.popup).chain(self.overflow_popup.iter().map(|p| &p.0))
            {
                for w in mem::take(&mut parent_parents) {
                    if &w == p.c_popup.wl_surface() {
                        parent_parents.push(p.parent.clone());
                    }
                }
            }
        }

        self.close_popups(|p| clear_exclude.contains(&p.c_popup) || !p.grab);
        let c_popup = popup::Popup::from_surface(
            parent.as_ref().map(|p| p.xdg_surface()),
            &positioner,
            qh,
            c_wl_surface.clone(),
            xdg_shell_state,
        )?;
        if parent.is_none() {
            self.layer.as_ref().unwrap().get_popup(c_popup.xdg_popup());
        }

        let input_region = Region::new(compositor_state)?;

        if let Some(s_window_geometry) = with_states(s_surface.wl_surface(), |states| {
            let mut guard = states.cached_state.get::<SurfaceCachedState>();
            let pending = guard.pending();
            pending.geometry
        }) {
            c_popup.xdg_surface().set_window_geometry(
                s_window_geometry.loc.x,
                s_window_geometry.loc.y,
                s_window_geometry.size.w.max(1),
                s_window_geometry.size.h.max(1),
            );
        }

        if let Some(input_regions) = with_states(s_surface.wl_surface(), |states| {
            let mut guard_attr = states.cached_state.get::<SurfaceAttributes>();
            let attr = guard_attr.pending();
            attr.input_region.clone()
        }) {
            let mut area: i32 = 0;
            for r in input_regions.rects {
                area = area.saturating_add(r.1.size.w.saturating_mul(r.1.size.h));
                input_region.add(0, 0, r.1.size.w, r.1.size.h);
            }
            c_wl_surface.set_input_region(Some(input_region.wl_region()));
        }
        let fractional_scale =
            fractional_scale_manager.map(|f| f.fractional_scaling(&c_wl_surface, qh));

        let viewport = viewport.map(|v| {
            with_states(s_surface.wl_surface(), |states| {
                with_fractional_scale(states, |fractional_scale| {
                    fractional_scale.set_preferred_scale(self.scale);
                });
            });
            let viewport = v.get_viewport(&c_wl_surface, qh);
            viewport.set_destination(
                positioner_state.rect_size.w.max(1),
                positioner_state.rect_size.h.max(1),
            );
            viewport
        });
        if fractional_scale.is_none() {
            c_wl_surface.set_buffer_scale(self.scale as i32);
        }

        let cur_popup_state = Some(WrapperPopupState::WaitConfigure);
        tracing::info!("adding popup to popups");
        self.popups.push(WrapperPopup {
            popup: PanelPopup {
                damage_tracked_renderer: OutputDamageTracker::new(
                    positioner_state.rect_size.to_f64().to_physical(self.scale).to_i32_round(),
                    self.scale,
                    smithay::utils::Transform::Flipped180,
                ),
                c_popup,
                egl_surface: None,
                dirty: false,
                rectangle: Rectangle::from_size(positioner_state.rect_size),
                state: cur_popup_state,
                input_region: Some(input_region),
                wrapper_rectangle: Rectangle::from_size(positioner_state.rect_size),
                positioner,
                has_frame: true,
                fractional_scale,
                viewport,
                scale: self.scale,
                parent: parent
                    .map(|p| p.wl_surface().clone())
                    .unwrap_or(self.layer.as_ref().unwrap().wl_surface().clone()),
                grab: true,
                pending_initial_commit: true,
                blur_surface: None,
                corner_radius: None,
            },
            s_surface,
        });

        Ok(())
    }

    fn grab_popup(
        &mut self,
        popup: PopupSurface,
        seat: wl_seat::WlSeat,
        serial: u32,
    ) -> anyhow::Result<()> {
        if let Some(p) = self.popups.iter().find(|wp| wp.s_surface == popup) {
            p.popup.forward_grab(&seat, serial);
        }
        Ok(())
    }

    fn reposition_popup(
        &mut self,
        popup: PopupSurface,
        pos_state: PositionerState,
        token: u32,
    ) -> anyhow::Result<()> {
        popup.with_pending_state(|pending| {
            pending.geometry = Rectangle::from_size(pos_state.rect_size);
        });
        if let Some(i) = self.popups.iter().position(|wp| wp.s_surface == popup) {
            let p = &self.popups[i];

            let positioner: &sctk::shell::xdg::XdgPositioner = &p.popup.positioner;
            self.apply_positioner_state(positioner, pos_state, &p.s_surface);
            let p = &mut self.popups[i];
            let positioner: &sctk::shell::xdg::XdgPositioner = &p.popup.positioner;

            if positioner.version() >= 3 {
                p.popup.c_popup.reposition(positioner, token);
            }
            p.popup.state = Some(WrapperPopupState::WaitConfigure);
            if positioner.version() >= 3 {
                popup.send_repositioned(token);
            }
        }

        popup.send_configure()?;
        Ok(())
    }

    fn config(&self) -> Self::Config {
        self.config.clone()
    }

    fn spawn_clients(
        &mut self,
        mut display: DisplayHandle,
        qh: &QueueHandle<GlobalState>,
        security_context_manager: Option<SecurityContextManager>,
    ) -> anyhow::Result<()> {
        info!("Spawning applets");
        let config = self.config.clone();
        self.update_applet_clients(&config, &mut display, qh, security_context_manager.as_ref());
        info!("Done spawning applets");
        Ok(())
    }

    fn destroy(&mut self) {
        unimplemented!()
    }

    fn visibility(&self) -> Visibility {
        self.visibility
    }

    fn raise_window(&mut self, w: &Window, activate: bool) {
        self.space.raise_element(&CosmicMappedInternal::Window(w.clone()), activate);
    }

    fn dirty_window(&mut self, _dh: &DisplayHandle, s: &s_WlSurface) {
        self.is_dirty = true;
        self.needs_layout = true;
        self.last_dirty = Some(Instant::now());
        if let Some(w) = self
            .space
            .elements()
            .filter_map(|w| if let CosmicMappedInternal::Window(w) = w { Some(w) } else { None })
            .find(|w| w.wl_surface().is_some_and(|w| w.as_ref() == s))
        {
            w.on_commit();
            w.refresh();
        }

        if let Some(w) = self
            .overflow_left
            .elements()
            .chain(self.overflow_center.elements().chain(self.overflow_right.elements()))
            .find_map(|w| {
                if let PopupMappedInternal::Window(w) = w {
                    w.wl_surface().is_some_and(|w| w.as_ref() == s).then_some(w)
                } else {
                    None
                }
            })
        {
            if let Some(p) = self.overflow_popup.as_mut() {
                p.0.dirty = true;
            }
            w.on_commit();
            w.refresh();
        }
    }

    fn dirty_popup(&mut self, _dh: &DisplayHandle, s: &s_WlSurface) {
        self.is_dirty = true;
        self.needs_layout = true;
        self.space.refresh();

        if let Some(p) = self.popups.iter_mut().find(|p| p.s_surface.wl_surface() == s) {
            // the client is done setting up its popup, and any `grab` it
            // requested has been forwarded from the pre-commit hook
            // that runs before this
            p.popup.map();

            let p_bbox = bbox_from_surface_tree(p.s_surface.wl_surface(), (0, 0));
            if p_bbox.size == p.popup.rectangle.size {
                p.popup.dirty = true;
            }
        }
    }

    // XXX the renderer is provided by the container, not tracked by the
    // PanelSpace
    fn renderer(&mut self) -> Option<&mut GlesRenderer> {
        unimplemented!()
    }

    fn setup(
        &mut self,
        _compositor_state: &CompositorState,
        _fractional_scale_manager: Option<&FractionalScalingManager>,
        _security_context_manager: Option<SecurityContextManager>,
        _viewport: Option<&ViewporterState>,
        _layer_state: &mut LayerShell,
        _conn: &Connection,
        _qh: &QueueHandle<GlobalState>,
        overlap_notify: Option<OverlapNotifyV1>,
    ) {
        self.overlap_notify = overlap_notify;
    }

    /// returns false to forward the button press, and true to intercept
    fn handle_button(&mut self, seat_name: &str, press: bool) -> Option<SpaceTarget> {
        if let Some(prev_foc) = {
            let c_hovered_surface: &ClientFocus = &self.shared.c_hovered_surface.borrow();

            c_hovered_surface
                .iter()
                .enumerate()
                .find(|(_, f)| {
                    f.1 == seat_name
                        && (self.layer.as_ref().is_some_and(|s| s.wl_surface() == &f.0)
                            || self.popups.iter().any(|p| p.popup.c_popup.wl_surface() == &f.0)
                            || self
                                .overflow_popup
                                .as_ref()
                                .is_some_and(|p| p.0.c_popup.wl_surface() == &f.0))
                })
                .map(|(i, f)| (i, f.0.clone()))
        } {
            let target = self.s_hovered_surface.iter().find_map(|h| {
                if h.seat_name.as_str() == seat_name { Some(h.surface.clone()) } else { None }
            });
            if target.is_none() {
                // close popups when panel is pressed
                if self.layer.as_ref().map(|s| s.wl_surface()) == Some(&prev_foc.1) && press {
                    self.close_popups(|_| false);
                }
            }
            target
        } else {
            if press {
                self.close_popups(|_| false);
            }
            // no hover found
            // if has keyboard focus remove it and close popups
            self.keyboard_leave(seat_name, None);
            None
        }
    }

    ///  update active window based on pointer location
    fn update_pointer(
        &mut self,
        (x, y): (i32, i32),
        seat_name: &str,
        c_wl_surface: c_wl_surface::WlSurface,
        pointer: &WlPointer,
    ) -> Option<ServerPointerFocus> {
        let mut prev_hover =
            self.s_hovered_surface.iter_mut().enumerate().find(|(_, f)| f.seat_name == seat_name);
        let prev_foc = self.s_focused_surface.iter_mut().find(|f| f.1 == seat_name);

        let mut cur_client_hover_id: Option<HoverId> = None;
        let mut overflow_client_hover_id = None;
        let mut hover_relative_loc = None;
        let mut hover_geo = None;

        let ret = if let Some(p) =
            self.popups.iter().find(|p| p.popup.c_popup.wl_surface() == &c_wl_surface)
        {
            let geo = smithay::desktop::PopupKind::Xdg(p.s_surface.clone()).geometry();
            // special handling for popup bc they exist on their own client
            // surface

            if let Some(prev_foc) = prev_foc {
                prev_foc.0 = p.s_surface.wl_surface().clone().into();
            } else {
                self.s_focused_surface
                    .push((p.s_surface.wl_surface().clone().into(), seat_name.to_string()));
            }
            if let Some((_, prev_foc)) = prev_hover.as_mut() {
                prev_foc.c_pos = p.popup.rectangle.loc;
                prev_foc.s_pos = (p.popup.rectangle.loc - geo.loc).to_f64();

                prev_foc.surface = p.s_surface.wl_surface().clone().into();
                Some(prev_foc.clone())
            } else {
                self.s_hovered_surface.push(ServerPointerFocus {
                    surface: p.s_surface.wl_surface().clone().into(),
                    seat_name: seat_name.to_string(),
                    c_pos: p.popup.rectangle.loc,
                    s_pos: (p.popup.rectangle.loc - geo.loc).to_f64(),
                });
                self.s_hovered_surface.last().cloned()
            }
        } else if self.layer.as_ref().is_some_and(|s| *s.wl_surface() == c_wl_surface) {
            // if not on this panel's client surface return None
            if let Some(focus) = space_focus(&self.space, x, y) {
                let geo = focus.geo(self.scale);

                if let Some(prev_kbd) = prev_foc {
                    prev_kbd.0 = focus.space_target.clone();
                } else {
                    self.s_focused_surface
                        .push((focus.space_target.clone(), seat_name.to_string()));
                }

                hover_geo = Some(geo);
                hover_relative_loc = Some(focus.relative_loc);
                match &focus.target {
                    CosmicMappedInternal::Window(w) => {
                        cur_client_hover_id = w
                            .wl_surface()
                            .and_then(|t| t.client().map(|c| HoverId::Client(c.id())));
                    },
                    CosmicMappedInternal::OverflowButton(b) => {
                        cur_client_hover_id =
                            Some(HoverId::Overflow(b.with_program(|p| p.id.clone())));
                    },
                    _ => {},
                };

                if let Some((_, prev_foc)) = prev_hover.as_mut() {
                    prev_foc.s_pos = focus.relative_loc.to_f64();
                    prev_foc.c_pos = geo.loc;
                    prev_foc.surface = focus.space_target;
                    Some(prev_foc.clone())
                } else {
                    self.s_hovered_surface.push(ServerPointerFocus {
                        surface: focus.space_target,
                        seat_name: seat_name.to_string(),
                        c_pos: geo.loc,
                        s_pos: focus.relative_loc.to_f64(),
                    });
                    self.s_hovered_surface.last().cloned()
                }
            } else {
                if let Some((prev_i, _)) = prev_hover {
                    self.s_hovered_surface.swap_remove(prev_i);
                }
                None
            }
        } else if self
            .overflow_popup
            .as_ref()
            .is_some_and(|p| p.0.c_popup.wl_surface() == &c_wl_surface)
        {
            let (_, section) = self.overflow_popup.as_ref().unwrap();
            let space = match section {
                OverflowSection::Left => &self.overflow_left,
                OverflowSection::Center => &self.overflow_center,
                OverflowSection::Right => &self.overflow_right,
            };

            if let Some(focus) = space_focus(space, x, y) {
                let geo = focus.geo(self.scale);

                if let Some(prev_kbd) = prev_foc {
                    prev_kbd.0 = focus.space_target.clone();
                } else {
                    self.s_focused_surface
                        .push((focus.space_target.clone(), seat_name.to_string()));
                }

                hover_geo = Some(geo);
                hover_relative_loc = Some(focus.relative_loc);
                overflow_client_hover_id =
                    focus.target.wl_surface().and_then(|t| t.client().map(|c| c.id()));

                if let Some((_, prev_foc)) = prev_hover.as_mut() {
                    prev_foc.s_pos = focus.relative_loc.to_f64();
                    prev_foc.c_pos = geo.loc;
                    prev_foc.surface = focus.space_target.clone();
                    Some(prev_foc.clone())
                } else {
                    self.s_hovered_surface.push(ServerPointerFocus {
                        surface: focus.space_target.clone(),
                        seat_name: seat_name.to_string(),
                        c_pos: geo.loc,
                        s_pos: focus.relative_loc.to_f64(),
                    });
                    self.s_hovered_surface.last().cloned()
                }
            } else {
                if let Some((prev_i, _)) = prev_hover {
                    self.s_hovered_surface.swap_remove(prev_i);
                }
                None
            }
        } else {
            if self
                .space
                .elements()
                .filter_map(|e| e.wl_surface())
                .chain(self.overflow_left.elements().filter_map(|e| e.wl_surface()))
                .any(|e| {
                    e.wl_surface()
                        .zip(prev_hover.as_ref().map(|s| s.1.surface.wl_surface()))
                        .is_some_and(|(s, prev_hover)| Some(s) == prev_hover)
                })
            {
                let (pos, _) = prev_hover.unwrap();
                self.s_hovered_surface.remove(pos);
            }
            return None;
        };

        let prev_popup_client = self
            .popups
            .iter()
            .find(|p| p.popup.grab)
            .and_then(|p| p.s_surface.wl_surface().client())
            .map(|c| c.id());

        if let Some(auto_hover_dur) =
            self.config.autohover_delay_ms.map(|d| Duration::from_millis(d as u64))
        {
            if prev_popup_client.is_some()
                && matches!(cur_client_hover_id, Some(HoverId::Overflow(_)))
            {
                self.hover_track.set_hover_id(cur_client_hover_id.clone());

                // TODO replace this with dbus protocol, I guess...
                // maybe replace with wayland protocol. Idk...
                if let Some((relative_loc, geo)) = hover_relative_loc.zip(hover_geo) {
                    self.hover_track.set_hover_id(cur_client_hover_id.clone());
                    let cur_hover_track = self.hover_track.clone();
                    let panel_id = self.id();
                    let pointer = pointer.clone();

                    let on_autohover = move |data: &mut GlobalState| {
                        let mut generated_events = if let Some(space) = data
                            .space
                            .space_list
                            .iter_mut()
                            .find(|s| s.id() == panel_id)
                            .filter(|s| s.hover_track == cur_hover_track)
                        {
                            // place in center
                            let mut p = (x, y);
                            p.0 = relative_loc.x + geo.size.w / 2;
                            p.1 = relative_loc.y + geo.size.h / 2;

                            vec![
                                PointerEvent {
                                    surface: space.layer.as_ref().unwrap().wl_surface().clone(),
                                    position: (p.0 as f64, p.1 as f64),
                                    kind: sctk::seat::pointer::PointerEventKind::Motion { time: 0 },
                                },
                                PointerEvent {
                                    surface: space.layer.as_ref().unwrap().wl_surface().clone(),
                                    position: (p.0 as f64, p.1 as f64),
                                    kind: sctk::seat::pointer::PointerEventKind::Press {
                                        time: 0,
                                        button: BTN_LEFT,
                                        serial: 0,
                                    },
                                },
                                PointerEvent {
                                    surface: space.layer.as_ref().unwrap().wl_surface().clone(),
                                    position: (p.0 as f64, p.1 as f64),
                                    kind: sctk::seat::pointer::PointerEventKind::Release {
                                        time: 0,
                                        button: BTN_LEFT,
                                        serial: 0,
                                    },
                                },
                            ]
                        } else {
                            return calloop::timer::TimeoutAction::Drop;
                        };
                        if !generated_events.is_empty() {
                            data.update_generated_event_serial(&mut generated_events);
                            let conn = data.client_state.connection.clone();
                            data.pointer_frame_inner(&conn, &pointer, &generated_events);
                        }

                        calloop::timer::TimeoutAction::Drop
                    };
                    if auto_hover_dur.as_millis() > 0 {
                        _ = self.shared.loop_handle.insert_source(
                            Timer::from_duration(auto_hover_dur),
                            move |_, _, data| on_autohover(data),
                        );
                    } else {
                        _ = self.shared.loop_handle.insert_idle(move |data| {
                            _ = on_autohover(data);
                        });
                    }
                }
            } else if ((prev_popup_client
                .as_ref()
                .zip(cur_client_hover_id.as_ref())
                .is_some_and(|(a, b)| &HoverId::Client(a.clone()) != b))
                || self.overflow_popup.is_some())
                && matches!(cur_client_hover_id, Some(HoverId::Client(_)))
            {
                self.hover_track.set_hover_id(cur_client_hover_id.clone());
                let cur_hover_track = self.hover_track.clone();
                let panel_id = self.id();
                let pointer = pointer.clone();
                let on_autohover = move |data: &mut GlobalState| {
                    let mut generated_events = if let Some(space) = data
                        .space
                        .space_list
                        .iter_mut()
                        .find(|s| s.id() == panel_id)
                        .filter(|s| s.hover_track == cur_hover_track)
                    {
                        // exit early if popup is open on the hover id
                        if space.popups.first().zip(cur_client_hover_id.as_ref()).is_some_and(|(p, c_id)| matches!(c_id, HoverId::Client(c_id) if Some(c_id) == p.s_surface.wl_surface().client().map(|c| c.id()).as_ref())) {
                            return calloop::timer::TimeoutAction::Drop;
                        }

                        // send press to new client if it hover flag is set
                        let left_guard = space.clients_left.lock().unwrap();
                        let center_guard = space.clients_center.lock().unwrap();
                        let right_guard = space.clients_right.lock().unwrap();
                        let client = left_guard
                            .iter()
                            .chain(center_guard.iter())
                            .chain(right_guard.iter())
                            .find(|c| {
                                c.auto_popup_hover_press.is_some()
                                    && c.client
                                        .as_ref()
                                        .zip(cur_client_hover_id.as_ref())
                                        .is_some_and(|(c, id)| HoverId::Client(c.id()) == *id)
                                    || c.client
                                        .as_ref()
                                        .zip(overflow_client_hover_id.as_ref())
                                        .is_some_and(|(c, id)| c.id() == *id)
                            })
                            .or({
                                // overflow button
                                None
                            })
                            .zip(hover_relative_loc)
                            .zip(hover_geo);
                        if let Some(((c, relative_loc), geo)) = client {
                            let mut p = (x, y);
                            let effective_anchor = match (
                                c.auto_popup_hover_press.unwrap(),
                                space.config.is_horizontal(),
                            ) {
                                (AppletAutoClickAnchor::Start, true) => AppletAutoClickAnchor::Left,
                                (AppletAutoClickAnchor::Start, false) => AppletAutoClickAnchor::Top,
                                (AppletAutoClickAnchor::End, true) => AppletAutoClickAnchor::Right,
                                (AppletAutoClickAnchor::End, false) => {
                                    AppletAutoClickAnchor::Bottom
                                },
                                (anchor, _) => anchor,
                            };
                            match effective_anchor {
                                AppletAutoClickAnchor::Top => {
                                    // centered on the top edge
                                    p.0 = relative_loc.x + geo.size.w / 2;
                                    p.1 = relative_loc.y + 4;
                                },
                                AppletAutoClickAnchor::Bottom => {
                                    // centered on the bottom edge
                                    p.0 = relative_loc.x + geo.size.w / 2;
                                    p.1 = relative_loc.y + geo.size.h - 4;
                                },
                                AppletAutoClickAnchor::Left => {
                                    // centered on the left edge
                                    p.0 = relative_loc.x + 4;
                                    p.1 = relative_loc.y + geo.size.h / 2;
                                },
                                AppletAutoClickAnchor::Right => {
                                    // centered on the right edge
                                    p.0 = relative_loc.x + geo.size.w - 4;
                                    p.1 = relative_loc.y + geo.size.h / 2;
                                },
                                AppletAutoClickAnchor::Center => {
                                    // centered on the center
                                    p.0 = relative_loc.x + geo.size.w / 2;
                                    p.1 = relative_loc.y + geo.size.h / 2;
                                },
                                AppletAutoClickAnchor::Auto => {
                                    let relative_x = x - relative_loc.x;
                                    let relative_y = y - relative_loc.y;
                                    if relative_x.abs() < 4 {
                                        p.0 += 4;
                                    } else if (relative_x - geo.size.w).abs() < 4 {
                                        p.0 -= 4;
                                    }
                                    if relative_y.abs() < 4 {
                                        p.1 += 4;
                                    } else if (relative_y - geo.size.h).abs() < 4 {
                                        p.1 -= 4;
                                    }
                                },
                                AppletAutoClickAnchor::Start | AppletAutoClickAnchor::End => {
                                    tracing::warn!("Invalid anchor for auto click");
                                    // should be handled above
                                },
                            }
                            vec![
                                PointerEvent {
                                    surface: space.layer.as_ref().unwrap().wl_surface().clone(),
                                    position: (p.0 as f64, p.1 as f64),
                                    kind: sctk::seat::pointer::PointerEventKind::Motion { time: 0 },
                                },
                                PointerEvent {
                                    surface: space.layer.as_ref().unwrap().wl_surface().clone(),
                                    position: (p.0 as f64, p.1 as f64),
                                    kind: sctk::seat::pointer::PointerEventKind::Press {
                                        time: 0,
                                        button: BTN_LEFT,
                                        serial: 0,
                                    },
                                },
                                PointerEvent {
                                    surface: space.layer.as_ref().unwrap().wl_surface().clone(),
                                    position: (p.0 as f64, p.1 as f64),
                                    kind: sctk::seat::pointer::PointerEventKind::Release {
                                        time: 0,
                                        button: BTN_LEFT,
                                        serial: 0,
                                    },
                                },
                            ]
                        } else {
                            return calloop::timer::TimeoutAction::Drop;
                        }
                    } else {
                        return calloop::timer::TimeoutAction::Drop;
                    };

                    if !generated_events.is_empty() {
                        data.update_generated_event_serial(&mut generated_events);
                        let conn = data.client_state.connection.clone();
                        data.pointer_frame_inner(&conn, &pointer, &generated_events);
                    }
                    calloop::timer::TimeoutAction::Drop
                };
                if auto_hover_dur.as_millis() > 0 {
                    _ =
                        self.shared.loop_handle.insert_source(
                            Timer::from_duration(auto_hover_dur),
                            move |_, _, data| on_autohover(data),
                        );
                } else {
                    _ = self.shared.loop_handle.insert_idle(move |data| {
                        _ = on_autohover(data);
                    });
                }
            } else {
                self.hover_track.set_hover_id(None);
            }
        }
        ret
    }

    fn touch_under(
        &mut self,
        (x, y): (i32, i32),
        seat_name: &str,
        c_wl_surface: c_wl_surface::WlSurface,
    ) -> Option<ServerPointerFocus> {
        // first check if the motion is on a popup's client surface
        if let Some(p) = self.popups.iter().find(|p| p.popup.c_popup.wl_surface() == &c_wl_surface)
        {
            let geo = smithay::desktop::PopupKind::Xdg(p.s_surface.clone()).geometry();
            Some(ServerPointerFocus {
                surface: p.s_surface.wl_surface().clone().into(),
                seat_name: seat_name.to_string(),
                c_pos: p.popup.rectangle.loc,
                s_pos: (p.popup.rectangle.loc - geo.loc).to_f64(),
            })
        } else if self
            .overflow_popup
            .as_ref()
            .is_some_and(|p| p.0.c_popup.wl_surface() == &c_wl_surface)
        {
            let (_, section) = self.overflow_popup.as_ref().unwrap();
            let space = match section {
                OverflowSection::Left => &self.overflow_left,
                OverflowSection::Center => &self.overflow_center,
                OverflowSection::Right => &self.overflow_right,
            };

            if let Some(focus) = space_focus(space, x, y) {
                let geo = focus.geo(self.scale);
                Some(ServerPointerFocus {
                    surface: focus.space_target,
                    seat_name: seat_name.to_string(),
                    c_pos: geo.loc,
                    s_pos: focus.relative_loc.to_f64(),
                })
            } else {
                None
            }
        } else {
            // if not on this panel's client surface return None
            if self.layer.as_ref().map(|s| *s.wl_surface() != c_wl_surface).unwrap_or(true) {
                return None;
            }
            if let Some(focus) = space_focus(&self.space, x, y) {
                let geo = focus.geo(self.scale);
                Some(ServerPointerFocus {
                    surface: focus.space_target,
                    seat_name: seat_name.to_string(),
                    c_pos: geo.loc,
                    s_pos: focus.relative_loc.to_f64(),
                })
            } else {
                None
            }
        }
    }

    fn keyboard_leave(&mut self, seat_name: &str, f: Option<c_wl_surface::WlSurface>) {
        if let Some(surface) = f.as_ref() {
            // if not a leaf, return early
            if self.popups.iter().any(|p| p.popup.parent == *surface) {
                return;
            }

            if self.layer.as_ref().is_some_and(|l| l.wl_surface() == surface)
                && (self.popups.iter().any(|p| p.popup.grab) || self.overflow_popup.is_some())
            {
                return;
            }

            // Ignore event for wl_surface that isn't our layer or popup (i.e. a
            // different panel)
            if !self.layer.as_ref().is_some_and(|l| l.wl_surface() == surface)
                && !self
                    .overflow_popup
                    .as_ref()
                    .is_some_and(|(p, _)| p.c_popup.wl_surface() == surface)
                && !self.popups.iter().any(|p| p.popup.c_popup.wl_surface() == surface)
            {
                return;
            }
        }

        self.s_focused_surface.retain(|(_, name)| name != seat_name);
        self.close_popups(|_| false);
    }

    fn keyboard_enter(&mut self, _: &str, _: c_wl_surface::WlSurface) -> Option<s_WlSurface> {
        None
    }

    fn pointer_leave(&mut self, seat_name: &str, _s: Option<c_wl_surface::WlSurface>) {
        self.hover_track.set_hover_id(None);
        self.s_hovered_surface.retain(|focus| focus.seat_name != seat_name);
    }

    fn pointer_enter(
        &mut self,
        dim: (i32, i32),
        seat_name: &str,
        c_wl_surface: c_wl_surface::WlSurface,
        pointer: &WlPointer,
    ) -> Option<ServerPointerFocus> {
        self.update_pointer(dim, seat_name, c_wl_surface, pointer)
    }

    fn configure_popup(
        &mut self,
        _popup: &sctk::shell::xdg::popup::Popup,
        _config: sctk::shell::xdg::popup::PopupConfigure,
    ) {
    }

    fn close_popup(&mut self, popup: &sctk::shell::xdg::popup::Popup) {
        self.close_popups(|p| p.c_popup.wl_surface() == popup.wl_surface());
    }

    // handled by custom method with access to renderer instead
    fn configure_layer(&mut self, _: &LayerSurface, _: LayerSurfaceConfigure) {}

    // handled by the container
    fn close_layer(&mut self, _: &LayerSurface) {}

    // handled by the container
    fn output_leave(
        &mut self,
        _c_output: c_wl_output::WlOutput,
        _s_output: Output,
    ) -> anyhow::Result<()> {
        anyhow::bail!("Output leaving should be handled by the container")
    }

    fn update_output(
        &mut self,
        c_output: c_wl_output::WlOutput,
        s_output: Output,
        info: OutputInfo,
    ) -> anyhow::Result<bool> {
        let old = self.output.replace((c_output, s_output, info.clone()));

        if old.is_some_and(|old| old.2.logical_size != info.logical_size) {
            let (width, height) = if self.config.is_horizontal() {
                (0, self.dimensions.h)
            } else {
                (self.dimensions.w, 0)
            };
            self.pending_dimensions = Some((width, height).into());
            self.clear();
        }
        Ok(true)
    }

    fn new_output(
        &mut self,
        compositor_state: &sctk::compositor::CompositorState,
        fractional_scale_manager: Option<&FractionalScalingManager>,
        viewport: Option<&ViewporterState>,
        layer_state: &mut LayerShell,
        _conn: &sctk::reexports::client::Connection,
        qh: &QueueHandle<GlobalState>,
        c_output: Option<c_wl_output::WlOutput>,
        s_output: Option<Output>,
        output_info: Option<OutputInfo>,
    ) -> anyhow::Result<()> {
        if self.output.is_some() {
            bail!("output already setup for this panel");
        }
        if let (Some(_c_output), Some(s_output), Some(output_info)) =
            (c_output.as_ref(), s_output.as_ref(), output_info.as_ref())
        {
            self.space.map_output(s_output, output_info.location);
            self.overflow_center.map_output(s_output, output_info.location);
            self.overflow_left.map_output(s_output, output_info.location);
            self.overflow_right.map_output(s_output, output_info.location);

            match &self.config.output {
                CosmicPanelOuput::Active => {
                    bail!("output does not match config")
                },
                CosmicPanelOuput::Name(config_name)
                    if output_info.name != Some(config_name.to_string()) =>
                {
                    bail!("output does not match config")
                },
                _ => {},
            };
            if matches!(self.config.output, CosmicPanelOuput::Active) && self.layer.is_some() {
                return Ok(());
            }
        } else if !matches!(self.config.output, CosmicPanelOuput::Active) {
            bail!("output does not match config");
        }

        let config = self.config.clone();
        self.create_layer_surface(
            &config,
            c_output.as_ref(),
            compositor_state,
            fractional_scale_manager,
            viewport,
            layer_state,
            qh,
        )?;

        self.output =
            izip!(c_output.into_iter(), s_output.into_iter(), output_info.as_ref().cloned()).next();
        self.left_overflow_button_id = id::Id::new(format!("left_overflow_button_{}", self.id()));
        self.right_overflow_button_id = id::Id::new(format!("right_overflow_button_{}", self.id()));
        self.center_overflow_button_id =
            id::Id::new(format!("center_overflow_button_{}", self.id()));
        self.left_overflow_popup_id = id::Id::new(format!("left_overflow_popup_{}", self.id()));
        self.right_overflow_popup_id = id::Id::new(format!("right_overflow_popup_{}", self.id()));
        self.center_overflow_popup_id = id::Id::new(format!("center_overflow_popup_{}", self.id()));

        let security_context_manager = self.shared.security_context_manager.borrow().clone();
        if let Err(err) =
            self.spawn_clients(self.s_display.clone().unwrap(), qh, security_context_manager)
        {
            error!(?err, "Failed to spawn clients");
        }
        Ok(())
    }

    fn handle_events(
        &mut self,
        _dh: &DisplayHandle,
        _qh: &QueueHandle<GlobalState>,
        _popup_manager: &mut PopupManager,
        _time: u32,
        _throttle: Option<Duration>,
    ) -> Instant {
        unimplemented!()
    }

    fn frame(&mut self, surface: &c_wl_surface::WlSurface, _time: u32) {
        if Some(surface) == self.layer.as_ref().map(|l| l.wl_surface()) {
            self.has_frame = true;
        } else if let Some(p) =
            self.popups.iter_mut().find(|p| surface == p.popup.c_popup.wl_surface())
        {
            p.popup.has_frame = true;
        }
    }

    fn get_scale_factor(&self, surface: &s_WlSurface) -> std::option::Option<f64> {
        let client = surface.client();
        if self
            .clients_center
            .lock()
            .unwrap()
            .iter()
            .chain(self.clients_left.lock().unwrap().iter())
            .chain(self.clients_right.lock().unwrap().iter())
            .any(|c| c.client.as_ref() == client.as_ref())
        {
            Some(self.scale)
        } else {
            None
        }
    }

    fn scale_factor_changed(
        &mut self,
        surface: &c_wl_surface::WlSurface,
        scale: f64,
        legacy: bool,
    ) {
        info!(
            "Scale factor changed {scale} for as surface in space \"{}\" on {}",
            self.config.name,
            self.output
                .as_ref()
                .and_then(|o| o.2.name.clone())
                .unwrap_or_else(|| "None".to_string())
        );
        if Some(surface) == self.layer.as_ref().map(|l| l.wl_surface())
            || self.overflow_popup.as_ref().is_some_and(|p| p.0.c_popup.wl_surface() == surface)
        {
            self.scale = scale;

            if legacy && self.layer_fractional_scale.is_none() {
                surface.set_buffer_scale(scale as i32);
                if let Some(output) = self.output.as_ref() {
                    output.1.change_current_state(
                        None,
                        None,
                        Some(smithay::output::Scale::Integer(scale as i32)),
                        None,
                    );
                }
            } else {
                surface.set_buffer_scale(1);
                if let Some(output) = self.output.as_ref() {
                    output.1.change_current_state(
                        None,
                        None,
                        Some(smithay::output::Scale::Fractional(scale)),
                        None,
                    );
                }
                if let Some(viewport) = self.layer_viewport.as_ref() {
                    viewport.set_destination(self.actual_size.w.max(1), self.actual_size.h.max(1));
                }
                for surface in self.space.elements().filter_map(|e| e.toplevel()) {
                    surface.with_pending_state(|s| {
                        s.size = None;
                        s.bounds = None;
                    });
                    with_states(surface.wl_surface(), |states| {
                        with_fractional_scale(states, |fractional_scale| {
                            fractional_scale.set_preferred_scale(scale);
                        });
                    });
                    surface.send_configure();
                }

                for o in self
                    .overflow_left
                    .elements()
                    .chain(self.overflow_center.elements().chain(self.overflow_right.elements()))
                {
                    let w = match o {
                        PopupMappedInternal::Window(w) => w,
                        _ => continue,
                    };
                    let Some(toplevel) = o.toplevel() else {
                        continue;
                    };
                    toplevel.with_pending_state(|s| {
                        s.size = None;
                        s.bounds = None;
                    });
                    with_states(toplevel.wl_surface(), |states| {
                        with_fractional_scale(states, |fractional_scale| {
                            fractional_scale.set_preferred_scale(scale);
                        });
                    });
                    toplevel.send_configure();
                    self.space.map_element(CosmicMappedInternal::Window(w.clone()), (0, 0), false);
                }

                let left = self.overflow_left.elements().cloned().collect::<Vec<_>>();
                for e in left {
                    self.overflow_left.unmap_elem(&e);
                }
                let center = self.overflow_center.elements().cloned().collect::<Vec<_>>();
                for e in center {
                    self.overflow_center.unmap_elem(&e);
                }
                let right = self.overflow_right.elements().cloned().collect::<Vec<_>>();
                for e in right {
                    self.overflow_right.unmap_elem(&e);
                }
                // remove all buttons from space
                let buttons = self
                    .space
                    .elements()
                    .filter(|&b| matches!(b, CosmicMappedInternal::OverflowButton(_)))
                    .cloned()
                    .collect::<Vec<_>>();
                for e in buttons {
                    self.space.unmap_elem(&e);
                }

                self.reset_overflow();
            }

            let scaled = self.dimensions.to_f64();
            self.dimensions = scaled.to_i32_round();
            self.pending_dimensions =
                Some(if self.config.is_horizontal() { (0, 1) } else { (1, 0) }.into());
            self.clear();

            // check overflow popup
            if let Some((popup, _)) = self.overflow_popup.as_mut() {
                popup.scale = scale;
                let Rectangle { loc, size } = popup.rectangle;
                if popup.state.is_none() {
                    popup.state = Some(WrapperPopupState::Rectangle {
                        x: loc.x,
                        y: loc.y,
                        width: size.w,
                        height: size.h,
                    });
                }

                if legacy {
                    popup.c_popup.wl_surface().set_buffer_scale(scale as i32);
                } else {
                    popup.c_popup.wl_surface().set_buffer_scale(1);

                    if let Some(viewport) = popup.viewport.as_ref() {
                        viewport.set_destination(size.w.max(1), size.h.max(1));
                    }
                }
            }
        }
        for popup in &mut self.popups {
            if popup.popup.c_popup.wl_surface() != surface {
                continue;
            }
            popup.popup.scale = scale;
            let Rectangle { loc, size } = popup.popup.rectangle;
            if popup.popup.state.is_none() {
                popup.popup.state = Some(WrapperPopupState::Rectangle {
                    x: loc.x,
                    y: loc.y,
                    width: size.w,
                    height: size.h,
                });
            }

            with_states(popup.s_surface.wl_surface(), |states| {
                with_fractional_scale(states, |fractional_scale| {
                    fractional_scale.set_preferred_scale(scale);
                });
            });
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
}
