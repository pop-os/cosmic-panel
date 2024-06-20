// SPDX-License-Identifier: MPL-2.0

use std::time::Duration;

use itertools::Itertools;
use sctk::{
    reexports::client::protocol::{wl_output as c_wl_output, wl_surface::WlSurface},
    shell::WaylandSurface,
};
use smithay::{
    backend::renderer::{
        element::surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
        gles::GlesRenderer,
        Bind, ImportDma, ImportEgl, Unbind,
    },
    desktop::utils::send_frames_surface_tree,
    output::Output,
    reexports::wayland_server::{backend::GlobalId, DisplayHandle},
    wayland::{
        compositor::with_states, dmabuf::DmabufState, fractional_scale::with_fractional_scale,
    },
};
use tracing::error;

use crate::{
    space_container::SpaceContainer,
    xdg_shell_wrapper::{
        client_state::ClientState, server_state::ServerState, space::WrapperSpace,
    },
};

/// group of info for an output
pub type OutputGroup = (Output, GlobalId, String, c_wl_output::WlOutput);

/// the  global state for the embedded server state
#[allow(missing_debug_implementations)]
pub struct GlobalState {
    /// the implemented space
    pub space: SpaceContainer,
    /// desktop client state
    pub client_state: ClientState,
    /// embedded server state
    pub server_state: ServerState,
    /// instant that the panel was started
    pub start_time: std::time::Instant,
}

impl GlobalState {
    pub(crate) fn new(
        client_state: ClientState,
        server_state: ServerState,
        space: SpaceContainer,
        start_time: std::time::Instant,
    ) -> Self {
        Self { space, client_state, server_state, start_time }
    }

    /// set the scale factor for a surface
    /// this should be called when the scale factor of a surface changes
    pub fn scale_factor_changed(&mut self, surface: &WlSurface, scale_factor: f64, legacy: bool) {
        if legacy && self.client_state.fractional_scaling_manager.is_some() {
            return;
        }
        for tracked_surface in &mut self.client_state.proxied_layer_surfaces {
            if tracked_surface.3.wl_surface() == surface {
                if legacy {
                    surface.set_buffer_scale(scale_factor as i32);
                }
                tracked_surface.5 = scale_factor;
                with_states(tracked_surface.2.wl_surface(), |states| {
                    with_fractional_scale(states, |fractional_scale| {
                        fractional_scale.set_preferred_scale(scale_factor);
                    });
                });
                return;
            }
        }

        self.space.scale_factor_changed(surface, scale_factor, legacy);
    }
}

impl GlobalState {
    /// bind the display for the space
    pub fn bind_display(&mut self, dh: &DisplayHandle) {
        if let Some(renderer) = self.space.renderer() {
            let res = renderer.bind_wl_display(dh);
            if let Err(err) = res {
                error!("{:?}", err);
            } else {
                let dmabuf_formats = renderer.dmabuf_formats().into_iter().collect_vec();
                let mut state = DmabufState::new();
                let global = state.create_global::<GlobalState>(dh, dmabuf_formats);
                self.server_state.dmabuf_state.replace((state, global));
            }
        }
    }

    /// draw the dnd icon if it exists and is ready
    pub fn draw_dnd_icon(&mut self) {
        // TODO proxied layer surfaces
        if let Some(dnd_icon) = self
            .server_state
            .seats
            .iter_mut()
            .find(|s| s.client.dnd_icon.is_some() && s.server.dnd_icon.is_some())
        {
            let (egl_surface, wl_surface, ref mut dmg_tracked_renderer, is_dirty, has_frame) =
                dnd_icon.client.dnd_icon.as_mut().unwrap();
            if !*is_dirty || !has_frame.is_some() {
                return;
            }
            *is_dirty = false;
            let time = has_frame.take().unwrap();
            let clear_color = &[0.0, 0.0, 0.0, 0.0];
            let renderer = match self.space.renderer() {
                Some(r) => r,
                None => {
                    error!("no renderer");
                    return;
                },
            };
            let s_icon = dnd_icon.server.dnd_icon.as_ref().unwrap();
            let _ = renderer.unbind();
            let _ = renderer.bind(egl_surface.clone());
            let elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                render_elements_from_surface_tree(
                    renderer,
                    s_icon,
                    (1, 1),
                    1.0,
                    1.0,
                    smithay::backend::renderer::element::Kind::Unspecified,
                );
            dmg_tracked_renderer
                .render_output(
                    renderer,
                    egl_surface.buffer_age().unwrap_or_default() as usize,
                    &elements,
                    *clear_color,
                )
                .unwrap();
            egl_surface.swap_buffers(None).unwrap();
            // FIXME: damage tracking issues on integrated graphics but not nvidia
            // self.egl_surface
            //     .as_ref()
            //     .unwrap()
            //     .swap_buffers(res.0.as_deref_mut())?;

            let _ = renderer.unbind();
            // // TODO what if there is "no output"?
            for o in &self.client_state.outputs {
                let output = &o.1;
                send_frames_surface_tree(
                    s_icon,
                    &o.1,
                    Duration::from_millis(time as u64),
                    None,
                    move |_, _| Some(output.clone()),
                );
            }
            wl_surface.frame(&self.client_state.queue_handle, wl_surface.clone());
            wl_surface.commit();
        }
    }
}
