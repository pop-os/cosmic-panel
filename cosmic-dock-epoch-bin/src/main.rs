// SPDX-License-Identifier: MPL-2.0-only

use anyhow::Result;
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
    let usage = "USAGE: cosmic-dock-epoch <profile name>";
    let profile = match arg.as_ref().map(|s| &s[..]) {
        Some(arg) if arg == "--help" || arg == "-h" => {
            println!("{}", usage);
            std::process::exit(1);
        }
        Some(profile) => profile,
        None => {
            println!("{}", usage);
            std::process::exit(1);
        }
    };

    dock_xdg_wrapper(log, profile)?;
    Ok(())
}
