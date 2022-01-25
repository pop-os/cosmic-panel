// SPDX-License-Identifier: GPL-3.0-only
use cascade::cascade;
use glib::Object;
use glib::Type;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::Align;
use gtk4::Application;
use gtk4::Box;
use gtk4::DropTarget;
use gtk4::EventControllerMotion;
use gtk4::Orientation;
use gtk4::Revealer;
use gtk4::RevealerTransitionType;
use gtk4::Separator;
use gtk4::{gio, glib};

use crate::dock_list::DockList;
use crate::dock_list::DockListType;

mod imp;

glib::wrapper! {
    pub struct DockWindowInner(ObjectSubclass<imp::DockWindowInner>)
        @extends gtk4::Widget, gtk4::Box,
    @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget, gtk4::Orientable;
}

impl Default for DockWindowInner {
    fn default() -> Self {
        Self::new()
    }
}

impl DockWindowInner {
    pub fn new() -> Self {
        let self_: Self = glib::Object::new(&[]).expect("Failed to create DockWindowInner");
        let imp = imp::DockWindowInner::from_instance(&self_);

        cascade! {
            &self_;
            ..set_orientation(Orientation::Vertical);
            // ..add_css_class("dock_container");
        };

        let window_filler = cascade! {
            Box::new(Orientation::Vertical, 0);
            ..set_height_request(0); // shrinks to nothing when revealer is shown
            ..set_vexpand(true); // expands to fill window when revealer is hidden, preventingb window from changing size so much...
        };
        self_.append(&window_filler);

        let revealer = cascade! {
            Revealer::new();
            ..set_reveal_child(true);
            ..set_valign(Align::Baseline);
            ..set_transition_duration(150);
            ..set_transition_type(RevealerTransitionType::SwingUp);
        };
        self_.append(&revealer);

        let dock = cascade! {
            Box::new(Orientation::Horizontal, 4);
            ..set_margin_start(4);
            ..set_margin_end(4);
            ..set_margin_bottom(4);
        };
        dock.add_css_class("dock");
        revealer.set_child(Some(&dock));

        let saved_app_list_view = DockList::new(DockListType::Saved);
        dock.append(&saved_app_list_view);

        let separator = cascade! {
            Separator::new(Orientation::Vertical);
            ..set_margin_start(8);
            ..set_margin_end(8);
        };
        dock.append(&separator);

        let active_app_list_view = DockList::new(DockListType::Active);
        dock.append(&active_app_list_view);

        imp.revealer.set(revealer).unwrap();
        imp.saved_list.set(saved_app_list_view).unwrap();
        imp.active_list.set(active_app_list_view).unwrap();
        // Setup
        self_.setup_motion_controller();
        self_.setup_drop_target();
        self_.setup_callbacks();

        Self::setup_callbacks(&self_);

        self_
    }
    pub fn model(&self, type_: DockListType) -> &gio::ListStore {
        // Get state
        let imp = imp::DockWindowInner::from_instance(self);
        match type_ {
            DockListType::Active => imp.active_list.get().unwrap().model(),
            DockListType::Saved => imp.saved_list.get().unwrap().model(),
        }
    }

    fn setup_callbacks(&self) {
        // Get state
        let imp = imp::DockWindowInner::from_instance(self);
        // let window = self.root().unwrap().dynamic_cast::<gtk4::Window>().unwrap();
        let cursor_event_controller = &imp.cursor_motion_controller.get().unwrap();
        // let drop_controller = &imp.drop_controller.get().unwrap();
        let window_drop_controller = &imp.window_drop_controller.get().unwrap();
        let revealer = &imp.revealer.get().unwrap();
        // self.connect_show(
        //     glib::clone!(@weak revealer, @weak cursor_event_controller => move |_| {
        //         // dbg!(!cursor_event_controller.contains_pointer());
        //         if !cursor_event_controller.contains_pointer() {
        //             revealer.set_reveal_child(false);
        //         }
        //     }),
        // );
        let drop_controller = imp.saved_list.get().unwrap().drop_controller();
        cursor_event_controller.connect_leave(
            glib::clone!(@weak revealer, @weak drop_controller => move |_evc| {
                // only hide if DnD is not happening
                // if drop_controller.current_drop().is_none() {
                    // dbg!("hello, mouse left me :)");
                    // revealer.set_reveal_child(false);
                // }
            }),
        );

        // hack to prevent hiding window when dnd from other apps
        drop_controller.connect_enter(glib::clone!(@weak revealer => @default-return gdk4::DragAction::COPY, move |_self, _x, _y| {

            revealer.set_reveal_child(true);
            gdk4::DragAction::COPY
        }));
        window_drop_controller.connect_drop(|_, _, _, _| {
            println!("dropping into window");
            false
        });
    }

    fn setup_motion_controller(&self) {
        let imp = imp::DockWindowInner::from_instance(self);
        let ev = EventControllerMotion::builder()
            .propagation_limit(gtk4::PropagationLimit::None)
            .propagation_phase(gtk4::PropagationPhase::Capture)
            .build();
        self.add_controller(&ev);

        imp.cursor_motion_controller
            .set(ev)
            .expect("Could not set event controller");
    }
    fn setup_drop_target(&self) {
        // hack for revealing hidden dock when drag enters dock window
        let imp = imp::DockWindowInner::from_instance(self);
        let mut drop_actions = gdk4::DragAction::COPY;
        drop_actions.insert(gdk4::DragAction::MOVE);
        let drop_format = gdk4::ContentFormats::for_type(Type::STRING);
        let drop_format = drop_format.union(&gdk4::ContentFormats::for_type(Type::U32));

        let window_drop_target_controller = DropTarget::builder()
            .actions(drop_actions)
            .formats(&drop_format)
            .build();

        self.add_controller(&window_drop_target_controller);
        imp.window_drop_controller
            .set(window_drop_target_controller)
            .expect("Could not set dock dnd drop controller");
    }
}
