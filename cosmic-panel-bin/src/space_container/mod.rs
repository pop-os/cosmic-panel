// SPDX-License-Identifier: MPL-2.0-only

//! space container is a container for all running panels, each panel space is a separate panel
//! space container implements the WrapperSpace abstraction, calling handle events and other methods of its PanelSpaces as necessary

mod space_container;
mod wrapper_space;

pub use space_container::*;
pub use wrapper_space::*;
