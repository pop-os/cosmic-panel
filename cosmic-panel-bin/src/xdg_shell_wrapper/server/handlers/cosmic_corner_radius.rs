use cctk::wayland_client::Proxy;
// proxy requests from clients for popups or layer surfaces
use cosmic_protocols::corner_radius::v1::server::cosmic_corner_radius_layer_v1::{
    self, CosmicCornerRadiusLayerV1,
};
use cosmic_protocols::corner_radius::v1::server::cosmic_corner_radius_toplevel_v1::CosmicCornerRadiusToplevelV1;
use cosmic_protocols::corner_radius::v1::server::{
    cosmic_corner_radius_manager_v1, cosmic_corner_radius_toplevel_v1,
};
use sctk::shell::wlr_layer::SurfaceKind;
use smithay::desktop::utils::bbox_from_surface_tree;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_popup::XdgPopup;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::XdgToplevel;
use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::server::zwlr_layer_surface_v1::ZwlrLayerSurfaceV1;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{
    Client, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, Weak,
};
use smithay::utils::{HookId, Logical, Point, Rectangle};
use smithay::wayland::compositor::{Cacheable, add_pre_commit_hook, with_states};
use smithay::wayland::shell::wlr_layer::WlrLayerShellHandler;
use smithay::wayland::shell::xdg::{SurfaceCachedState, XdgShellHandler, XdgShellSurfaceUserData};
use std::sync::Mutex;
use wayland_backend::server::GlobalId;

use crate::xdg_shell_wrapper::shared_state::GlobalState;

type ToplevelHookId = Mutex<Option<(HookId, Weak<CosmicCornerRadiusToplevelV1>)>>;
type LayerHookId = Mutex<Option<(HookId, Weak<CosmicCornerRadiusLayerV1>)>>;

#[derive(Debug)]
pub struct CornerRadiusState {
    instances: Vec<cosmic_corner_radius_manager_v1::CosmicCornerRadiusManagerV1>,
    global: GlobalId,
}

impl CornerRadiusState {
    pub fn new<D>(dh: &DisplayHandle) -> CornerRadiusState
    where
        D: GlobalDispatch<cosmic_corner_radius_manager_v1::CosmicCornerRadiusManagerV1, ()>
            + Dispatch<cosmic_corner_radius_manager_v1::CosmicCornerRadiusManagerV1, ()>
            + Dispatch<
                cosmic_corner_radius_toplevel_v1::CosmicCornerRadiusToplevelV1,
                CornerRadiusData,
            > + Dispatch<cosmic_corner_radius_layer_v1::CosmicCornerRadiusLayerV1, CornerRadiusData>
            + CornerRadiusHandler
            + 'static,
    {
        let global = dh
            .create_global::<D, cosmic_corner_radius_manager_v1::CosmicCornerRadiusManagerV1, _>(
                2,
                (),
            );
        CornerRadiusState { instances: Vec::new(), global }
    }
}

pub trait CornerRadiusHandler: XdgShellHandler + WlrLayerShellHandler {
    fn corner_radius_state(&mut self) -> &mut CornerRadiusState;
    fn commit_xdg(&mut self, corners: CacheableCorners, wl_surface: &WlSurface);
    fn commit_wlr(
        &mut self,
        corners: CacheableCorners,
        padding: CacheablePadding,
        wl_surface: &WlSurface,
    );
}

impl<D> GlobalDispatch<cosmic_corner_radius_manager_v1::CosmicCornerRadiusManagerV1, (), D>
    for CornerRadiusState
where
    D: GlobalDispatch<cosmic_corner_radius_manager_v1::CosmicCornerRadiusManagerV1, ()>
        + Dispatch<cosmic_corner_radius_manager_v1::CosmicCornerRadiusManagerV1, ()>
        + Dispatch<cosmic_corner_radius_toplevel_v1::CosmicCornerRadiusToplevelV1, CornerRadiusData>
        + Dispatch<cosmic_corner_radius_layer_v1::CosmicCornerRadiusLayerV1, CornerRadiusData>
        + CornerRadiusHandler
        + 'static,
{
    fn bind(
        state: &mut D,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: smithay::reexports::wayland_server::New<
            cosmic_corner_radius_manager_v1::CosmicCornerRadiusManagerV1,
        >,
        _global_data: &(),
        data_init: &mut smithay::reexports::wayland_server::DataInit<'_, D>,
    ) {
        let instance = data_init.init(resource, ());
        state.corner_radius_state().instances.push(instance);
    }
}

