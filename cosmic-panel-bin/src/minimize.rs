use crate::xdg_shell_wrapper::shared_state::GlobalState;
use cctk::{
    cosmic_protocols::toplevel_info::v1::client::zcosmic_toplevel_handle_v1,
    wayland_client::{protocol::wl_surface::WlSurface, Proxy},
};
use smithay::utils::{Logical, Rectangle};

#[derive(Debug, Clone)]
pub struct MinimizeApplet {
    pub priority: i32,
    pub rect: Rectangle<i32, Logical>,
    pub surface: WlSurface,
}

pub fn update_toplevel(
    state: &mut GlobalState,
    toplevel: zcosmic_toplevel_handle_v1::ZcosmicToplevelHandleV1,
) {
    let Some(toplevel_mngr) = state.client_state.toplevel_manager_state.as_ref() else {
        return;
    };
    let minimized_applets = &state.space.minimized_applets;
    let Some((_, toplevel_info)) = state.space.toplevels.iter().find(|t| &t.0 == &toplevel) else {
        return;
    };

    if let Some((_, info)) = minimized_applets.iter().find(|(output_name, _)| {
        toplevel_info.output.iter().any(|o| {
            let Some(i) = state.client_state.output_state.info(o) else {
                return false;
            };
            i.name.as_ref() == Some(output_name)
        })
    }) {
        toplevel_mngr.manager.set_rectangle(
            &toplevel,
            &info.surface,
            info.rect.loc.x,
            info.rect.loc.y,
            info.rect.size.w,
            info.rect.size.h,
        );
    }
}

pub fn set_rectangles(state: &mut GlobalState, output: String, info: MinimizeApplet) {
    // if surface matches cur & is different, or is higher priority, replace
    let mut changed = false;
    let minimized_applets = &mut state.space.minimized_applets;

    let old_info = minimized_applets.entry(output.clone()).or_insert_with(|| {
        changed = true;
        info.clone()
    });

    if !changed {
        if old_info.surface == info.surface && old_info.rect != info.rect {
            old_info.rect = info.rect;
            changed = true;
        } else if old_info.priority < info.priority || !old_info.surface.is_alive() {
            *old_info = info.clone();
            changed = true;
        }
    }

    // if changed, send rect for all toplevels for the given out
    if changed {
        let Some(toplevel_mngr) = state.client_state.toplevel_manager_state.as_ref() else {
            return;
        };

        for (toplevel, toplevel_info) in &mut state.space.toplevels {
            if !toplevel_info.output.iter().any(|o| {
                let Some(i) = state.client_state.output_state.info(o) else {
                    return false;
                };
                i.name.as_ref() == Some(&output)
            }) {
                continue;
            }
            toplevel_mngr.manager.set_rectangle(
                toplevel,
                &info.surface,
                info.rect.loc.x,
                info.rect.loc.y,
                info.rect.size.w,
                info.rect.size.h,
            );
        }
    }
}
