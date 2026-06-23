use cctk::wayland_client::Proxy;
use smithay::wayland::viewporter::ViewportCachedState;
use std::os::fd::OwnedFd;
use std::sync::Mutex;

use itertools::Itertools;
use sctk::data_device_manager::data_device::DataDeviceData;
use sctk::data_device_manager::data_offer::receive_to_fd;
use sctk::delegate_subcompositor;
use sctk::reexports::client::protocol::wl_data_device_manager::DndAction as ClientDndAction;
use sctk::shm::multi::MultiPool;
use smithay::backend::renderer::ImportDma;
use smithay::input::dnd::{DndAction, DndGrabHandler, DndTarget, SourceMetadata};
use smithay::input::pointer::CursorImageAttributes;
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_data_source::WlDataSource;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Transform};
use smithay::wayland::compositor::{SurfaceAttributes, with_states};
use smithay::wayland::dmabuf::{DmabufHandler, ImportNotifier};
use smithay::wayland::output::OutputHandler;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::selection::data_device::{
    DataDeviceHandler, WaylandDndGrabHandler, set_data_device_focus,
};
use smithay::wayland::selection::primary_selection::{
    PrimarySelectionHandler, PrimarySelectionState, set_primary_focus,
};
use smithay::wayland::selection::{SelectionHandler, SelectionSource, SelectionTarget};
use smithay::{
    delegate_data_device, delegate_dmabuf, delegate_output, delegate_primary_selection,
    delegate_seat,
};
use tracing::{error, info, trace};

use crate::iced::elements::target::SpaceTarget;
use crate::xdg_shell_wrapper::shared_state::GlobalState;
use crate::xdg_shell_wrapper::space::WrapperSpace;
use crate::xdg_shell_wrapper::util::write_and_attach_buffer;

pub(crate) mod compositor;
pub(crate) mod cursor;
pub(crate) mod fractional;
pub(crate) mod layer;
pub(crate) mod viewporter;
pub(crate) mod xdg_shell;

delegate_subcompositor!(GlobalState);

impl PrimarySelectionHandler for GlobalState {
    fn primary_selection_state(&mut self) -> &mut PrimarySelectionState {
        &mut self.server_state.primary_selection_state
    }
}

delegate_primary_selection!(GlobalState);

// Wl Seat
//

