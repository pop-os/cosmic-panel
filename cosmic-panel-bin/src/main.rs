// SPDX-License-Identifier: MPL-2.0-only

use anyhow::Result;
use slog::{o, Drain};
use smithay::reexports::calloop;
use xdg_shell_wrapper::run;

mod space;

fn main() -> Result<()> {
    dbg!(std::time::Instant::now());
    let log = slog::Logger::root(
        slog_async::Async::default(slog_term::term_full().fuse()).fuse(),
        o!(),
    );

    let _guard = slog_scope::set_global_logger(log.clone());
    slog_stdlog::init().expect("Could not setup log backend");

    let arg = std::env::args().nth(1);
    let usage = "USAGE: cosmic-panel <profile name>";
    let config = match arg.as_ref().map(|s| &s[..]) {
        Some(arg) if arg == "--help" || arg == "-h" => {
            println!("{}", usage);
            std::process::exit(1);
        }
        Some(profile) => cosmic_panel_config::CosmicPanelConfig::load(profile, Some(log.clone()))?,
        None => {
            println!("{}", usage);
            std::process::exit(1);
        }
    };

    let event_loop = calloop::EventLoop::try_new()?;
    run(space::PanelSpace::new(config, log), event_loop)?;
    Ok(())
}
