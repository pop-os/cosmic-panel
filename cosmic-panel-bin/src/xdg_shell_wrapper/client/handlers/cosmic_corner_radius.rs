use cctk::cosmic_protocols::corner_radius::v1::client::cosmic_corner_radius_manager_v1::CosmicCornerRadiusManagerV1;
use cctk::cosmic_protocols::corner_radius::v1::client::cosmic_corner_radius_toplevel_v1::CosmicCornerRadiusToplevelV1;
use cctk::sctk;
use cosmic_protocols::corner_radius::v1::client::cosmic_corner_radius_layer_v1::CosmicCornerRadiusLayerV1;
use sctk::reexports::client::{Connection, Dispatch, Proxy};

use crate::xdg_shell_wrapper::shared_state::GlobalState;

impl Dispatch<CosmicCornerRadiusManagerV1, ()> for GlobalState {
    fn event(
        _state: &mut Self,
        _proxy: &CosmicCornerRadiusManagerV1,
        _event: <CosmicCornerRadiusManagerV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &sctk::reexports::client::QueueHandle<Self>,
    ) {
        unimplemented!()
    }
}

impl Dispatch<CosmicCornerRadiusToplevelV1, ()> for GlobalState {
    fn event(
        _state: &mut Self,
        _proxy: &CosmicCornerRadiusToplevelV1,
        _event: <CosmicCornerRadiusToplevelV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &sctk::reexports::client::QueueHandle<Self>,
    ) {
        unimplemented!()
    }
}

impl Dispatch<CosmicCornerRadiusLayerV1, ()> for GlobalState {
    fn event(
        _state: &mut Self,
        _proxy: &CosmicCornerRadiusLayerV1,
        _event: <CosmicCornerRadiusLayerV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &sctk::reexports::client::QueueHandle<Self>,
    ) {
        unimplemented!()
    }
}