impl SeatHandler for GlobalState {
    type KeyboardFocus = SpaceTarget;
    type PointerFocus = SpaceTarget;
    type TouchFocus = SpaceTarget;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.server_state.seat_state
    }

    fn focus_changed(
        &mut self,
        seat: &smithay::input::Seat<Self>,
        focused: Option<&Self::KeyboardFocus>,
    ) {
        let dh = &self.server_state.display_handle;
        let Some(id) = focused.and_then(|s| s.wl_surface()).map(|s| s.id()) else {
            return;
        };
        if let Ok(client) = dh.get_client(id.clone()) {
            set_data_device_focus(dh, seat, Some(client));
            let client2 = dh.get_client(id).unwrap();
            set_primary_focus(dh, seat, Some(client2))
        }
    }

    fn cursor_image(
        &mut self,
        seat: &smithay::input::Seat<Self>,
        image: smithay::input::pointer::CursorImageStatus,
    ) {
        trace!("cursor icon");

        let Some(seat_pair) =
            self.server_state.seats.iter().find(|seat_pair| &seat_pair.server.seat == seat)
        else {
            return;
        };
        let Some(ptr) = seat_pair.client.ptr.as_ref() else {
            return;
        };

        match image {
            smithay::input::pointer::CursorImageStatus::Hidden => {
                let ptr = ptr.pointer();
                ptr.set_cursor(seat_pair.client.last_enter, None, 0, 0);
            },
            smithay::input::pointer::CursorImageStatus::Named(icon) => {
                trace!("Cursor image reset to default");
                if let Err(err) = ptr.set_cursor(&self.client_state.connection, icon) {
                    error!("{}", err);
                }
            },
            smithay::input::pointer::CursorImageStatus::Surface(surface) => {
                trace!("received surface with cursor image");
                let vp = with_states(&surface, |states| {
                    *states.cached_state.get::<ViewportCachedState>().current()
                });

                if let Some((vp, dst)) = self.client_state.cursor_vp.as_ref().zip(vp.dst) {
                    vp.set_destination(dst.w, dst.h);
                }
                if let Some((vp, src)) = self.client_state.cursor_vp.as_ref().zip(vp.src) {
                    vp.set_source(src.loc.x, src.loc.y, src.size.w, src.size.h);
                }

                if self.client_state.multipool.is_none() {
                    self.client_state.multipool = MultiPool::new(&self.client_state.shm_state).ok();
                }
                let multipool = match &mut self.client_state.multipool {
                    Some(m) => m,
                    None => {
                        error!("multipool is missing!");
                        return;
                    },
                };
                let cursor_surface = self.client_state.cursor_surface.get_or_insert_with(|| {
                    self.client_state
                        .compositor_state
                        .create_surface(&self.client_state.queue_handle)
                });

                let last_enter = seat_pair.client.last_enter;

                with_states(&surface, |data| {
                    let mut guard = data.cached_state.get::<SurfaceAttributes>();

                    let surface_attributes = guard.current();
                    let buf = surface_attributes.buffer.as_mut();
                    if let Some(hotspot) = data
                        .data_map
                        .get::<Mutex<CursorImageAttributes>>()
                        .and_then(|m| m.lock().ok())
                        .map(|attr| attr.hotspot)
                    {
                        trace!("Setting cursor {:?}", hotspot);
                        let ptr = ptr.pointer();
                        ptr.set_cursor(last_enter, Some(cursor_surface), hotspot.x, hotspot.y);

                        for ctr in 0..5 {
                            if let Err(e) = write_and_attach_buffer(
                                buf.as_ref().unwrap(),
                                cursor_surface,
                                ctr,
                                multipool,
                            ) {
                                info!("failed to attach buffer to cursor surface: {}", e);
                            } else {
                                break;
                            }
                        }
                    }
                });
            },
        }
    }
}

delegate_seat!(GlobalState);

// Wl Data Device
//

impl DataDeviceHandler for GlobalState {
    fn data_device_state(
        &mut self,
    ) -> &mut smithay::wayland::selection::data_device::DataDeviceState {
        &mut self.server_state.data_device_state
    }

    fn action_choice(
        &mut self,
        available: smithay::reexports::wayland_server::protocol::wl_data_device_manager::DndAction,
        preferred: smithay::reexports::wayland_server::protocol::wl_data_device_manager::DndAction,
    ) -> smithay::reexports::wayland_server::protocol::wl_data_device_manager::DndAction {
        use smithay::reexports::wayland_server::protocol::wl_data_device_manager::DndAction as WlDndAction;
        let dnd_seat =
            match self.server_state.seats.iter_mut().find(|s| s.client.dnd_source.is_some()) {
                Some(s) => s,
                None => return preferred,
            };

        let offer = match dnd_seat.client.data_device.data().drag_offer() {
            Some(offer) => offer,
            None => return preferred,
        };

        let mut client_actions = ClientDndAction::empty();
        if available.contains(WlDndAction::Copy) {
            client_actions |= ClientDndAction::Copy;
        }
        if available.contains(WlDndAction::Move) {
            client_actions |= ClientDndAction::Move;
        }
        if available.contains(WlDndAction::Ask) {
            client_actions |= ClientDndAction::Ask;
        }
        let mut client_preferred = ClientDndAction::empty();
        if preferred.contains(WlDndAction::Copy) {
            client_preferred |= ClientDndAction::Copy;
        }
        if preferred.contains(WlDndAction::Move) {
            client_preferred |= ClientDndAction::Move;
        }
        if preferred.contains(WlDndAction::Ask) {
            client_preferred |= ClientDndAction::Ask;
        }
        offer.set_actions(client_actions, client_preferred);
        preferred
    }
}

