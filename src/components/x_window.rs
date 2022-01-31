use super::window_inner::{DockWindowInnerModel, DockWindowInnerOutput};
use crate::components::window_inner::*;
use ccs::*;
use cosmic_plugin::Position;
use gdk4_x11::X11Display;
use gtk4::{gdk, gio, glib, prelude::*};
use libcosmic::x;
use std::cell::Cell;
use std::rc::Rc;

#[derive(Clone, Copy, Debug)]
pub enum XWindowInput {
    Position(Position),
}

component! {
    #[derive(Default)]
    pub struct XDockWindow {
        position: Rc<Cell<Position>>,
    }

    pub struct XDockWindowWidgets {
        inner: Controller<gtk4::Box, DockWindowInnerInput>,
        window: gtk4::ApplicationWindow,
    }

    type Input = XWindowInput;
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

    fn init(args: (gtk4::Application, gdk::Monitor), root, input, output) {
        let model = XDockWindow::default();

        let inner = DockWindowInnerModel::init().launch_stateful(()).forward(input.clone(), |e| {
            match e {
                DockWindowInnerOutput::Position(p) => XWindowInput::Position(p),
            }
        });

        let monitor = args.1;
        root.connect_realize(glib::clone!(@weak model.position as position => move |window| {
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
                let resize = glib::clone!(@weak window, @weak position, @strong monitor => move || {
                    dbg!(position.get());
                    let height = window.height();
                    let width = window.width();

                    if let Some((display, _surface)) = x::get_window_x11(&window) {
                        let geom = monitor.geometry();
                        let monitor_x = geom.x();
                        let monitor_y = geom.y();
                        let monitor_width = geom.width();
                        let monitor_height = geom.height();
                        let (x, y) = match position.get() {
                            Position::Top => (monitor_x + monitor_width / 2 - width / 2, monitor_y + 50),
                            Position::Bottom => (monitor_x + monitor_width / 2 - width / 2, monitor_y + monitor_height - height),
                            Position::Start => (monitor_x, monitor_y + monitor_height / 2 - height / 2),
                            Position::End => (monitor_x + monitor_width - width, monitor_y + monitor_height / 2 - height / 2),
                        };
                        dbg!((x,y));
                        unsafe { x::set_position(&display, &surface, x, y);}
                    }
                });

                resize.clone()();
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

        }));

        root.set_application(Some(&args.0));
        root.set_child(Some(&inner.widget));

        root.show();

        Fuselage {
            model,
            widgets: XDockWindowWidgets {
                window: root.clone(),
                inner,
            },
        }
    }

    fn update(&mut self, widgets, event, _input, _output) {
        let model = self;
        dbg!(event.clone());
        match event {
            XWindowInput::Position(p) => {
                model.position.replace(p);
                match p {
                    Position::Start | Position::End => {
                        widgets.window.set_height_request(128);
                        widgets.window.set_width_request(80);
                    }
                    Position::Top | Position::Bottom => {
                        widgets.window.set_height_request(80);
                        widgets.window.set_width_request(128);
                    }
                };
            }
        }
        Some(())
    }
    async fn command(_message: (), _input) {

    }
}