impl<D> Dispatch<cosmic_corner_radius_manager_v1::CosmicCornerRadiusManagerV1, (), D>
    for CornerRadiusState
where
    D: GlobalDispatch<cosmic_corner_radius_manager_v1::CosmicCornerRadiusManagerV1, ()>
        + Dispatch<cosmic_corner_radius_manager_v1::CosmicCornerRadiusManagerV1, ()>
        + Dispatch<cosmic_corner_radius_toplevel_v1::CosmicCornerRadiusToplevelV1, CornerRadiusData>
        + Dispatch<cosmic_corner_radius_layer_v1::CosmicCornerRadiusLayerV1, CornerRadiusData>
        + CornerRadiusHandler
        + 'static,
{
    fn request(
        state: &mut D,
        _client: &Client,
        resource: &cosmic_corner_radius_manager_v1::CosmicCornerRadiusManagerV1,
        request: <cosmic_corner_radius_manager_v1::CosmicCornerRadiusManagerV1 as smithay::reexports::wayland_server::Resource>::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut smithay::reexports::wayland_server::DataInit<'_, D>,
    ) {
        match request {
            cosmic_corner_radius_manager_v1::Request::Destroy => {
                let corner_radius_state = state.corner_radius_state();
                corner_radius_state.instances.retain(|i| i != resource);
            },
            cosmic_corner_radius_manager_v1::Request::GetCornerRadius { id, toplevel } => {
                if let Some(surface) = state.xdg_shell_state().get_toplevel(&toplevel) {
                    new_xdg(
                        surface.wl_surface(),
                        CornerRadiusSurface::Toplevel(surface.xdg_toplevel().downgrade()),
                        resource,
                        data_init,
                        id,
                    )
                } // TODO: can this fail?
            },
            cosmic_corner_radius_manager_v1::Request::GetCornerRadiusSurface { id, surface } => {
                if let Some(toplevel) =
                    state.xdg_shell_state().toplevel_surfaces().iter().find(|toplevel| {
                        toplevel
                            .xdg_toplevel()
                            .data::<XdgShellSurfaceUserData>()
                            .unwrap()
                            .xdg_surface()
                            == &surface
                    })
                {
                    new_xdg(
                        toplevel.wl_surface(),
                        CornerRadiusSurface::Toplevel(toplevel.xdg_toplevel().downgrade()),
                        resource,
                        data_init,
                        id,
                    )
                } else if let Some(popup) =
                    state.xdg_shell_state().popup_surfaces().iter().find(|popup| {
                        popup.xdg_popup().data::<XdgShellSurfaceUserData>().unwrap().xdg_surface()
                            == &surface
                    })
                {
                    new_xdg(
                        popup.wl_surface(),
                        CornerRadiusSurface::Popup(popup.xdg_popup().downgrade()),
                        resource,
                        data_init,
                        id,
                    )
                }
            },
            cosmic_corner_radius_manager_v1::Request::GetCornerRadiusLayer { id, layer } => {
                if let Some(surface) = state
                    .shell_state()
                    .layer_surfaces()
                    .find(|surface| surface.shell_surface() == &layer)
                {
                    let radius_exists = with_states(surface.wl_surface(), |surface_data| {
                        let hook_id = surface_data
                            .data_map
                            .get_or_insert_threadsafe(|| ToplevelHookId::new(None));
                        let guard = hook_id.lock().unwrap();
                        guard.as_ref().map(|(_, t)| t.upgrade().is_ok())
                    });
                    if radius_exists.unwrap_or_default() {
                        resource.post_error(
                            cosmic_corner_radius_manager_v1::Error::CornerRadiusExists as u32,
                            format!(
                                "{resource:?} CosmicCornerRadiusToplevelV1 object already exists \
                                 for the surface"
                            ),
                        );
                    }
                    let data = Mutex::new(CornerRadiusInternal {
                        surface: CornerRadiusSurface::Layer(layer.downgrade()),
                        corners: None,
                        padding: None,
                    });
                    let obj = data_init.init(id, data);
                    let obj_downgrade = obj.downgrade();

                    let needs_hook = radius_exists.is_none();
                    if needs_hook {
                        let hook_id =
                            add_pre_commit_hook::<D, _>(surface.wl_surface(), layer_radius_hook);
                        with_states(surface.wl_surface(), |surface_data| {
                            let hook_ids = surface_data
                                .data_map
                                .get_or_insert_threadsafe(|| LayerHookId::new(None));
                            let mut guard = hook_ids.lock().unwrap();
                            *guard = Some((hook_id, obj_downgrade));
                        });
                    }
                } // TODO: can this fail?
            },
            _ => unimplemented!(),
        }
    }

    fn destroyed(
        state: &mut D,
        _client: wayland_backend::server::ClientId,
        resource: &cosmic_corner_radius_manager_v1::CosmicCornerRadiusManagerV1,
        _data: &(),
    ) {
        let corner_radius_state = state.corner_radius_state();
        corner_radius_state.instances.retain(|i| i != resource);
    }
}