impl DndGrabHandler for GlobalState {
    fn dropped(
        &mut self,
        _: Option<DndTarget<'_, Self>>,
        _: bool,
        seat: Seat<Self>,
        _location: Point<f64, Logical>,
    ) {
        let seat = match self.server_state.seats.iter_mut().find(|s| s.server.seat == seat) {
            Some(s) => s,
            None => return,
        };
        // XXX is this correct?
        seat.server.dnd_source = None;
        seat.server.dnd_icon = None;
        seat.client.dnd_icon = None;
        seat.client.dnd_source = None;
    }
}

use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::input::dnd::{DnDGrab, GrabType, Source};
use smithay::input::pointer::Focus;
use smithay::utils::Serial;
impl WaylandDndGrabHandler for GlobalState {
    fn dnd_requested<S: Source>(
        &mut self,
        source: S,
        icon: Option<WlSurface>,
        seat: Seat<Self>,
        serial: Serial,
        type_: GrabType,
    ) {
        let seat = match self.server_state.seats.iter_mut().find(|s| s.server.seat == seat) {
            Some(s) => s,
            None => return,
        };

        if let Some(metadata) = source.metadata() {
            seat.client.next_dnd_offer_is_mine = true;
            let mut actions = ClientDndAction::empty();
            if metadata.dnd_actions.contains(&DndAction::Copy) {
                actions |= ClientDndAction::Copy;
            }
            if metadata.dnd_actions.contains(&DndAction::Move) {
                actions |= ClientDndAction::Move;
            }
            if metadata.dnd_actions.contains(&DndAction::Ask) {
                actions |= ClientDndAction::Ask;
            }

            let dnd_source = self.client_state.data_device_manager.create_drag_and_drop_source(
                &self.client_state.queue_handle,
                metadata.mime_types.iter().map(|m| m.as_str()).collect_vec(),
                actions,
            );
            if let Some(focus) =
                self.client_state.focused_surface.borrow().iter().find(|f| f.1 == seat.name)
            {
                let c_icon_surface = icon.as_ref().map(|_| {
                    self.client_state
                        .compositor_state
                        .create_surface(&self.client_state.queue_handle)
                });

                dnd_source.start_drag(
                    &seat.client.data_device,
                    &focus.0,
                    c_icon_surface.as_ref(),
                    seat.client.get_serial_of_last_seat_event(),
                );
                if let Some(client_surface) = c_icon_surface.as_ref() {
                    client_surface.frame(&self.client_state.queue_handle, client_surface.clone());
                    client_surface.commit();

                    seat.client.dnd_icon = Some(DndIcon {
                        surface: client_surface.clone(),
                        egl_surface: None,
                        output_tracker: OutputDamageTracker::new(
                            (32, 32),
                            self.space.space_list[0].scale,
                            Transform::Flipped180,
                        ),
                        is_ready: false,
                        has_frame: false,
                    });
                }
            }
            seat.client.dnd_source = Some(dnd_source);
        }

        // seat.server.dnd_source = source;
        seat.server.dnd_icon = icon;

        let seat = seat.server.seat.clone();
        match type_ {
            GrabType::Pointer => {
                let pointer = seat.get_pointer().unwrap();
                let start_data = pointer.grab_start_data().unwrap();
                pointer.set_grab(
                    self,
                    DnDGrab::new_pointer(
                        &self.server_state.display_handle,
                        start_data,
                        source,
                        seat,
                    ),
                    serial,
                    Focus::Keep,
                );
            },
            GrabType::Touch => {
                let touch = seat.get_touch().unwrap();
                let start_data = touch.grab_start_data().unwrap();
                touch.set_grab(
                    self,
                    DnDGrab::new_touch(&self.server_state.display_handle, start_data, source, seat),
                    serial,
                );
            },
        }
    }
}

