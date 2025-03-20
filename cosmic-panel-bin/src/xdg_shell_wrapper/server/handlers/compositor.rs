use std::rc::Rc;

use sctk::{
    reexports::client::Proxy,
    shell::{
        wlr_layer::{self, Anchor, KeyboardInteractivity},
        WaylandSurface,
    },
};
use smithay::{
    backend::{
        egl::EGLSurface,
        renderer::{damage::OutputDamageTracker, utils::on_commit_buffer_handler, Bind},
    },
    delegate_compositor, delegate_shm,
    desktop::{utils::bbox_from_surface_tree, LayerSurface as SmithayLayerSurface},
    reexports::wayland_server::protocol::{wl_buffer, wl_surface::WlSurface},
    utils::{Logical, Size, Transform},
    wayland::{
        buffer::BufferHandler,
        compositor::{get_role, CompositorHandler, CompositorState},
        shell::wlr_layer::{ExclusiveZone, Layer},
        shm::{ShmHandler, ShmState},
    },
};
use tracing::{error, info, trace};
use wayland_egl::WlEglSurface;

use crate::xdg_shell_wrapper::{
    client_state::{SurfaceState, WrapperClientCompositorState},
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
        } else if role == "zwlr_layer_surface_v1".into() {
            if let Some(pos) = self
                .client_state
                .pending_layer_surfaces
                .iter()
                .position(|s| s.0.wl_surface() == surface)
            {
                let (surface, output, namespace) =
                    self.client_state.pending_layer_surfaces.swap_remove(pos);
                // layer created by client
                // request received here
                // layer created in compositor & tracked by xdg-shell-wrapper in its own space
                // that spans all outputs get renderer from wrapper space and
                // draw to it
                let renderer = match self.space.renderer() {
                    Some(r) => r,
                    None => return,
                };
                let mut size = surface.with_pending_state(|s| s.size).unwrap_or_default();
                let server_surface = SmithayLayerSurface::new(surface, namespace.clone());
                let state = server_surface.cached_state();
                let anchor = Anchor::from_bits(state.anchor.bits());

                if !state.anchor.anchored_horizontally() {
                    size.w = 1.max(size.w);
                }
                if !state.anchor.anchored_vertically() {
                    size.h = 1.max(size.h);
                }

                let output =
                    self.client_state.outputs.iter().find(|o| {
                        output.as_ref().map(|output| o.1.owns(output)).unwrap_or_default()
                    });
                let surface = self
                    .client_state
                    .compositor_state
                    .create_surface(&self.client_state.queue_handle);

                let exclusive_zone = match state.exclusive_zone {
                    ExclusiveZone::Exclusive(area) => area as i32,
                    ExclusiveZone::Neutral => 0,
                    ExclusiveZone::DontCare => -1,
                };
                let layer = match server_surface.layer() {
                    Layer::Background => wlr_layer::Layer::Background,
                    Layer::Bottom => wlr_layer::Layer::Bottom,
                    Layer::Top => wlr_layer::Layer::Top,
                    Layer::Overlay => wlr_layer::Layer::Overlay,
                };
                let interactivity = match state.keyboard_interactivity {
                    smithay::wayland::shell::wlr_layer::KeyboardInteractivity::None => {
                        KeyboardInteractivity::None
                    },
                    smithay::wayland::shell::wlr_layer::KeyboardInteractivity::Exclusive => {
                        KeyboardInteractivity::Exclusive
                    },
                    smithay::wayland::shell::wlr_layer::KeyboardInteractivity::OnDemand => {
                        KeyboardInteractivity::OnDemand
                    },
                };
                let client_surface = self.client_state.layer_state.create_layer_surface(
                    &self.client_state.queue_handle,
                    surface,
                    layer,
                    Some(namespace),
                    output.as_ref().map(|o| &o.0),
                );
                client_surface.set_margin(
                    state.margin.top,
                    state.margin.right,
                    state.margin.bottom,
                    state.margin.left,
                );
                client_surface.set_keyboard_interactivity(interactivity);
                client_surface.set_size(size.w as u32, size.h as u32);
                client_surface.set_exclusive_zone(exclusive_zone);
                if let Some(anchor) = anchor {
                    client_surface.set_anchor(anchor);
                }

                client_surface.commit();
                let client_egl_surface = unsafe {
                    ClientEglSurface::new(
                        WlEglSurface::new(
                            client_surface.wl_surface().id(),
                            size.w.max(1),
                            size.h.max(1),
                        )
                        .unwrap(), // TODO remove unwrap
                        client_surface.wl_surface().clone(),
                    )
                };

                let egl_surface = unsafe {
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
                };

                let surface = client_surface.wl_surface();
                let scale = self
                    .client_state
                    .fractional_scaling_manager
                    .as_ref()
                    .map(|f| f.fractional_scaling(surface, &self.client_state.queue_handle));
                let viewport = self.client_state.viewporter_state.as_ref().map(|v| {
                    let v = v.get_viewport(surface, &self.client_state.queue_handle);
                    if size.w > 0 && size.h > 0 {
                        v.set_destination(size.w, size.h);
                    }
                    v
                });
                self.client_state.proxied_layer_surfaces.push((
                    egl_surface,
                    OutputDamageTracker::new(
                        (size.w.max(1), size.h.max(1)),
                        1.,
                        Transform::Flipped180,
                    ),
                    server_surface,
                    client_surface,
                    SurfaceState::Waiting(0, size),
                    1.0,
                    scale,
                    viewport,
                ));
            }
            if let Some((
                egl_surface,
                renderer,
                s_layer_surface,
                c_layer_surface,
                state,
                scale,
                _,
                viewport,
            )) = self
                .client_state
                .proxied_layer_surfaces
                .iter_mut()
                .find(|s| s.2.wl_surface() == surface)
            {
                let old_size = s_layer_surface.bbox().size;
                on_commit_buffer_handler::<GlobalState>(surface);
                let size = s_layer_surface.bbox().size;
                let scaled_size = size.to_f64().to_physical_precise_round(*scale);

                if size.w <= 0 || size.h <= 0 {
                    return;
                }
                if let Some(viewport) = viewport {
                    viewport.set_destination(size.w, size.h);
                }
                let generation = match state {
                    SurfaceState::WaitingFirst(..) => return,
                    SurfaceState::Waiting(gen, _) => *gen,
                    SurfaceState::Dirty(gen) => *gen,
                };
                if let Some(gles_renderer) = self.space.renderer() {
                    if old_size != size {
                        tracing::trace!("Layer surface update. old: {old_size:?}, new: {size:?}, generation: {generation}");
                        _ = unsafe {
                            gles_renderer.egl_context().make_current_with_surface(egl_surface)
                        };
                        egl_surface.resize(scaled_size.w, scaled_size.h, 0, 0);
                        c_layer_surface.set_size(size.w as u32, size.h as u32);
                        *renderer =
                            OutputDamageTracker::new(scaled_size, *scale, Transform::Flipped180);
                        c_layer_surface.wl_surface().commit();
                        *state = if old_size.w == 0 || old_size.h == 0 {
                            SurfaceState::Dirty(generation)
                        } else {
                            SurfaceState::Waiting(generation.wrapping_add(1), size)
                        };
                    } else {
                        *state = SurfaceState::Dirty(generation);
                    }
                }
            }
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
                    match c_icon.0.as_mut() {
                        Some(egl_surface) => {
                            _ = unsafe {
                                renderer.egl_context().make_current_with_surface(egl_surface)
                            };
                            _ = renderer.bind(egl_surface);
                            if !egl_surface.resize(size.w.max(1), size.h.max(1), 0, 0) {
                                error!("Failed to resize egl surface");
                            }
                        },
                        None => {
                            let c_surface = &c_icon.1;
                            let client_egl_surface = unsafe {
                                ClientEglSurface::new(
                                    WlEglSurface::new(c_surface.id(), size.w.max(1), size.h.max(1))
                                        .unwrap(), /* TODO remove unwrap */
                                    c_surface.clone(),
                                )
                            };

                            let mut egl_surface = unsafe {
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
                            };
                            _ = unsafe {
                                renderer.egl_context().make_current_with_surface(&egl_surface)
                            };
                            _ = renderer.bind(&mut egl_surface);
                            c_icon.0 = Some(egl_surface);
                        },
                    };

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
        } else if role == "subsurface".into() {
            on_commit_buffer_handler::<GlobalState>(surface);
            self.space.dirty_subsurface(
                &self.client_state.compositor_state,
                &self.client_state.subcompositor,
                self.client_state.fractional_scaling_manager.as_ref(),
                self.client_state.viewporter_state.as_ref(),
                &self.client_state.queue_handle,
                surface,
            );
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