fn new_xdg<D>(
    wl_surface: &WlSurface,
    surface: CornerRadiusSurface,
    resource: &cosmic_corner_radius_manager_v1::CosmicCornerRadiusManagerV1,
    data_init: &mut smithay::reexports::wayland_server::DataInit<'_, D>,
    id: New<cosmic_corner_radius_toplevel_v1::CosmicCornerRadiusToplevelV1>,
) where
    D: GlobalDispatch<cosmic_corner_radius_manager_v1::CosmicCornerRadiusManagerV1, ()>
        + Dispatch<cosmic_corner_radius_manager_v1::CosmicCornerRadiusManagerV1, ()>
        + Dispatch<cosmic_corner_radius_toplevel_v1::CosmicCornerRadiusToplevelV1, CornerRadiusData>
        + CornerRadiusHandler
        + 'static,
{
    let radius_exists = with_states(wl_surface, |surface_data| {
        let hook_id = surface_data.data_map.get_or_insert_threadsafe(|| ToplevelHookId::new(None));
        let guard = hook_id.lock().unwrap();
        guard.as_ref().map(|(_, t)| t.upgrade().is_ok())
    });
    if radius_exists.unwrap_or_default() {
        resource.post_error(
            cosmic_corner_radius_manager_v1::Error::CornerRadiusExists as u32,
            format!(
                "{resource:?} CosmicCornerRadiusToplevelV1 object already exists for the surface"
            ),
        );
    }
    let data = Mutex::new(CornerRadiusInternal { surface, corners: None, padding: None });
    let obj = data_init.init(id, data);
    let obj_downgrade = obj.downgrade();

    let needs_hook = radius_exists.is_none();
    if needs_hook {
        let hook_id = add_pre_commit_hook::<D, _>(wl_surface, xdg_radius_hook);
        with_states(wl_surface, |surface_data| {
            let hook_ids =
                surface_data.data_map.get_or_insert_threadsafe(|| ToplevelHookId::new(None));
            let mut guard = hook_ids.lock().unwrap();
            *guard = Some((hook_id, obj_downgrade));
        });
    }
}

impl<D>
    Dispatch<cosmic_corner_radius_toplevel_v1::CosmicCornerRadiusToplevelV1, CornerRadiusData, D>
    for CornerRadiusState
