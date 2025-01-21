use std::rc::Rc;

use sctk::reexports::client::Proxy;
use smithay::{
    backend::{
        egl::EGLSurface,
        renderer::{damage::OutputDamageTracker, utils::on_commit_buffer_handler, Bind, Unbind},
    },
    delegate_compositor, delegate_shm,
    desktop::utils::bbox_from_surface_tree,
    reexports::wayland_server::protocol::{wl_buffer, wl_surface::WlSurface},
    utils::Transform,
    wayland::{
        buffer::BufferHandler,
        compositor::{get_role, CompositorHandler, CompositorState},
        shm::{ShmHandler, ShmState},
    },
};
use tracing::{error, info, trace};
use wayland_egl::WlEglSurface;

use crate::xdg_shell_wrapper::{
    client_state::WrapperClientCompositorState,
    shared_state::GlobalState,
    space::{ClientEglSurface, WrapperSpace},
};

impl CompositorHandler for GlobalState {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.server_state.compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        let dh = self.server_state.display_handle.clone();
        let role = get_role(surface);
        trace!("role: {:?} surface: {:?}", &role, &surface);

        if role == "xdg_toplevel".into() {
            on_commit_buffer_handler::<GlobalState>(surface);
            self.space.dirty_window(&dh, surface);
            // check for pending motion events and send them now
            if let Some((pending_event, pointer, iter_count)) =
                self.client_state.delayed_surface_motion.remove(surface)
            {
                if iter_count == self.iter_count {
                    self.client_state
                        .delayed_surface_motion
                        .insert(surface.clone(), (pending_event, pointer, iter_count));
                    return;
                }
                let conn = &self.client_state.connection.clone();
                self.pointer_frame_inner(conn, &pointer, &[pending_event]);
            }
        } else if role == "xdg_popup".into() {
            on_commit_buffer_handler::<GlobalState>(surface);
            self.server_state.popup_manager.commit(surface);
            self.space.dirty_popup(&dh, surface);
        } else if role == "dnd_icon".into() {
            info!("dnd_icon commit");
            // render dnd icon to the active dnd icon surface
            on_commit_buffer_handler::<GlobalState>(surface);
            let seat = match self
                .server_state
                .seats
                .iter_mut()
                .find(|s| s.server.dnd_icon.as_ref() == Some(surface))
            {
                Some(s) => s,
                None => {
                    error!("dnd icon received, but no seat found");
                    return;
                },
            };
            if let Some(c_icon) = seat.client.dnd_icon.as_mut() {
                let size = bbox_from_surface_tree(surface, (0, 0)).size;

                if let Some(renderer) = self.space.renderer() {
                    _ = renderer.unbind();

                    match c_icon.0.clone() {
                        Some(egl_surface) => {
                            _ = renderer.bind(egl_surface.clone());
                            if !egl_surface.resize(size.w.max(1), size.h.max(1), 0, 0) {
                                error!("Failed to resize egl surface");
                            }
                        },
                        None => {
                            let c_surface = &c_icon.1;
                            let client_egl_surface = unsafe {
                                ClientEglSurface::new(
                                    WlEglSurface::new(c_surface.id(), size.w.max(1), size.h.max(1))
                                        .unwrap(), // TODO remove unwrap
                                    c_surface.clone(),
                                )
                            };

                            let egl_surface = Rc::new(unsafe {
                                EGLSurface::new(
                                    renderer.egl_context().display(),
                                    renderer
                                        .egl_context()
                                        .pixel_format()
                                        .expect("Failed to get pixel format from EGL context "),
                                    renderer.egl_context().config_id(),
                                    client_egl_surface,
                                )
                                .expect("Failed to create EGL Surface")
                            });
                            _ = renderer.bind(egl_surface.clone());
                            c_icon.0 = Some(egl_surface);
                        },
                    };

                    let _ = renderer.unbind();
                    c_icon.2 = OutputDamageTracker::new(
                        (size.w.max(1), size.h.max(1)),
                        self.space.space_list[0].scale,
                        Transform::Flipped180,
                    );
                }

                c_icon.3 = true;
                c_icon.1.commit();
                c_icon.1.frame(&self.client_state.queue_handle, c_icon.1.clone());
            }
        } else {
            trace!("{:?}", surface);
        }
    }

    fn client_compositor_state<'a>(
        &self,
        client: &'a smithay::reexports::wayland_server::Client,
    ) -> &'a smithay::wayland::compositor::CompositorClientState {
        &client.get_data::<WrapperClientCompositorState>().unwrap().compositor_state
    }
}

impl BufferHandler for GlobalState {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for GlobalState {
    fn shm_state(&self) -> &ShmState {
        &self.server_state.shm_state
    }
}

delegate_compositor!(GlobalState);
delegate_shm!(GlobalState);
