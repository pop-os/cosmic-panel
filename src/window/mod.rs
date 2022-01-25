// SPDX-License-Identifier: GPL-3.0-only
use crate::dock_list::DockListType;
use crate::window_inner::DockWindowInner;
use cascade::cascade;
use gdk4_x11::X11Display;
use glib::Object;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::Application;
use gtk4::{gio, glib};
use libcosmic::x;
mod imp;

glib::wrapper! {
    pub struct Window(ObjectSubclass<imp::Window>)
        @extends gtk4::ApplicationWindow, gtk4::Window, gtk4::Widget,
        @implements gio::ActionGroup, gio::ActionMap, gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget, gtk4::Native, gtk4::Root, gtk4::ShortcutManager;
}

impl Window {
    pub fn new(app: &Application) -> Self {
        let self_: Self = Object::new(&[("application", app)]).expect("Failed to create `Window`.");
        let imp = imp::Window::from_instance(&self_);
        cascade! {
            &self_;
            ..set_height_request(100);
            ..set_width_request(128);
            ..set_title(Some("Cosmic Dock"));
            ..set_decorated(false);
            ..set_resizable(false);
            ..add_css_class("root_window");
        };

        let inner = DockWindowInner::new();
        self_.set_child(Some(&inner));
        imp.inner.set(inner).unwrap();

        self_.setup_callbacks();
        self_
    }

    fn setup_callbacks(&self) {
        let window = self.clone().upcast::<gtk4::Window>();
        window.connect_realize(move |window| {
            if let Some((display, surface)) = x::get_window_x11(window) {
                // ignore all x11 errors...
                let xdisplay = display.clone().downcast::<X11Display>().expect("Failed to downgrade X11 Display.");
                xdisplay.error_trap_push();
                unsafe {
                    x::change_property(
                        &display,
                        &surface,
                        "_NET_WM_WINDOW_TYPE",
                        x::PropMode::Replace,
                        &[x::Atom::new(&display, "_NET_WM_WINDOW_TYPE_DOCK").unwrap()],
                    );
                }
                let resize = glib::clone!(@weak window => move || {
                    let height = window.height();
                    let width = window.width();

                    if let Some((display, _surface)) = x::get_window_x11(&window) {
                        let geom = display
                            .primary_monitor().geometry();
                        let monitor_x = geom.x();
                        let monitor_y = geom.y();
                        let monitor_width = geom.width();
                        let monitor_height = geom.height();
                        // dbg!(monitor_x);
                        // dbg!(monitor_y);
                        // dbg!(monitor_width);
                        // dbg!(monitor_height);
                        // dbg!(width);
                        // dbg!(height);
                        unsafe { x::set_position(&display, &surface,
                            (monitor_x + monitor_width / 2 - width / 2).clamp(0, monitor_x + monitor_width - 1),
                                                    (monitor_y + monitor_height - height).clamp(0, monitor_y + monitor_height - 1));}
                    }
                });

                // let resize_drop = resize.clone();
                // window_drop_controller.connect_enter(glib::clone!(@weak revealer, @weak window => @default-return gdk4::DragAction::COPY, move |_self, _x, _y| {
                //     glib::source::idle_add_local_once(resize_drop.clone());
                //     revealer.set_reveal_child(true);
                //     gdk4::DragAction::COPY
                // }));

                // let resize_cursor = resize.clone();
                // cursor_event_controller.connect_enter(glib::clone!(@weak revealer, @weak window => move |_evc, _x, _y| {
                //     // dbg!("hello, mouse entered me :)");
                //     revealer.set_reveal_child(true);
                //     glib::source::idle_add_local_once(resize_cursor.clone());
                // }));

                // let resize_revealed = resize.clone();
                // revealer.connect_child_revealed_notify(glib::clone!(@weak window => move |r| {
                //     if !r.is_child_revealed() {
                //         glib::source::idle_add_local_once(resize_revealed.clone());
                //     }
                //     }));

                let s = window.surface();
                let resize_height = resize.clone();
                s.connect_height_notify(move |_s| {
                    glib::source::idle_add_local_once(resize_height.clone());
                });
                let resize_width = resize.clone();
                s.connect_width_notify(move |_s| {
                    glib::source::idle_add_local_once(resize_width.clone());
                });
                s.connect_scale_factor_notify(move |_s| {
                    glib::source::idle_add_local_once(resize.clone());
                });
            } else {
                println!("failed to get X11 window");
            }
        });
    }

    pub fn model(&self, type_: DockListType) -> &gio::ListStore {
        // Get state
        let imp = imp::Window::from_instance(self);
        imp.inner.get().unwrap().model(type_)
    }
}
