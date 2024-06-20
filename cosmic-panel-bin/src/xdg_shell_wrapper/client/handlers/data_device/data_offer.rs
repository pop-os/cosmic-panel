use sctk::{
    data_device_manager::data_offer::DataOfferHandler,
    reexports::client::protocol::wl_data_device_manager::DndAction,
};

use crate::xdg_shell_wrapper::shared_state::GlobalState;

impl DataOfferHandler for GlobalState {
    // TODO DnD
    fn source_actions(
        &mut self,
        _conn: &sctk::reexports::client::Connection,
        _qh: &sctk::reexports::client::QueueHandle<Self>,
        _offer: &mut sctk::data_device_manager::data_offer::DragOffer,
        _actions: DndAction,
    ) {
        // TODO forward the source actions event
        // for when it was received after the Enter event
    }

    fn selected_action(
        &mut self,
        _conn: &sctk::reexports::client::Connection,
        _qh: &sctk::reexports::client::QueueHandle<Self>,
        _offer: &mut sctk::data_device_manager::data_offer::DragOffer,
        _actions: DndAction,
    ) {
        // TODO forward the selected action event
        // could be useful when we are selecting the action?
    }
}
