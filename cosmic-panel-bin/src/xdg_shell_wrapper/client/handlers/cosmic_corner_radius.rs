use std::sync::Arc;

use cctk::{
    cosmic_protocols::corner_radius::v1::client::{
        cosmic_corner_radius_manager_v1::CosmicCornerRadiusManagerV1,
        cosmic_corner_radius_toplevel_v1::CosmicCornerRadiusToplevelV1,
    },
    sctk,
};
use cosmic_protocols::corner_radius::v1::client::cosmic_corner_radius_layer_v1::CosmicCornerRadiusLayerV1;
use sctk::reexports::client::{Connection, Dispatch, Proxy};

use crate::xdg_shell_wrapper::shared_state::GlobalState;

#[derive(Debug, Clone)]
pub struct CornerRadius(Arc<CornerRadiusWrapper>);

#[derive(Debug)]
pub enum CornerRadiusWrapper {
    Xdg(CosmicCornerRadiusToplevelV1),
    Wlr(CosmicCornerRadiusLayerV1),
}

impl Drop for CornerRadiusWrapper {
    fn drop(&mut self) {
        match self {
            Self::Xdg(c) => c.destroy(),
            Self::Wlr(c) => c.destroy(),
        };
    }
}

impl Dispatch<CosmicCornerRadiusManagerV1, ()> for GlobalState {
    fn event(
        _state: &mut Self,
        _proxy: &CosmicCornerRadiusManagerV1,
        event: <CosmicCornerRadiusManagerV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &sctk::reexports::client::QueueHandle<Self>,
    ) {
        match event {
            _ => unimplemented!(),
        }
    }
}

impl Dispatch<CosmicCornerRadiusToplevelV1, ()> for GlobalState {
    fn event(
        _state: &mut Self,
        _proxy: &CosmicCornerRadiusToplevelV1,
        event: <CosmicCornerRadiusToplevelV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &sctk::reexports::client::QueueHandle<Self>,
    ) {
        match event {
            _ => unimplemented!(),
        }
    }
}

impl Dispatch<CosmicCornerRadiusLayerV1, ()> for GlobalState {
    fn event(
        _state: &mut Self,
        _proxy: &CosmicCornerRadiusLayerV1,
        event: <CosmicCornerRadiusLayerV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &sctk::reexports::client::QueueHandle<Self>,
    ) {
        match event {
            _ => unimplemented!(),
        }
    }
}
