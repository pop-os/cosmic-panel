use crate::components::x_window;
use ccs::*;
use gtk4::Application;
use gtk4::{gdk, gio, glib};

pub fn create(app: &Application, monitor: gdk::Monitor) {
    #[cfg(feature = "layer-shell")]
    if let Some(wayland_monitor) = monitor.downcast_ref() {
        wayland_create(app, wayland_monitor);
        return;
    }

    x_create(app);
}

#[cfg(feature = "layer-shell")]
fn wayland_create(app: &Application, monitor: &gdk4_wayland::WaylandMonitor) {
    use crate::components::wayland_window;
    use libcosmic::wayland::{Anchor, KeyboardInteractivity, Layer, LayerShellWindow};

    let window = cascade! {
        LayerShellWindow::new(Some(monitor), Layer::Top, "");
        ..set_width_request(800);
        ..set_height_request(600);
        // ..set_title(Some("Cosmic App Library"));
        // ..set_decorated(false);
        ..set_keyboard_interactivity(KeyboardInteractivity::OnDemand);
        ..add_css_class("root_window");
        ..set_anchor(Anchor::empty());
        ..show();
    };

    let app_library = AppLibraryWindowInner::new();
    window.set_child(Some(&app_library));
    dbg!(&window);
    window.connect_is_active_notify(glib::clone!(@weak app => move |w| {
        if !w.is_active() {
            app.quit();
        }
    }));
    window.show();

    // setup_shortcuts(window.clone().upcast::<gtk4::ApplicationWindow>());
    // XXX
    unsafe { window.set_data("cosmic-app-hold", app.hold()) };
}

fn x_create(app: &Application) {
    x_window::XDockWindow::default().register(app.clone());
}
