use ccs::*;
use cosmic_plugin::{PluginManager, Position};
use futures::{channel::mpsc::Receiver, SinkExt, StreamExt};
use gtk4::prelude::*;
use notify::{
    event::{AccessKind, AccessMode, DataChange, EventKind, ModifyKind},
    Event, INotifyWatcher, RecursiveMode, Watcher,
};
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use std::{fs::File, path::PathBuf};
extern crate notify;

pub enum DockWindowInnerInput {
    PluginList(Vec<String>),
    PluginUpdate(notify::Event),
    Position(Position),
    Scale(i32),
}

pub enum DockWindowInnerOutput {
    Position(Position),
}

component! {
    #[derive(Default)]
    pub struct DockWindowInnerModel {
        pub plugin_manager: PluginManager<'static>,
    }

    pub struct DockWindowInnerWidgets {
        applet_box: gtk4::Box,
    }

    type Input = DockWindowInnerInput;
    type Output = DockWindowInnerOutput;

    type Root = gtk::Box {
        ccs::view! {
            root = gtk4::Box {
                set_orientation:gtk4::Orientation::Horizontal,
                set_spacing: 0,
                set_vexpand: true,
                set_hexpand: true,
                // set_halign: gtk4::Align::Center,
                // set_valign: gtk4::Align::Center,
                add_css_class: "dock",
            }
        }
        root
    };

    fn init(_args: (), root, input, output) {
        let (plugin_manager, rx) = PluginManager::new();

        if let Some(rx) = rx {
            ccs::spawn_local(async_watch_loaded_plugins(input.clone(), rx));
        }
        ccs::spawn_local(async_watch_plugin_settings(input.clone()));
        ccs::view! {
           applet_box = &gtk4::Box {
                    set_orientation: gtk4::Orientation::Horizontal,
                    set_spacing: 0,
                    set_vexpand: true,
                    set_hexpand: true,
                    set_valign: gtk4::Align::Center,
                    set_halign: gtk4::Align::Center,
                }
        }
        root.append(&applet_box);
        Fuselage
        { model: DockWindowInnerModel{plugin_manager}, widgets: DockWindowInnerWidgets {applet_box}}
    }

    fn update(&mut self, widgets, event, _input, output) {
        let model = self;
        match event {
            DockWindowInnerInput::PluginList(plugin_filenames) => {
                while let Some(c) = widgets.applet_box.first_child() {
                    widgets.applet_box.remove(&c);
                }
                // XXX is it guaranteed that all references for the applets are dropped by now?
                model.plugin_manager.unload_all();
                // update applets
                for f in plugin_filenames {
                    match unsafe {model.plugin_manager.load_plugin(&f)} {
                        Ok((applet, css_provider)) => {
                            gtk4::StyleContext::add_provider_for_display(
                                &gdk4::Display::default().expect("Error initializing GTK CSS provider."),
                                css_provider,
                                gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
                            );
                            println!("adding applet for {}", f);

                            widgets.applet_box.append(applet);
                        }
                        Err(e) => eprintln!("{}", e)
                    }
                }
            }
            DockWindowInnerInput::PluginUpdate(e) => {
                match e.kind {
                    EventKind::Remove(_) => {
                        for f in e.paths {
                            if let Some(applet_to_remove) = model.plugin_manager.library_path_to_applet(&f) {
                                widgets.applet_box.remove(applet_to_remove);
                            }
                            unsafe { model.plugin_manager.unload_plugin(f) };
                        }
                    }
                    EventKind::Access(AccessKind::Close(AccessMode::Write)) => {
                        for f in e.paths {
                            // get name of plugin to be loaded from plugin manager
                            let name = match model.plugin_manager.library_path_to_name(f) {
                                Some(s) => s,
                                None => continue,
                            };
                            match unsafe {model.plugin_manager.load_plugin(name)} {
                                Ok((applet, css_provider)) => {
                                    gtk4::StyleContext::add_provider_for_display(
                                        &gdk4::Display::default().expect("Error initializing GTK CSS provider."),
                                        css_provider,
                                        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
                                    );
                                    widgets.applet_box.append(applet);
                                }
                                Err(e) => eprintln!("{}", e)
                            }
                        }

                    }
                    e => {
                        // dbg!(e);
                    }
                };
            }
            DockWindowInnerInput::Position(p) => {
                dbg!(p);
                model.plugin_manager.set_position(p);
                if let Err(_e) = output.send(DockWindowInnerOutput::Position(p)) {
                    eprintln!("failed to send position to window");
                }
            }
            DockWindowInnerInput::Scale(s) => {
                dbg!(s);
            }
        }
        Some(())
    }

    async fn command(command: (), input) {

    }
}

