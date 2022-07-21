// SPDX-License-Identifier: MPL-2.0-only

mod panel_space;
mod wrapper_space;

pub(crate) use panel_space::PanelSpace;
pub use wrapper_space::*;

#[derive(Debug)]
pub enum Alignment {
    Left,
    Center,
    Right,
}
