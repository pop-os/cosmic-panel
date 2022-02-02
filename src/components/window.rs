use crate::components::x_window;
use ccs::*;
use gtk4::Application;
use gtk4::{gdk, prelude::*};

pub fn create(app: &Application, monitor: gdk::Monitor) {
    #[cfg(feature = "layer-shell")]
    if let Some(wayland_monitor) = monitor.downcast_ref() {
        wayland_create(app, wayland_monitor);
        return;
    }

    x_create(app, monitor);
}

#[cfg(feature = "layer-shell")]
fn wayland_create(app: &Application, monitor: &gdk4_wayland::WaylandMonitor) {
    use crate::components::wayland_window;
    wayland_window::WaylandDockWindow::init().launch_stateful((app.clone(), monitor.clone()));
}

fn x_create(app: &Application, monitor: gdk::Monitor) {
    x_window::XDockWindow::init().launch_stateful((app.clone(), monitor));
}
