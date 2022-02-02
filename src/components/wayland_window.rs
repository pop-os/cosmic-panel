use super::window_inner::{DockWindowInnerModel, DockWindowInnerOutput};
use crate::components::window_inner::*;
use cascade::cascade;
use ccs::*;
use cosmic_plugin::Position;
use gtk4::prelude::ApplicationExt;
use gtk4::{gdk, gio, glib, prelude::*};
use std::cell::Cell;
use std::rc::Rc;
#[derive(Clone, Copy, Debug)]
pub enum WaylandWindowInput {
    Position(Position),
}

use crate::components::wayland_window;
use libcosmic::wayland::{Anchor, KeyboardInteractivity, Layer, LayerShellWindow};
component! {

    #[derive(Default)]
    pub struct WaylandDockWindow {
        position: Rc<Cell<Position>>,
    }

    pub struct WaylandDockWindowWidgets {
        inner: Controller<gtk4::Box, DockWindowInnerInput>,
        window: LayerShellWindow,
    }

    type Input = WaylandWindowInput;
    type Output = ();

    type Root = LayerShellWindow {
        let root = cascade! {
            LayerShellWindow::new();
            ..add_css_class("root_window");
            ..set_layer(Layer::Top);
            ..set_namespace("");
            ..set_anchor(Anchor::Bottom);
            ..set_height_request(80);
            ..set_width_request(80);
        };
        root
    };

    fn init(args: (gtk4::Application, gdk4_wayland::WaylandMonitor), root, input, output) {
        // XXX
        // Some(monitor), Layer::Top, ""
        root.set_monitor(&args.1);
        let model = WaylandDockWindow::default();

        let inner = DockWindowInnerModel::init().launch_stateful(()).forward(input.clone(), |e| {
            match e {
                DockWindowInnerOutput::Position(p) => WaylandWindowInput::Position(p),
            }
        });

        let monitor = args.1;

        unsafe { root.set_data("cosmic-app-hold", args.0.hold()) };
        root.set_child(Some(&inner.widget));

        root.show();

        Fuselage {
            model,
            widgets: WaylandDockWindowWidgets {
                window: root.clone(),
                inner,
            },
        }
    }

    fn update(&mut self, widgets, event, _input, _output) {
        let model = self;
        dbg!(event.clone());
        match event {
            WaylandWindowInput::Position(p) => {
                model.position.replace(p);
                match p {
                    Position::Start => {
                        widgets.window.set_anchor(Anchor::Left);
                        widgets.window.set_height_request(128);
                        widgets.window.set_width_request(80);
                    }
                    Position::End => {
                        widgets.window.set_anchor(Anchor::Right);
                        widgets.window.set_height_request(128);
                        widgets.window.set_width_request(80);
                    }
                    Position::Top => {
                        widgets.window.set_anchor(Anchor::Top);
                        widgets.window.set_height_request(80);
                        widgets.window.set_width_request(128);
                    }
                    Position::Bottom => {
                        widgets.window.set_anchor(Anchor::Bottom);
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
