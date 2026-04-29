use cctk::{sctk, wayland_client};
use sctk::globals::GlobalData;
use sctk::reexports::client::globals::{BindError, GlobalList};
use sctk::reexports::client::protocol::wl_surface::WlSurface;
use sctk::reexports::client::{Connection, Dispatch, Proxy, QueueHandle, delegate_dispatch};
use wayland_protocols::ext::background_effect::v1::client::ext_background_effect_manager_v1::{
    Capability, Event, ExtBackgroundEffectManagerV1,
};
use wayland_protocols::ext::background_effect::v1::client::ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1;

use crate::xdg_shell_wrapper::shared_state::GlobalState;

#[derive(Debug, Clone)]
pub struct ExtBackgroundEffectManager {
    pub manager: ExtBackgroundEffectManagerV1,
    capabilities: Capability,
}

impl ExtBackgroundEffectManager {
    pub fn new(
        globals: &GlobalList,
        queue_handle: &QueueHandle<GlobalState>,
    ) -> Result<Self, BindError> {
        let manager = globals.bind(queue_handle, 1..=1, GlobalData)?;
        Ok(Self { manager, capabilities: Capability::empty() })
    }

    pub fn blur(
        &mut self,
        surface: &WlSurface,
        queue_handle: &QueueHandle<GlobalState>,
    ) -> ExtBackgroundEffectSurfaceV1 {
        self.manager.get_background_effect(surface, queue_handle, ())
    }

    pub fn capabilities(&self) -> Capability {
        self.capabilities
    }
}

impl Dispatch<ExtBackgroundEffectManagerV1, GlobalData, GlobalState>
    for ExtBackgroundEffectManager
{
    fn event(
        state: &mut GlobalState,
        _: &ExtBackgroundEffectManagerV1,
        event: <ExtBackgroundEffectManagerV1 as Proxy>::Event,
        _: &GlobalData,
        _: &Connection,
        _: &QueueHandle<GlobalState>,
    ) {
        match event {
            Event::Capabilities { flags } => match flags {
                wayland_client::WEnum::Value(capability) => {
                    if let Some(bg_effect_mgr) =
                        state.client_state.ext_background_effect_manager.as_mut()
                    {
                        bg_effect_mgr.capabilities = capability;
                    }
                    if capability.contains(Capability::Blur) {
                        state.enable_blur_capacity();
                    }
                },
                wayland_client::WEnum::Unknown(u) => {
                    tracing::warn!("Unknown value: {u:?}");
                },
            },
            e => {
                tracing::warn!("Ignored event {e:?}");
            },
        }
    }
}

impl Dispatch<ExtBackgroundEffectSurfaceV1, (), GlobalState> for ExtBackgroundEffectManager {
    fn event(
        _: &mut GlobalState,
        _: &ExtBackgroundEffectSurfaceV1,
        _: <ExtBackgroundEffectSurfaceV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<GlobalState>,
    ) {
        // There is no event
    }
}

delegate_dispatch!(GlobalState: [ExtBackgroundEffectManagerV1: GlobalData] => ExtBackgroundEffectManager);
delegate_dispatch!(GlobalState: [ExtBackgroundEffectSurfaceV1: ()] => ExtBackgroundEffectManager);
