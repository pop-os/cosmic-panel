use std::cell::RefCell;

use cosmic_plugin::PluginManager;
// SPDX-License-Identifier: GPL-3.0-only
use gtk4::glib;
use gtk4::subclass::prelude::*;
use gtk4::Box;
use gtk4::DropTarget;
use gtk4::EventControllerMotion;
use gtk4::Revealer;
use once_cell::sync::OnceCell;
use std::rc::Rc;

use crate::dock_list::DockList;

#[derive(Default)]
pub struct DockWindowInner {
    pub revealer: OnceCell<Revealer>,
    pub cursor_handle: OnceCell<Box>,
    pub cursor_motion_controller: OnceCell<EventControllerMotion>,
    pub window_drop_controller: OnceCell<DropTarget>,
    pub saved_list: OnceCell<DockList>,
    pub active_list: OnceCell<DockList>,
    pub(super) plugin_manager: Rc<RefCell<PluginManager>>,
}

#[glib::object_subclass]
impl ObjectSubclass for DockWindowInner {
    // `NAME` needs to match `class` attribute of template
    const NAME: &'static str = "DockWindowInner";
    type Type = super::DockWindowInner;
    type ParentType = gtk4::Box;
}

impl ObjectImpl for DockWindowInner {}

impl WidgetImpl for DockWindowInner {}

impl BoxImpl for DockWindowInner {}