/// forwards events from plugin manager to application
async fn async_watch_loaded_plugins(
    sender: Sender<DockWindowInnerInput>,
    mut receiver: Receiver<notify::Result<Event>>,
) {
    while let Some(res) = receiver.next().await {
        match res {
            Ok(event) => {
                let _ = sender.send(DockWindowInnerInput::PluginUpdate(event));
            }
            Err(e) => {
                eprintln!("{}", e);
            }
        }
    }
}

fn async_watcher() -> notify::Result<(INotifyWatcher, Receiver<notify::Result<Event>>)> {
    use futures::channel::mpsc::channel;
    let (mut tx, rx) = channel(1);

    let watcher = INotifyWatcher::new(move |res| {
        futures::executor::block_on(async {
            if let Err(e) = tx.send(res).await {
                dbg!(e);
            }
        })
    })?;

    Ok((watcher, rx))
}

async fn async_watch_plugin_settings(sender: Sender<DockWindowInnerInput>) {
    let config_path = settings_config_path();
    let settings = DockSettings::load_settings(config_path.clone());
    let mut cached_settings = Some(settings.clone());
    let _ = sender.send(DockWindowInnerInput::PluginList(settings.plugins));
    let _ = sender.send(DockWindowInnerInput::Position(settings.position));
    let _ = sender.send(DockWindowInnerInput::Scale(settings.scale));

    let (mut watcher, mut rx) = match async_watcher() {
        Ok(res) => res,
        Err(e) => {
            eprintln!("{}", e);
            return;
        }
    };

    // if configs do not exist, they will not be monitored
    if let Some(settings_path) = settings_config_path() {
        if let Err(e) = watcher.watch(&settings_path, RecursiveMode::NonRecursive) {
            eprintln!("{}", e);
            return;
        };
    }
    let config_path = match config_path {
        Some(p) => p,
        None => return,
    };
    while let Some(res) = rx.next().await {
        match res {
            Ok(event)
                if event.kind == EventKind::Access(AccessKind::Close(AccessMode::Write))
                    || event.kind == EventKind::Modify(ModifyKind::Any) =>
            {
                let mut p = config_path.clone();
                p.push("settings.ron");
                let settings =
                    match File::open(p).map(|file| ron::de::from_reader::<_, DockSettings>(file)) {
                        Ok(Ok(s)) => s,
                        _ => continue,
                    };
                dbg!((settings.clone(), cached_settings.clone()));
                if cached_settings.is_none() {
                    cached_settings = Some(settings.clone());
                    let _ = sender.send(DockWindowInnerInput::PluginList(settings.plugins));
                    let _ = sender.send(DockWindowInnerInput::Position(settings.position));
                    let _ = sender.send(DockWindowInnerInput::Scale(settings.scale));
                } else {
                    let old_settings = cached_settings.clone().unwrap();
                    let new_settings = settings.clone();
                    let _ = sender.send(DockWindowInnerInput::PluginList(settings.plugins));
                    if old_settings.position != settings.position {
                        dbg!((old_settings.position, settings.position));
                        let _ = sender.send(DockWindowInnerInput::Position(settings.position));
                    } else if old_settings.scale != settings.scale {
                        let _ = sender.send(DockWindowInnerInput::Scale(settings.scale));
                    }
                    cached_settings = Some(new_settings);
                }
            }

            e => {
                dbg!(e);
            }
        }
    }
}

fn settings_config_path() -> Option<PathBuf> {
    let mut data_dirs = vec![gtk4::glib::user_config_dir()];
    data_dirs.append(&mut gtk4::glib::system_config_dirs());
    for mut d in data_dirs {
        d.push(crate::ID);
        let mut f = d.clone();
        f.push("settings.ron");
        if f.exists() {
            return Some(d);
        }
    }
    None
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DockSettings {
    pub(super) plugins: Vec<String>,
    pub(super) position: Position,
    pub(super) scale: i32,
}
impl DockSettings {
    pub fn load_settings(p: Option<PathBuf>) -> Self {
        match p
            .map(|mut p| {
                p.push("settings.ron");
                File::open(p)
            })
            .map(|file| ron::de::from_reader::<_, DockSettings>(file?))
        {
            Some(Ok(s)) => s,
            _ => DockSettings::default(),
        }
    }
}
impl Default for DockSettings {
    fn default() -> Self {
        Self {
            plugins: vec!["apps_plugin".into()],
            position: Position::Bottom,
            scale: 1,
        }
    }
}
