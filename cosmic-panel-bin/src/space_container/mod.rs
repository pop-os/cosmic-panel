//! space container is a container for all running panels, each panel space is a
//! separate panel space container implements the WrapperSpace abstraction,
//! calling handle events and other methods of its PanelSpaces as necessary

mod space_container;
pub(crate) mod toplevel;
pub(crate) mod workspace;
mod wrapper_space;

pub use space_container::*;
