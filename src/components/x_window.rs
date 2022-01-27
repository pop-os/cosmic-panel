use super::window_inner::DockWindowInnerModel;
use crate::components::window_inner::*;
use ccs::*;
use gdk4_x11::X11Display;
use gtk4::{gdk, gio, glib, prelude::*};
use libcosmic::x;

component! {
    #[derive(Default)]
    pub struct XDockWindow {}

    pub struct XDockWindowWidgets {
        inner: Handle<gtk4::Box, DockWindowInnerInput>,
    }

    type Input = ();
    type Output = ();

    type Root = gtk4::ApplicationWindow {
        ccs::view! {
            root = gtk4::ApplicationWindow {
                set_height_request: 80,
                set_width_request: 128,
                set_title: Some("Cosmic Dock"),
                set_decorated: false,
                set_resizable: false,
                add_css_class: "root_window",
            }
        }
        root
    };

    fn init(args: gtk4::Application, root, input, output) {
        let inner = DockWindowInnerModel::init(()).forward(input.clone(), |()| {});

        root.connect_realize(|window| {
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
                    let _height = window.height();
                    let width = window.width();

                    if let Some((display, _surface)) = x::get_window_x11(&window) {
                        let geom = display
                            .primary_monitor().geometry();
                        let monitor_x = geom.x();
                        let _monitor_y = geom.y();
                        let monitor_width = geom.width();
                        let _monitor_height = geom.height();
                        unsafe { x::set_position(&display, &surface,
                                 (monitor_x + monitor_width / 2 - width / 2).clamp(0, monitor_x + monitor_width - 1),
                                  50);
                        }
                                                    // (monitor_y + monitor_height - height).clamp(0, monitor_y + monitor_height - 1));}
                    }
                });

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

        root.set_application(Some(&args));
        root.set_child(Some(inner.widget()));

        root.show();

        ComponentInner {
            model: XDockWindow::default(),
            widgets: XDockWindowWidgets {
                inner,
            },
            input,
            output
        }
    }

    fn update(_component, _event) {}
}
