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

    // TODO load config
    dock_xdg_wrapper(log, CosmicDockConfig::default())?;
    Ok(())
}
