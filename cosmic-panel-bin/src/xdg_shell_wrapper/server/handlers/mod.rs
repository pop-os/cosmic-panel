use std::{cell::RefMut, os::fd::OwnedFd, rc::Rc, sync::Mutex};

use itertools::Itertools;
use sctk::{
    data_device_manager::data_offer::receive_to_fd,
    reexports::client::{protocol::wl_data_device_manager::DndAction as ClientDndAction, Proxy},
};
use smithay::{
    backend::{
        egl::EGLSurface,
        renderer::{damage::OutputDamageTracker, ImportDma},
    },
    delegate_data_device, delegate_dmabuf, delegate_output, delegate_primary_selection,
    delegate_seat,
    input::{pointer::CursorImageAttributes, Seat, SeatHandler, SeatState},
    reexports::wayland_server::{
        protocol::{
            wl_data_device_manager::DndAction, wl_data_source::WlDataSource, wl_surface::WlSurface,
        },
        Resource,
    },
    utils::Transform,
    wayland::{
        compositor::{with_states, SurfaceAttributes},
        dmabuf::{DmabufHandler, ImportNotifier},
        output::OutputHandler,
        selection::{
            data_device::{
                set_data_device_focus, with_source_metadata, ClientDndGrabHandler,
                DataDeviceHandler, ServerDndGrabHandler,
            },
            primary_selection::{
                set_primary_focus, PrimarySelectionHandler, PrimarySelectionState,
            },
            SelectionHandler, SelectionSource, SelectionTarget,
        },
    },
};
use tracing::{error, trace};
use wayland_egl::WlEglSurface;

use crate::xdg_shell_wrapper::{
    shared_state::GlobalState,
    space::{ClientEglSurface, WrapperSpace},
    util::write_and_attach_buffer,
};

pub(crate) mod compositor;
pub(crate) mod fractional;
pub(crate) mod layer;
pub(crate) mod viewporter;
pub(crate) mod xdg_shell;

impl PrimarySelectionHandler for GlobalState {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.server_state.primary_selection_state
    }
}

delegate_primary_selection!(GlobalState);

// Wl Seat
//

impl SeatHandler for GlobalState {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.server_state.seat_state
    }

    fn focus_changed(
        &mut self,
        seat: &smithay::input::Seat<Self>,
        focused: Option<&Self::KeyboardFocus>,
    ) {
        let dh = &self.server_state.display_handle;
        if let Some(client) = focused.and_then(|s| dh.get_client(s.id()).ok()) {
            set_data_device_focus(dh, seat, Some(client));
            let client2 = focused.and_then(|s| dh.get_client(s.id()).ok()).unwrap();
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
        // render dnd icon to the active dnd icon surface
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

                let _ = with_states(&surface, |data| {
                    let surface_attributes = data.cached_state.current::<SurfaceAttributes>();
                    let buf = RefMut::map(surface_attributes, |s| &mut s.buffer);
                    if let Some(hotspot) = data
                        .data_map
                        .get::<Mutex<CursorImageAttributes>>()
                        .and_then(|m| m.lock().ok())
                        .map(|attr| (*attr).hotspot)
                    {
                        trace!("Setting cursor {:?}", hotspot);
                        let ptr = ptr.pointer();
                        ptr.set_cursor(last_enter, Some(&cursor_surface), hotspot.x, hotspot.y);
                        self.client_state.multipool_ctr += 1;

                        if let Err(e) = write_and_attach_buffer(
                            buf.as_ref().unwrap(),
                            &cursor_surface,
                            self.client_state.multipool_ctr,
                            multipool,
                        ) {
                            error!("failed to attach buffer to cursor surface: {}", e);
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
    fn data_device_state(&self) -> &smithay::wayland::selection::data_device::DataDeviceState {
        &self.server_state.data_device_state
    }
}

impl ClientDndGrabHandler for GlobalState {
    fn started(&mut self, source: Option<WlDataSource>, icon: Option<WlSurface>, seat: Seat<Self>) {
        let seat = match self.server_state.seats.iter_mut().find(|s| s.server.seat == seat) {
            Some(s) => s,
            None => return,
        };

        if let Some(source) = source.as_ref() {
            seat.client.next_dnd_offer_is_mine = true;
            let metadata = with_source_metadata(&source, |metadata| metadata.clone()).unwrap();
            let mut actions = ClientDndAction::empty();
            if metadata.dnd_action.contains(DndAction::Copy) {
                actions |= ClientDndAction::Copy;
            }
            if metadata.dnd_action.contains(DndAction::Move) {
                actions |= ClientDndAction::Move;
            }
            if metadata.dnd_action.contains(DndAction::Ask) {
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
                    let renderer = if let Some(r) = self.space.renderer() {
                        r
                    } else {
                        tracing::error!("No renderer available");
                        return;
                    };
                    let client_egl_surface = unsafe {
                        ClientEglSurface::new(
                            WlEglSurface::new(client_surface.id(), 1, 1).unwrap(), /* TODO remove unwrap */
                            client_surface.clone(),
                        )
                    };

                    let egl_surface = Rc::new(unsafe {
                        EGLSurface::new(
                            &renderer.egl_context().display(),
                            renderer
                                .egl_context()
                                .pixel_format()
                                .expect("Failed to get pixel format from EGL context "),
                            renderer.egl_context().config_id(),
                            client_egl_surface,
                        )
                        .expect("Failed to create EGL Surface")
                    });

                    seat.client.dnd_icon = Some((
                        egl_surface,
                        client_surface.clone(),
                        OutputDamageTracker::new((1, 1), 1.0, Transform::Flipped180),
                        false,
                        Some(0),
                    ));
                }
            }
            seat.client.dnd_source = Some(dnd_source);
        }

        seat.server.dnd_source = source;
        seat.server.dnd_icon = icon;
    }

    fn dropped(&mut self, seat: Seat<Self>) {
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
impl ServerDndGrabHandler for GlobalState {
    fn send(&mut self, mime_type: String, fd: OwnedFd, seat: Seat<Self>) {
        let seat = match self.server_state.seats.iter().find(|s| s.server.seat == seat) {
            Some(s) => s,
            None => return,
        };
        if let Some(offer) = seat.client.dnd_offer.as_ref() {
            receive_to_fd(offer.inner(), mime_type, fd)
        }
    }

    fn finished(&mut self, seat: Seat<Self>) {
        let seat = match self.server_state.seats.iter_mut().find(|s| s.server.seat == seat) {
            Some(s) => s,
            None => return,
        };
        if let Some(offer) = seat.client.dnd_offer.take() {
            offer.finish();
        }
    }

    fn cancelled(&mut self, seat: Seat<Self>) {
        let seat = match self.server_state.seats.iter_mut().find(|s| s.server.seat == seat) {
            Some(s) => s,
            None => return,
        };
        if let Some(offer) = seat.client.dnd_offer.take() {
            offer.destroy();
        }
    }

    fn action(&mut self, action: DndAction, seat: Seat<Self>) {
        let seat = match self.server_state.seats.iter().find(|s| s.server.seat == seat) {
            Some(s) => s,
            None => return,
        };
        let mut c_action = ClientDndAction::empty();
        if action.contains(DndAction::Copy) {
            c_action |= ClientDndAction::Copy;
        }
        if action.contains(DndAction::Move) {
            c_action |= ClientDndAction::Move;
        }
        if action.contains(DndAction::Ask) {
            c_action |= ClientDndAction::Ask;
        }

        if let Some(offer) = seat.client.dnd_offer.as_ref() {
            offer.set_actions(c_action, c_action)
        }
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
            seat.client.copy_paste_source = Some(copy_paste_source);
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
