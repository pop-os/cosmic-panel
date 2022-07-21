// SPDX-License-Identifier: MPL-2.0-only

use anyhow::Result;
use cosmic_panel_config::CosmicPanelContainerConfig;
use slog::{o, warn, Drain};
use smithay::reexports::calloop;
use xdg_shell_wrapper::run;

mod space;
mod space_container;

fn main() -> Result<()> {
    dbg!(std::time::Instant::now());
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
    run(
        space_container::SpaceContainer::new(config, log),
        event_loop,
    )?;
    Ok(())
}
