// SPDX-License-Identifier: MPL-2.0-only

use std::{
    fs::File,
    io::{BufRead, BufReader},
};

use adw::{
    gdk::RGBA,
    gio::{self, FileMonitorEvent, FileMonitorFlags},
    glib, gtk,
    prelude::*,
    StyleManager,
};
use anyhow::Result;
use cosmic_panel_config::{CosmicPanelBackground, CosmicPanelContainerConfig};
use slog::{o, warn, Drain};
use smithay::reexports::calloop;
use xdg_shell_wrapper::{run, shared_state::GlobalState};

mod space;
mod space_container;

fn main() -> Result<()> {
    let log = slog::Logger::root(
        slog_async::Async::default(slog_term::term_full().fuse()).fuse(),
        o!(),
    );

    let _guard = slog_scope::set_global_logger(log.clone());
    slog_stdlog::init().expect("Could not setup log backend");

    let arg = std::env::args().nth(1);
    let usage = "USAGE: cosmic-panel";
    let config = match arg.as_ref().map(|s| &s[..]) {
        Some(arg) if arg == "--help" || arg == "-h" => {
            println!("{}", usage);
            std::process::exit(1);
        }
        None => match cosmic_panel_config::CosmicPanelContainerConfig::load() {
            Ok(c) => c,
            Err(e) => {
                warn!(
                    log.clone(),
                    "Falling back to default panel configuration: {}", e
                );
                CosmicPanelContainerConfig::default()
            }
        },
        _ => {
            println!("{}", usage);
            std::process::exit(1);
        }
    };

    let event_loop = calloop::EventLoop::try_new()?;
    if config
        .config_list
        .iter()
        .any(|c| matches!(c.background, CosmicPanelBackground::ThemeDefault(_)))
    {
        let (tx, rx) = calloop::channel::sync_channel(100);
        std::thread::spawn(move || -> anyhow::Result<()> {
            let _ = gtk::init();

            let path = xdg::BaseDirectories::with_prefix("gtk-4.0")
                .ok()
                .and_then(|xdg_dirs| xdg_dirs.find_config_file("cosmic.css"))
                .unwrap_or_else(|| "~/.config/gtk-4.0/cosmic.css".into());
            let cosmic_file = gio::File::for_path(path);
            let _cosmic_css_monitor = cosmic_file
                .monitor(FileMonitorFlags::all(), None::<&gio::Cancellable>)
                .ok()
                .map(|monitor| {
                    monitor.connect_changed(move |_monitor, file, _other_file, event| {
                        match event {
                            FileMonitorEvent::Deleted
                            | FileMonitorEvent::MovedOut
                            | FileMonitorEvent::Renamed => {
                                if adw::is_initialized() {
                                    let manager = StyleManager::default();
                                    let css = if manager.is_dark() {
                                        adw_user_colors_lib::colors::ColorOverrides::dark_default()
                                            .as_css()
                                    } else {
                                        adw_user_colors_lib::colors::ColorOverrides::light_default()
                                            .as_css()
                                    };
                                    let window_bg_color_pattern = "@define-color window_bg_color";
                                    if let Some(color) = css
                                        .rfind(window_bg_color_pattern)
                                        .and_then(|i| css.get(i + window_bg_color_pattern.len()..))
                                        .and_then(|color_str| {
                                            RGBA::parse(&color_str.trim().replace(";", "")).ok()
                                        })
                                    {
                                        let _ = tx.send([
                                            color.red(),
                                            color.green(),
                                            color.blue(),
                                            color.alpha(),
                                        ]);
                                    }
                                }
                            }
                            FileMonitorEvent::ChangesDoneHint
                            | FileMonitorEvent::Created
                            | FileMonitorEvent::MovedIn => {
                                let _ = tx.send([0.0, 0.0, 0.0, 0.0]);
                                let file = match File::open(file.path().unwrap()) {
                                    Ok(f) => f,
                                    _ => return,
                                };

                                let window_bg_color_pattern = "@define-color window_bg_color";
                                if let Some(color) = BufReader::new(file)
                                    .lines()
                                    .filter_map(|l| l.ok())
                                    .find_map(|line| {
                                        line.rfind(window_bg_color_pattern)
                                            .and_then(|i| {
                                                line.get(i + window_bg_color_pattern.len()..)
                                            })
                                            .and_then(|color_str| {
                                                RGBA::parse(&color_str.trim().replace(";", "")).ok()
                                            })
                                    })
                                {
                                    let _ = tx.send([
                                        color.red(),
                                        color.green(),
                                        color.blue(),
                                        color.alpha(),
                                    ]);
                                }
                            }
                            _ => {} // ignored
                        }
                    });
                    monitor
                });

            let main_loop = glib::MainLoop::new(None, false);
            main_loop.run();
            Ok(())
        });

        event_loop
            .handle()
            .insert_source(
                rx,
                |e, _, state: &mut GlobalState<space_container::SpaceContainer>| {
                    match e {
                        calloop::channel::Event::Msg(c) => state.space.set_theme_window_color(c),
                        calloop::channel::Event::Closed => {}
                    };
                },
            )
            .expect("failed to insert dbus event source");
    }

    run(
        space_container::SpaceContainer::new(config, log),
        event_loop,
    )?;
    Ok(())
}
