// SPDX-License-Identifier: MPL-2.0-only

use anyhow::Result;
use cosmic_dock_epoch_config::config::CosmicDockConfig;
use cosmic_dock_epoch_xdg_wrapper::dock_xdg_wrapper;
use slog::{o, Drain};

fn main() -> Result<()> {
    let log = slog::Logger::root(
        slog_async::Async::default(slog_term::term_full().fuse()).fuse(),
        o!(),
    );

    let _guard = slog_scope::set_global_logger(log.clone());
    slog_stdlog::init().expect("Could not setup log backend");

    let arg = std::env::args().nth(1);
    let usage =
        "USAGE: cosmic-dock-epoch --profile <profile name>";
    let config = match arg.as_ref().map(|s| &s[..]) {
        Some(arg) if arg == "--profile" || arg == "-p" => {
            if let Some(profile) = std::env::args().nth(2) {
                CosmicDockConfig::load(profile.as_str())
            } else {
                println!("{}", usage);
                std::process::exit(1);
            }
        }
        _ => {
            println!("{}", usage);
            std::process::exit(1);
        }
    };

    dock_xdg_wrapper(log, config)?;
    Ok(())
}