use sctk::data_device_manager::data_offer::DragOffer;
// TODO rename
use crate::xdg_shell_wrapper::client_state::{ClientSeat, DndIcon};
pub(crate) struct ServerGrabSource {
    pub metadata: smithay::input::dnd::SourceMetadata,
    pub dnd_offer: DragOffer,
}

impl smithay::utils::IsAlive for ServerGrabSource {
    fn alive(&self) -> bool {
        self.dnd_offer.inner().is_alive()
    }
}

impl smithay::input::dnd::Source for ServerGrabSource {
    fn metadata(&self) -> Option<SourceMetadata> {
        Some(self.metadata.clone())
    }

    fn choose_action(&self, action: smithay::input::dnd::DndAction) {
        // XXX actions?
        //
        let mut c_action = ClientDndAction::empty();
        if action == DndAction::Copy {
            c_action |= ClientDndAction::Copy;
        }
        if action == DndAction::Move {
            c_action |= ClientDndAction::Move;
        }
        if action == DndAction::Ask {
            c_action |= ClientDndAction::Ask;
        }

        self.dnd_offer.set_actions(c_action, c_action)
    }

    fn send(&self, mime_type: &str, fd: OwnedFd) {
        receive_to_fd(self.dnd_offer.inner(), mime_type.to_owned(), fd)
    }

    fn drop_performed(&self) {}

    fn cancel(&self) {
        self.dnd_offer.destroy();
    }

    fn finished(&self) {
        self.dnd_offer.finish();
    }
}

delegate_data_device!(GlobalState);

// Wl Output
//

delegate_output!(GlobalState);

impl OutputHandler for GlobalState {}
// Dmabuf
//
impl DmabufHandler for GlobalState {
    fn dmabuf_state(&mut self) -> &mut smithay::wayland::dmabuf::DmabufState {
        &mut self.server_state.dmabuf_state.as_mut().unwrap().0
    }

    fn dmabuf_imported(
        &mut self,
        _global: &smithay::wayland::dmabuf::DmabufGlobal,
        dmabuf: smithay::backend::allocator::dmabuf::Dmabuf,
        _: ImportNotifier,
    ) {
        if let Some(Err(err)) =
            self.space.renderer().map(|renderer| renderer.import_dmabuf(&dmabuf, None))
        {
            error!("Failed to import dmabuf: {}", err);
        }
    }
}

impl SelectionHandler for GlobalState {
    type SelectionUserData = ();

    fn new_selection(
        &mut self,
        _target: SelectionTarget,
        source: Option<SelectionSource>,
        seat: Seat<GlobalState>,
    ) {
        let seat = match self.server_state.seats.iter_mut().find(|s| s.server.seat == seat) {
            Some(s) => s,
            None => return,
        };

        let serial = seat.client.get_serial_of_last_seat_event();

        if let Some(source) = source {
            seat.client.next_selection_offer_is_mine = true;
            let mime_types = source.mime_types();
            let copy_paste_source = self
                .client_state
                .data_device_manager
                .create_copy_paste_source(&self.client_state.queue_handle, mime_types);
            copy_paste_source.set_selection(&seat.client.data_device, serial);
            seat.client.copy_paste_source = Some(copy_paste_source);
            seat.server.selection_source = Some(source);
        } else {
            seat.client.data_device.unset_selection(serial)
        }
    }

    fn send_selection(
        &mut self,
        _target: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
        seat: Seat<Self>,
        _: &Self::SelectionUserData,
    ) {
        let seat = match self.server_state.seats.iter().find(|s| s.server.seat == seat) {
            Some(s) => s,
            None => return,
        };
        if let Some(offer) = seat.client.selection_offer.as_ref() {
            receive_to_fd(offer.inner(), mime_type, fd)
        }
    }
}

delegate_dmabuf!(GlobalState);
