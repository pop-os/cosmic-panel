use ccs::*;
use cosmic_plugin::PluginManager;
use futures::{channel::mpsc::Receiver, SinkExt, StreamExt};
use gtk::{glib, prelude::*};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::ffi::OsString;
use std::path::Path;
use std::{fs::File, path::PathBuf};
extern crate notify;
use std::sync::mpsc::channel;
use std::time::Duration;
pub enum DockWindowInnerInput {
    PluginList(Vec<String>),
}

component! {
    #[derive(Default)]
    pub struct DockWindowInner(()) {
        pub plugin_manager: PluginManager,
    }

    pub struct DockWindowInnerWidgets(gtk::Box) {
        applet_box: gtk4::Box,
    }

    type Input = DockWindowInnerInput;
    type Output = ();

    fn init_view(self, _args, input, _output) {

        ccs::spawn_local(async_watch_plugins(input.clone()));

        ccs::view! {
            root = gtk4::Box {
                set_orientation:gtk4::Orientation::Horizontal,
                set_spacing: 0,
                set_vexpand: true,
                set_hexpand: true,
                // set_halign: gtk4::Align::Center,
                // set_valign: gtk4::Align::Center,
                add_css_class: "dock",
                append: applet_box = &gtk4::Box {
                    set_orientation: gtk4::Orientation::Horizontal,
                    set_spacing: 0,
                    set_vexpand: true,
                    set_hexpand: true,
                    set_valign: gtk4::Align::Center,
                    set_halign: gtk4::Align::Center,
                    append: testlabel = &gtk::Label {
                        set_text: "hi"
                    }
                }
            }
        }

        (DockWindowInnerWidgets {applet_box}, root)
    }

    fn update(self, widgets, message, _input, _output) {
        match message {
            DockWindowInnerInput::PluginList(plugin_filenames) => {
                dbg!(&plugin_filenames);
                while let Some(c) = widgets.applet_box.first_child() {
                    widgets.applet_box.remove(&c);
                }
                for f in plugin_filenames {
                    let mut path_dir = glib::user_data_dir();
                    path_dir.push(crate::ID);
                    std::fs::create_dir_all(&path_dir).expect("Could not create directory.");
                    path_dir.push("plugins");
                    std::fs::create_dir_all(&path_dir).expect("Could not create directory.");
                    let mut path = path_dir.clone();
                    path.push(f);

                    if let Err(e) = unsafe { self.plugin_manager.load_plugin(path) } {
                        eprint!("{}", e);
                    }
                }
                for applet in self.plugin_manager.applets() {
                    widgets.applet_box.append(applet);
                }

            }
        }
    }
}

fn async_watcher() -> notify::Result<(RecommendedWatcher, Receiver<notify::Result<Event>>)> {
    use futures::channel::mpsc::channel;
    let (mut tx, rx) = channel(1);

    // Automatically select the best implementation for your platform.
    // You can also access each implementation directly e.g. INotifyWatcher.
    let watcher = RecommendedWatcher::new(move |res| {
        futures::executor::block_on(async {
            tx.send(res).await.unwrap();
        })
    })?;

    Ok((watcher, rx))
}

async fn async_watch_plugins(sender: Sender<DockWindowInnerInput>) {
    let mut cached_results: Vec<String> = vec![];
    if let Ok(file) = File::open(plugin_data_path()) {
        if let Ok(data) = serde_json::from_reader::<_, Vec<String>>(file) {
            cached_results = data.clone();
            let _ = sender.send(DockWindowInnerInput::PluginList(data));
        }
    }

    let (mut watcher, mut rx) = match async_watcher() {
        Ok(res) => res,
        Err(e) => {
            eprintln!("{}", e);
            return;
        }
    };

    // Add a path to be watched. All files and directories at that path and
    // below will be monitored for changes.
    if let Err(e) = watcher.watch(&plugin_data_path(), RecursiveMode::NonRecursive) {
        eprintln!("{}", e);
        return;
    };

    while let Some(res) = rx.next().await {
        match res {
            Ok(_event) => {
                if let Ok(file) = File::open(plugin_data_path()) {
                    if let Ok(data) = serde_json::from_reader::<_, Vec<String>>(file) {
                        if cached_results.len() == data.len()
                            && data
                                .iter()
                                .zip(cached_results.iter())
                                .filter(|(a, b)| a != b)
                                .count()
                                == 0
                        {
                            continue;
                        }
                        cached_results = data.clone();
                        let _ = sender.send(DockWindowInnerInput::PluginList(data));
                    }
                }
            }
            Err(e) => eprintln!("watch error: {:?}", e),
        }
    }
}

// fn load_plugins() -> PluginManager {
//     let mut plugin_manager = PluginManager::new();
//     let mut path_dir = glib::user_data_dir();
//     path_dir.push(crate::ID);
//     std::fs::create_dir_all(&path_dir).expect("Could not create directory.");
//     path_dir.push("plugins");
//     std::fs::create_dir_all(&path_dir).expect("Could not create directory.");
//     let mut path = path_dir.clone();
//     path.push("libcosmic_dock_plugin_uwu.so");

//     unsafe { plugin_manager.load_plugin(path) }.unwrap();
//     for applet in plugin_manager.applets() {
//         self.append(applet);
//     }
//     plugin_manager
// }
fn plugin_data_path() -> PathBuf {
    let mut path = glib::user_data_dir();
    path.push(crate::ID);
    std::fs::create_dir_all(&path).expect("Could not create directory.");
    path.push("plugins.json");
    path
}

fn store_plugins(plugin_manager: PluginManager) {
    // Save state in file
    let file = File::create(plugin_data_path()).expect("Could not create json file.");
    serde_json::to_writer_pretty(file, &plugin_manager.filenames())
        .expect("Could not write data to json file");
}
