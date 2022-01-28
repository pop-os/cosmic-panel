use ccs::*;
use cosmic_plugin::PluginManager;
use futures::{channel::mpsc::Receiver, SinkExt, StreamExt};
use gtk4::{glib, prelude::*, Orientation};
use notify::{
    event::{AccessKind, AccessMode, EventKind},
    Event, INotifyWatcher, RecursiveMode, Watcher,
};
use std::{fs::File, path::PathBuf};
extern crate notify;

pub enum DockWindowInnerInput {
    PluginList(Vec<String>),
    PluginUpdate(notify::Event),
    Orientation(Orientation),
    Scale(i32),
}
use serde::{Deserialize, Serialize};

component! {
    #[derive(Default)]
    pub struct DockWindowInnerModel {
        pub plugin_manager: PluginManager<'static>,
    }

    pub struct DockWindowInnerWidgets {
        applet_box: gtk4::Box,
    }

    type Input = DockWindowInnerInput;
    type Output = ();

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
                    set_spacing: 4,
                    set_vexpand: true,
                    set_hexpand: true,
                    set_valign: gtk4::Align::Center,
                    set_halign: gtk4::Align::Center,
                }
        }
        root.append(&applet_box);
        ComponentInner
        { model: DockWindowInnerModel{plugin_manager}, widgets: DockWindowInnerWidgets {applet_box}, input, output}
    }

    fn update(component, message) {
        let ComponentInner { widgets, model, .. } = component;
        match message {
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
                    // EventKind::Modify(_) => {
                    //     for f in e.paths {
                    //         if let Some(applet_to_remove) = model.plugin_manager.library_path_to_applet(&f) {
                    //             widgets.applet_box.remove(&applet_to_remove);
                    //         }
                    //         unsafe { model.plugin_manager.unload_plugin(&f) }
                    //     }
                    // }
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
            DockWindowInnerInput::Orientation(o) => {
                while let Some(c) = widgets.applet_box.first_child() {
                    if let Ok(b) = c.downcast::<gtk4::Box>() {
                        b.set_orientation(o)
                    }
                }
            }
            DockWindowInnerInput::Scale(s) => {
                dbg!(s);
            }
        }
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
            tx.send(res).await.unwrap();
        })
    })?;

    Ok((watcher, rx))
}

async fn async_watch_plugin_settings(sender: Sender<DockWindowInnerInput>) {
    let settings = DockSettings::load_settings();
    let mut cached_settings = Some(settings.clone());
    let _ = sender.send(DockWindowInnerInput::PluginList(settings.plugins));

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

    while let Some(res) = rx.next().await {
        match res {
            Ok(_event) => {
                let settings = match settings_config_path()
                    .ok_or("settings file missing")
                    .map(|f| File::open(f))
                    .map(|file| ron::de::from_reader::<_, DockSettings>(file?))
                {
                    Ok(Ok(s)) => s,
                    _ => DockSettings::default(),
                };
                if cached_settings.is_none() {
                    cached_settings = Some(settings.clone());
                    let _ = sender.send(DockWindowInnerInput::PluginList(settings.plugins));
                    let _ = sender.send(DockWindowInnerInput::Orientation(
                        settings.orientation.into(),
                    ));
                    let _ = sender.send(DockWindowInnerInput::Scale(settings.scale));
                } else {
                    let old_settings = cached_settings.clone().unwrap();
                    cached_settings = Some(settings.clone());
                    let _ = sender.send(DockWindowInnerInput::PluginList(settings.plugins));

                    // if old_settings.plugins.len() != settings.plugins.len()
                    //     || settings
                    //         .plugins
                    //         .iter()
                    //         .zip(old_settings.plugins.iter())
                    //         .filter(|(a, b)| a != b)
                    //         .count()
                    //         != 0
                    // {
                    //     cached_settings = Some(settings.clone());
                    //     let _ = sender.send(DockWindowInnerInput::PluginList(settings.plugins));
                    // } else
                    if old_settings.orientation != settings.orientation {
                        let _ = sender.send(DockWindowInnerInput::Orientation(
                            settings.orientation.into(),
                        ));
                    } else if old_settings.scale != settings.scale {
                        let _ = sender.send(DockWindowInnerInput::Scale(settings.scale));
                    }
                }
            }
            Err(e) => eprintln!("watch error: {:?}", e),
        }
    }
}

fn settings_config_path() -> Option<PathBuf> {
    let mut data_dirs = vec![gtk4::glib::user_config_dir()];
    data_dirs.append(&mut gtk4::glib::system_config_dirs());
    for mut p in data_dirs {
        p.push(crate::ID);
        p.push("settings.ron");
        if p.exists() {
            return Some(p);
        }
    }
    None
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DockSettings {
    pub(super) plugins: Vec<String>,
    pub(super) orientation: DockOrientation,
    pub(super) scale: i32,
}
impl DockSettings {
    pub fn load_settings() -> Self {
        match settings_config_path()
            .ok_or("settings file missing")
            .map(|f| File::open(f))
            .map(|file| ron::de::from_reader::<_, DockSettings>(file?))
        {
            Ok(Ok(s)) => s,
            _ => DockSettings::default(),
        }
    }
}
impl Default for DockSettings {
    fn default() -> Self {
        Self {
            plugins: vec!["uwu_plugin".into(), "app_plugin".into()],
            orientation: DockOrientation::Horizontal,
            scale: 1,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum DockOrientation {
    /// The element is in horizontal orientation.
    Horizontal,
    /// The element is in vertical orientation.
    Vertical,
    Unknown(i32),
}

impl From<Orientation> for DockOrientation {
    fn from(item: Orientation) -> Self {
        match item {
            Orientation::Horizontal => Self::Horizontal,
            Orientation::Vertical => Self::Vertical,
            Orientation::__Unknown(x) => Self::Unknown(x),
            _ => unimplemented!(),
        }
    }
}

impl Into<Orientation> for DockOrientation {
    fn into(self) -> Orientation {
        match self {
            Self::Horizontal => Orientation::Horizontal,
            Self::Vertical => Orientation::Vertical,
            Self::Unknown(x) => Orientation::__Unknown(x),
        }
    }
}