where
    D: GlobalDispatch<cosmic_corner_radius_manager_v1::CosmicCornerRadiusManagerV1, ()>
        + Dispatch<cosmic_corner_radius_manager_v1::CosmicCornerRadiusManagerV1, ()>
        + Dispatch<cosmic_corner_radius_toplevel_v1::CosmicCornerRadiusToplevelV1, CornerRadiusData>
        + CornerRadiusHandler
        + 'static,
{
    fn request(
        state: &mut D,
        _client: &Client,
        resource: &cosmic_corner_radius_toplevel_v1::CosmicCornerRadiusToplevelV1,
        request: <cosmic_corner_radius_toplevel_v1::CosmicCornerRadiusToplevelV1 as Resource>::Request,
        data: &CornerRadiusData,
        _dhandle: &DisplayHandle,
        _data_init: &mut smithay::reexports::wayland_server::DataInit<'_, D>,
    ) {
        match request {
            cosmic_corner_radius_toplevel_v1::Request::Destroy => {
                let mut guard = data.lock().unwrap();
                guard.corners = None;

                let Some(wl_surface) = (match &guard.surface {
                    CornerRadiusSurface::Toplevel(toplevel) => toplevel
                        .upgrade()
                        .ok()
                        .and_then(|toplevel| state.xdg_shell_state().get_toplevel(&toplevel))
                        .map(|toplevel| toplevel.wl_surface().clone()),
                    CornerRadiusSurface::Popup(popup) => popup
                        .upgrade()
                        .ok()
                        .and_then(|popup| state.xdg_shell_state().get_popup(&popup))
                        .map(|popup| popup.wl_surface().clone()),
                    CornerRadiusSurface::Layer(_) => unreachable!(),
                }) else {
                    return;
                };

                with_states(&wl_surface, |surface_data| {
                    if let Some(hook_ids_mutex) = surface_data.data_map.get::<ToplevelHookId>() {
                        let mut hook_id = hook_ids_mutex.lock().unwrap();
                        *hook_id = None;
                    }

                    let mut cached = surface_data.cached_state.get::<CacheableCorners>();
                    let pending = cached.pending();
                    *pending = CacheableCorners(None);
                });
                drop(guard);
            },
            cosmic_corner_radius_toplevel_v1::Request::SetRadius {
                top_left,
                top_right,
                bottom_right,
                bottom_left,
            } => {
                let mut guard = data.lock().unwrap();
                guard.set_corner_radius(top_left, top_right, bottom_right, bottom_left);

                let Some(wl_surface) = (match &guard.surface {
                    CornerRadiusSurface::Toplevel(toplevel) => toplevel
                        .upgrade()
                        .ok()
                        .and_then(|toplevel| state.xdg_shell_state().get_toplevel(&toplevel))
                        .map(|toplevel| toplevel.wl_surface().clone()),
                    CornerRadiusSurface::Popup(popup) => popup
                        .upgrade()
                        .ok()
                        .and_then(|popup| state.xdg_shell_state().get_popup(&popup))
                        .map(|popup| popup.wl_surface().clone()),
                    CornerRadiusSurface::Layer(_) => unreachable!(),
                }) else {
                    resource.post_error(
                        cosmic_corner_radius_toplevel_v1::Error::ToplevelDestroyed as u32,
                        format!("{:?} No toplevel found", resource),
                    );
                    return;
                };
                with_states(&wl_surface, |s| {
                    let mut cached = s.cached_state.get::<CacheableCorners>();
                    let pending = cached.pending();
                    *pending = CacheableCorners(guard.corners);
                });
                drop(guard);
            },
            cosmic_corner_radius_toplevel_v1::Request::UnsetRadius => {
                let mut guard = data.lock().unwrap();
                guard.corners = None;

                let Some(wl_surface) = (match &guard.surface {
                    CornerRadiusSurface::Toplevel(toplevel) => toplevel
                        .upgrade()
                        .ok()
                        .and_then(|toplevel| state.xdg_shell_state().get_toplevel(&toplevel))
                        .map(|toplevel| toplevel.wl_surface().clone()),
                    CornerRadiusSurface::Popup(popup) => popup
                        .upgrade()
                        .ok()
                        .and_then(|popup| state.xdg_shell_state().get_popup(&popup))
                        .map(|popup| popup.wl_surface().clone()),
                    CornerRadiusSurface::Layer(_) => unreachable!(),
                }) else {
                    resource.post_error(
                        cosmic_corner_radius_toplevel_v1::Error::ToplevelDestroyed as u32,
                        format!("{:?} No toplevel found", resource),
                    );

                    return;
                };

                with_states(&wl_surface, |s| {
                    let mut cached = s.cached_state.get::<CacheableCorners>();
                    let pending = cached.pending();
                    *pending = CacheableCorners(None);
                });
                drop(guard);
            },
            _ => unimplemented!(),
        }
    }
}

