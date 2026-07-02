// proxy requests from clients for popups or layer surfaces

use std::any::TypeId;
use std::sync::Mutex;

use sctk::compositor::Region;
use sctk::shell::WaylandSurface;
use smithay::delegate_background_effect;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{HookId, Logical, Rectangle};
use smithay::wayland::background_effect::{Capability, ExtBackgroundEffectHandler};
use smithay::wayland::compositor::{
    Cacheable, RectangleKind, RegionAttributes, add_pre_commit_hook, with_states,
};

use crate::xdg_shell_wrapper::shared_state::GlobalState;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComputedBlurRegionCachedState {
    /// Region of the surface that will have its background blurred.
    pub blur_region: Option<Vec<Rectangle<i32, Logical>>>,
}

impl Cacheable for ComputedBlurRegionCachedState {
    fn commit(&mut self, _dh: &DisplayHandle) -> Self {
        self.clone()
    }

    fn merge_into(self, into: &mut Self, _dh: &DisplayHandle) {
        *into = self;
    }
}

trait BlurHandler {
    fn commit_blur(&mut self, region: Option<Vec<Rectangle<i32, Logical>>>, surface: &WlSurface);
}

impl BlurHandler for GlobalState {
    fn commit_blur(&mut self, region: Option<Vec<Rectangle<i32, Logical>>>, surface: &WlSurface) {
        for s in &mut self.space.space_list {
            if s.commit_popup_blur(region.as_ref(), surface) {
                return;
            }
        }

        for (_, _, s_layer_shell_surface, c_layer_shell_surface, _, _, _, _, _, blur_surface) in
            &mut self.client_state.proxied_layer_surfaces
        {
            if s_layer_shell_surface.wl_surface() == surface {
                continue;
            }

            let Some(ext_background_effect_manager) =
                self.client_state.ext_background_effect_manager.as_mut()
            else {
                break;
            };

            let new_blur_surface = ext_background_effect_manager
                .blur(c_layer_shell_surface.wl_surface(), &self.client_state.queue_handle);

            if let Some(ref region) = region {
                let Ok(wl_region) = Region::new(&self.client_state.compositor_state) else {
                    break;
                };
                for r in region {
                    wl_region.add(r.loc.x, r.loc.y, r.size.w, r.size.h);
                }
                new_blur_surface.set_blur_region(Some(wl_region.wl_region()));
            } else {
                new_blur_surface.set_blur_region(None);
            }

            *blur_surface = Some(new_blur_surface);
        }
    }
}

impl ExtBackgroundEffectHandler for GlobalState {
    fn capabilities(&self) -> Capability {
        Capability::Blur
    }

    fn set_blur_region(&mut self, surface: WlSurface, region: RegionAttributes) {
        with_states(&surface, |states| {
            let mut blur_state = states.cached_state.get::<ComputedBlurRegionCachedState>();

            blur_state.pending().blur_region = Some({
                let (added, subtracted) = region
                    .rects
                    .iter()
                    .cloned()
                    .partition::<Vec<_>, _>(|(op, _)| matches!(op, RectangleKind::Add));
                let added = added.into_iter().map(|(_, rect)| rect).collect::<Vec<_>>();
                Rectangle::subtract_rects_many_in_place(
                    added,
                    subtracted.into_iter().map(|(_, rect)| rect),
                )
            })
        });
        hook_commit::<GlobalState>(&surface);
    }

    fn unset_blur_region(&mut self, surface: WlSurface) {
        with_states(&surface, |states| {
            let mut blur_state = states.cached_state.get::<ComputedBlurRegionCachedState>();

            blur_state.pending().blur_region.take();
        })
    }
}

type BlurHookId = Mutex<Option<(HookId, TypeId)>>;

fn hook_commit<D: BlurHandler + 'static>(wl_surface: &WlSurface)
where
    D: 'static,
{
    struct Blur;
    let blur_exists = with_states(wl_surface, |surface_data| {
        let hook_id = surface_data.data_map.get_or_insert_threadsafe(|| BlurHookId::new(None));
        let guard = hook_id.lock().unwrap();
        guard.is_some()
    });
    if blur_exists {
        return;
    }
    let blur_id = std::any::TypeId::of::<Blur>();

    let hook_id = add_pre_commit_hook::<D, _>(wl_surface, blur_hook);
    with_states(wl_surface, |surface_data| {
        let hook_ids = surface_data.data_map.get_or_insert_threadsafe(|| BlurHookId::new(None));
        let mut guard = hook_ids.lock().unwrap();
        *guard = Some((hook_id, blur_id));
    });
}

fn blur_hook<D: 'static + BlurHandler>(state: &mut D, _dh: &DisplayHandle, surface: &WlSurface) {
    let region = with_states(surface, |states| {
        let mut blur_state = states.cached_state.get::<ComputedBlurRegionCachedState>();
        let pending = blur_state.pending().clone();
        if *blur_state.current() != pending {
            return None;
        } else {
            return Some(pending);
        }
    });
    if let Some(region) = region {
        state.commit_blur(region.blur_region, surface);
    }
}

delegate_background_effect!(GlobalState);
