//! PanelSpace is a container for all running panels, spawning each as a separate process and compositing them in a layer shell surface as configured
//! PanelSpace *partially* implements the WrapperSpace abstraction

mod layout;
mod panel_space;
mod popup;
mod render;
mod wrapper_space;

pub(crate) use panel_space::{AppletMsg, PanelSpace};
pub use wrapper_space::*;

#[derive(Debug)]
pub enum Alignment {
    Left,
    Center,
    Right,
}