impl<D> Dispatch<cosmic_corner_radius_layer_v1::CosmicCornerRadiusLayerV1, CornerRadiusData, D>
    for CornerRadiusState
where
    D: GlobalDispatch<cosmic_corner_radius_manager_v1::CosmicCornerRadiusManagerV1, ()>
        + Dispatch<cosmic_corner_radius_manager_v1::CosmicCornerRadiusManagerV1, ()>
        + Dispatch<cosmic_corner_radius_layer_v1::CosmicCornerRadiusLayerV1, CornerRadiusData>
        + CornerRadiusHandler
        + 'static,
{
    fn request(
        state: &mut D,
        _client: &Client,
        resource: &cosmic_corner_radius_layer_v1::CosmicCornerRadiusLayerV1,
        request: <cosmic_corner_radius_layer_v1::CosmicCornerRadiusLayerV1 as Resource>::Request,
        data: &CornerRadiusData,
        _dhandle: &DisplayHandle,
        _data_init: &mut smithay::reexports::wayland_server::DataInit<'_, D>,
    ) {
        match request {
            cosmic_corner_radius_layer_v1::Request::Destroy => {
                let mut guard = data.lock().unwrap();
                guard.corners = None;

                let CornerRadiusSurface::Layer(layer_surface) = &guard.surface else {
                    unreachable!("corner_radius_layer without layer shell?");
                };
                let Some(layer_surface) = layer_surface.upgrade().ok().and_then(|layer| {
                    state.shell_state().layer_surfaces().find(|s| s.shell_surface() == &layer)
                }) else {
                    resource.post_error(
                        cosmic_corner_radius_layer_v1::Error::LayerDestroyed as u32,
                        format!("{:?} No layer found", resource),
                    );
                    return;
                };

                with_states(layer_surface.wl_surface(), |surface_data| {
                    if let Some(hook_ids_mutex) = surface_data.data_map.get::<LayerHookId>() {
                        let mut hook_id = hook_ids_mutex.lock().unwrap();
                        *hook_id = None;
                    }

                    let mut cached = surface_data.cached_state.get::<CacheableCorners>();
                    let pending = cached.pending();
                    *pending = CacheableCorners(None);

                    let mut cached = surface_data.cached_state.get::<CacheablePadding>();
                    let pending = cached.pending();
                    *pending = CacheablePadding(None);
                });
                drop(guard);
            },
            cosmic_corner_radius_layer_v1::Request::SetRadius {
                top_left,
                top_right,
                bottom_right,
                bottom_left,
            } => {
                let mut guard = data.lock().unwrap();
                guard.set_corner_radius(top_left, top_right, bottom_right, bottom_left);

                let CornerRadiusSurface::Layer(layer_surface) = &guard.surface else {
                    unreachable!("corner_radius_layer without layer shell?");
                };
                let Some(layer_surface) = layer_surface.upgrade().ok().and_then(|layer| {
                    state.shell_state().layer_surfaces().find(|s| s.shell_surface() == &layer)
                }) else {
                    resource.post_error(
                        cosmic_corner_radius_layer_v1::Error::LayerDestroyed as u32,
                        format!("{:?} No layer found", resource),
                    );
                    return;
                };

                with_states(layer_surface.wl_surface(), |s| {
                    let mut cached = s.cached_state.get::<CacheableCorners>();
                    let pending = cached.pending();
                    *pending = CacheableCorners(guard.corners);
                });
                drop(guard);
            },
            cosmic_corner_radius_layer_v1::Request::UnsetRadius => {
                let mut guard = data.lock().unwrap();
                guard.corners = None;

                let CornerRadiusSurface::Layer(layer_surface) = &guard.surface else {
                    unreachable!("corner_radius_layer without layer shell?");
                };
                let Some(layer_surface) = layer_surface.upgrade().ok().and_then(|layer| {
                    state.shell_state().layer_surfaces().find(|s| s.shell_surface() == &layer)
                }) else {
                    resource.post_error(
                        cosmic_corner_radius_layer_v1::Error::LayerDestroyed as u32,
                        format!("{:?} No layer found", resource),
                    );
                    return;
                };

                with_states(layer_surface.wl_surface(), |s| {
                    let mut cached = s.cached_state.get::<CacheableCorners>();
                    let pending = cached.pending();
                    *pending = CacheableCorners(None);
                });
                drop(guard);
            },
            cosmic_corner_radius_layer_v1::Request::SetPadding { top, right, bottom, left } => {
                let mut guard = data.lock().unwrap();
                guard.set_padding(top, right, bottom, left);

                let CornerRadiusSurface::Layer(layer_surface) = &guard.surface else {
                    unreachable!("corner_radius_layer without layer shell?");
                };
                let Some(layer_surface) = layer_surface.upgrade().ok().and_then(|layer| {
                    state.shell_state().layer_surfaces().find(|s| s.shell_surface() == &layer)
                }) else {
                    resource.post_error(
                        cosmic_corner_radius_layer_v1::Error::LayerDestroyed as u32,
                        format!("{:?} No layer found", resource),
                    );
                    return;
                };

                with_states(layer_surface.wl_surface(), |s| {
                    let mut cached = s.cached_state.get::<CacheablePadding>();
                    let pending = cached.pending();
                    *pending = CacheablePadding(guard.padding);
                });
                drop(guard);
            },
            cosmic_corner_radius_layer_v1::Request::UnsetPadding => {
                let mut guard = data.lock().unwrap();
                guard.corners = None;

                let CornerRadiusSurface::Layer(layer_surface) = &guard.surface else {
                    unreachable!("corner_radius_layer without layer shell?");
                };
                let Some(layer_surface) = layer_surface.upgrade().ok().and_then(|layer| {
                    state.shell_state().layer_surfaces().find(|s| s.shell_surface() == &layer)
                }) else {
                    resource.post_error(
                        cosmic_corner_radius_layer_v1::Error::LayerDestroyed as u32,
                        format!("{:?} No layer found", resource),
                    );
                    return;
                };

                with_states(layer_surface.wl_surface(), |s| {
                    let mut cached = s.cached_state.get::<CacheablePadding>();
                    let pending = cached.pending();
                    *pending = CacheablePadding(None);
                });
                drop(guard);
            },
            _ => unimplemented!(),
        }
    }
}

