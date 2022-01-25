// SPDX-License-Identifier: GPL-3.0-only
use gtk4::glib;
use gtk4::subclass::prelude::*;
use gtk4::Box;
use gtk4::DropTarget;
use gtk4::EventControllerMotion;
use once_cell::sync::OnceCell;

use crate::window_inner::DockWindowInner;

// Object holding the state
#[derive(Default)]
pub struct Window {
    pub inner: OnceCell<DockWindowInner>,
}

// The central trait for subclassing a GObject
#[glib::object_subclass]
impl ObjectSubclass for Window {
    // `NAME` needs to match `class` attribute of template
    const NAME: &'static str = "DockWindow";
    type Type = super::Window;
    type ParentType = gtk4::ApplicationWindow;
}

// Trait shared by all GObjects
impl ObjectImpl for Window {}

// Trait shared by all widgets
impl WidgetImpl for Window {}

// Trait shared by all windows
impl WindowImpl for Window {}

// Trait shared by all application
impl ApplicationWindowImpl for Window {}
