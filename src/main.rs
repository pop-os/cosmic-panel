// SPDX-License-Identifier: GPL-3.0-only
use ccs::*;
use gdk4::Display;
use gtk4::prelude::*;
use gtk4::{CssProvider, StyleContext};

extern crate cosmic_component_system as ccs;

mod components;
mod utils;
use components::window;

const ID: &str = "com.cosmic.dock2";

fn load_css() {
    // Load the css file and add it to the provider
    let provider = CssProvider::new();
    provider.load_from_data(include_bytes!("style.css"));

    // Add the provider to the default screen
    StyleContext::add_provider_for_display(
        &Display::default().expect("Error initializing GTK CSS provider."),
        &provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

fn main() {
    let app = gtk4::builders::ApplicationBuilder::new()
        .application_id(ID)
        .build();
    app.connect_activate(|app| {
        load_css();
        let display = Display::default().unwrap();
        window::create(
            &app,
            display.monitors().item(0).unwrap().downcast().unwrap(),
        );
    });
    app.run();
}