fn xdg_radius_hook<D: 'static + CornerRadiusHandler>(
    state: &mut D,
    _dh: &DisplayHandle,
    surface: &WlSurface,
) {
    if let Some(corners) = with_states(surface, |surface_data| {
        let mut corners = *surface_data.cached_state.get::<CacheableCorners>().pending();
        // Geometry and corner-radius are independently double-buffered Wayland
        // state, so a transient mismatch during resize (radius committed against
        // a since-shrunk surface) is expected, not a protocol violation worth a
        // fatal disconnect. Clamp instead of post_error()'ing the client away.
        // See: https://github.com/pop-os/cosmic-epoch/issues/3711
        if let Some((geo, c)) = surface_data
            .cached_state
            .get::<SurfaceCachedState>()
            .pending()
            .geometry
            .zip(corners.0.as_mut())
        {
            let half_min_dim = (geo.size.w.min(geo.size.h).max(0) / 2) as u32;
            c.top_left = c.top_left.min(half_min_dim);
            c.top_right = c.top_right.min(half_min_dim);
            c.bottom_right = c.bottom_right.min(half_min_dim);
            c.bottom_left = c.bottom_left.min(half_min_dim);
        }
        Some(corners)
    }) {
        state.commit_xdg(corners, surface);
    }
}

