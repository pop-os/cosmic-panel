use cctk::{
    cosmic_protocols::overlap_notify::v1::client::{
        zcosmic_overlap_notification_v1::{self, ZcosmicOverlapNotificationV1},
        zcosmic_overlap_notify_v1::ZcosmicOverlapNotifyV1,
    },
    wayland_client::{
        self, event_created_child,
        globals::{BindError, GlobalList},
        protocol::wl_surface::WlSurface,
        Connection, Dispatch, Proxy, QueueHandle,
    },
    wayland_protocols::ext::foreign_toplevel_list::v1::client::ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
};
use sctk::{globals::GlobalData, shell::WaylandSurface};

use crate::xdg_shell_wrapper::shared_state::GlobalState;

#[derive(Debug, Clone)]
pub struct OverlapNotifyV1 {
    pub(crate) notify: ZcosmicOverlapNotifyV1,
}

impl OverlapNotifyV1 {
    pub fn bind(
        globals: &GlobalList,
        qh: &QueueHandle<GlobalState>,
    ) -> Result<OverlapNotifyV1, BindError> {
        let notify = globals.bind(qh, 1..=1, GlobalData)?;
        Ok(OverlapNotifyV1 { notify })
    }
}

impl Dispatch<ZcosmicOverlapNotifyV1, GlobalData, GlobalState> for OverlapNotifyV1 {
    fn event(
        _: &mut GlobalState,
        _: &ZcosmicOverlapNotifyV1,
        _: <ZcosmicOverlapNotifyV1 as Proxy>::Event,
        _: &GlobalData,
        _: &Connection,
        _: &QueueHandle<GlobalState>,
    ) {
    }
}

#[derive(Debug)]
pub struct OverlapNotificationV1 {
    pub surface: WlSurface,
}

impl Dispatch<ZcosmicOverlapNotificationV1, OverlapNotificationV1, GlobalState>
    for OverlapNotificationV1
{
    fn event(
        state: &mut GlobalState,
        _n: &ZcosmicOverlapNotificationV1,
        event: <ZcosmicOverlapNotificationV1 as Proxy>::Event,
        data: &OverlapNotificationV1,
        _: &Connection,
        _: &QueueHandle<GlobalState>,
    ) {
        let my_surface = &data.surface;
        for s in &mut state.space.space_list {
            if !s.layer.as_ref().is_some_and(|l| l.wl_surface() == my_surface) {
                continue;
            }
            match event {
                zcosmic_overlap_notification_v1::Event::ToplevelEnter { ref toplevel, .. } => {
                    s.toplevel_overlaps.insert(toplevel.id());
                },
                zcosmic_overlap_notification_v1::Event::ToplevelLeave { ref toplevel } => {
                    s.toplevel_overlaps.remove(&toplevel.id());
                },
                _ => {},
            }
        }
    }

    event_created_child!(GlobalState, ZcosmicOverlapNotifyV1, [
        0 => (ExtForeignToplevelHandleV1, Default::default())
    ]);
}

wayland_client::delegate_dispatch!(GlobalState: [ZcosmicOverlapNotifyV1: GlobalData] => OverlapNotifyV1);
wayland_client::delegate_dispatch!(GlobalState: [ZcosmicOverlapNotificationV1: OverlapNotificationV1] => OverlapNotificationV1);