fn pad_rect(
    mut rect: Rectangle<i32, Logical>,
    padding: &Padding,
) -> Option<Rectangle<i32, Logical>> {
    rect.size.h = rect.size.h.checked_sub(padding.top)?;
    rect.loc.x += padding.left;
    rect.size.w = rect.size.w.checked_sub(padding.left)?;
    rect.loc.y += padding.top;
    rect.size.h = rect.size.h.checked_sub(padding.bottom)?;
    rect.size.w = rect.size.w.checked_sub(padding.right)?;
    Some(rect)
}

fn layer_radius_hook<D: 'static + CornerRadiusHandler>(
    state: &mut D,
    _dh: &DisplayHandle,
    surface: &WlSurface,
) {
    let bbox = bbox_from_surface_tree(surface, Point::default());
    if let Some((corners, padding)) = with_states(surface, |surface_data| {
        let mut corners = *surface_data.cached_state.get::<CacheableCorners>().pending();
        let padding = *surface_data.cached_state.get::<CacheablePadding>().pending();
        let empty = Padding::default();
        let Some(padded_box) = pad_rect(bbox, padding.0.as_ref().unwrap_or(&empty)) else {
            if let Some(hook) = surface_data.data_map.get::<LayerHookId>() {
                let hook_ref = hook.lock().unwrap();
                if let Some((_, obj)) = hook_ref.as_ref()
                    && let Ok(obj) = obj.upgrade()
                {
                    obj.post_error(
                        cosmic_corner_radius_layer_v1::Error::PaddingTooLarge as u32,
                        format!("{obj:?} padding too large"),
                    );
                }
            }
            return None;
        };

        // Geometry and corner-radius are independently double-buffered Wayland
        // state, so a transient mismatch during resize is expected, not a
        // protocol violation worth a fatal disconnect. Clamp instead of
        // post_error()'ing the client away.
        // See: https://github.com/pop-os/cosmic-epoch/issues/3711
        if let Some(c) = corners.0.as_mut() {
            let half_min_dim = (padded_box.size.w.min(padded_box.size.h).max(0) / 2) as u32;
            c.top_left = c.top_left.min(half_min_dim);
            c.top_right = c.top_right.min(half_min_dim);
            c.bottom_right = c.bottom_right.min(half_min_dim);
            c.bottom_left = c.bottom_left.min(half_min_dim);
        }
        Some((corners, padding))
    }) {
        state.commit_wlr(corners, padding, surface);
    }
}

pub type CornerRadiusData = Mutex<CornerRadiusInternal>;

#[derive(Debug)]
pub enum CornerRadiusSurface {
    Toplevel(Weak<XdgToplevel>),
    Popup(Weak<XdgPopup>),
    Layer(Weak<ZwlrLayerSurfaceV1>),
}

#[derive(Debug)]
pub struct CornerRadiusInternal {
    pub surface: CornerRadiusSurface,
    pub corners: Option<Corners>,
    pub padding: Option<Padding>,
}

#[derive(Debug, Copy, Clone)]
pub struct Corners {
    pub top_left: u32,
    pub top_right: u32,
    pub bottom_right: u32,
    pub bottom_left: u32,
}

#[derive(Debug, Copy, Clone, Default)]
pub struct Padding {
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub left: i32,
}

#[derive(Default, Debug, Copy, Clone)]
pub struct CacheableCorners(pub Option<Corners>);

impl Cacheable for CacheableCorners {
    fn commit(&mut self, _dh: &DisplayHandle) -> Self {
        *self
    }

    fn merge_into(self, into: &mut Self, _dh: &DisplayHandle) {
        *into = self;
    }
}

#[derive(Default, Debug, Copy, Clone)]
pub struct CacheablePadding(pub Option<Padding>);

impl Cacheable for CacheablePadding {
    fn commit(&mut self, _dh: &DisplayHandle) -> Self {
        *self
    }

    fn merge_into(self, into: &mut Self, _dh: &DisplayHandle) {
        *into = self;
    }
}

impl CornerRadiusInternal {
    fn set_corner_radius(
        &mut self,
        top_left: u32,
        top_right: u32,
        bottom_right: u32,
        bottom_left: u32,
    ) {
        let corners = Corners { top_left, top_right, bottom_right, bottom_left };
        self.corners = Some(corners);
    }

    fn set_padding(&mut self, top: i32, right: i32, bottom: i32, left: i32) {
        let padding = Padding { top, right, bottom, left };
        self.padding = Some(padding);
    }
}

macro_rules! delegate_corner_radius {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        smithay::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            cosmic_protocols::corner_radius::v1::server::cosmic_corner_radius_manager_v1::CosmicCornerRadiusManagerV1: ()
        ] => CornerRadiusState);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            cosmic_protocols::corner_radius::v1::server::cosmic_corner_radius_manager_v1::CosmicCornerRadiusManagerV1: ()
        ] => CornerRadiusState);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            cosmic_protocols::corner_radius::v1::server::cosmic_corner_radius_toplevel_v1::CosmicCornerRadiusToplevelV1: CornerRadiusData
        ] => CornerRadiusState);
        smithay::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            cosmic_protocols::corner_radius::v1::server::cosmic_corner_radius_layer_v1::CosmicCornerRadiusLayerV1: CornerRadiusData
        ] => CornerRadiusState);
    };
}

impl CornerRadiusHandler for GlobalState {
    fn corner_radius_state(&mut self) -> &mut CornerRadiusState {
        &mut self.server_state.corner_radius_state
    }

    fn commit_xdg(&mut self, corners: CacheableCorners, wl_surface: &WlSurface) {
        for s in &mut self.space.space_list {
            if s.update_popup_corners(corners, wl_surface) {
                break;
            }
        }
    }

    fn commit_wlr(
        &mut self,
        corners: CacheableCorners,
        padding: CacheablePadding,
        wl_surface: &WlSurface,
    ) {
        for (_, _, s_layer_shell_surface, c_layer_shell_surface, _, _, _, _, corner, _) in
            &mut self.client_state.proxied_layer_surfaces
        {
            if s_layer_shell_surface.wl_surface() == wl_surface {
                continue;
            }

            let Some(cosmic_corner_radius_manager) =
                self.client_state.cosmic_corner_radius_manager.as_ref()
            else {
                break;
            };

            if cosmic_corner_radius_manager.version() < 2 {
                break;
            }

            let SurfaceKind::Wlr(wlr) = c_layer_shell_surface.kind() else {
                break;
            };

            let corner_surface = cosmic_corner_radius_manager.get_corner_radius_layer(
                wlr,
                &self.client_state.queue_handle,
                (),
            );

            if let Some(padding) = padding.0 {
                corner_surface.set_padding(
                    padding.top,
                    padding.right,
                    padding.bottom,
                    padding.left,
                );
            } else {
                corner_surface.unset_padding();
            }

            if let Some(corners) = corners.0 {
                corner_surface.set_radius(
                    corners.top_left,
                    corners.top_right,
                    corners.bottom_right,
                    corners.bottom_left,
                );
            } else {
                corner_surface.unset_radius();
            }
            *corner = Some(corner_surface);
        }
    }
}

pub fn _pad_rect(
    mut rect: Rectangle<i32, Logical>,
    padding: &[i32; 4],
) -> Option<Rectangle<i32, Logical>> {
    rect.size.h = rect.size.h.checked_sub(padding[0])?;
    rect.loc.x += padding[3];
    rect.size.w = rect.size.w.checked_sub(padding[1])?;
    rect.size.h = rect.size.h.checked_sub(padding[2])?;
    rect.size.w = rect.size.w.checked_sub(padding[3])?;
    rect.loc.y += padding[0];
    Some(rect)
}

delegate_corner_radius!(GlobalState);
