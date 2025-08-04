pub mod background;
pub mod overflow_button;
pub mod overflow_popup;
pub mod target;
use target::SpaceTarget;

use std::borrow::Cow;

use crate::space::Spacer;
use background::BackgroundElement;
use overflow_button::OverflowButtonElement;
use overflow_popup::OverflowPopupElement;
use smithay::{
    desktop::{space::SpaceElement, Window},
    space_elements,
    wayland::{seat::WaylandFocus, shell::xdg::ToplevelSurface},
};

pub trait PanelSpaceElement
where
    Self: SpaceElement + Clone + PartialEq,
    SpaceTarget: TryFrom<Self>,
{
    fn toplevel(&self) -> Option<&ToplevelSurface>;
}

space_elements! {
    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    pub CosmicMappedInternal;
    OverflowButton=OverflowButtonElement,
    Window=Window,
    Background=BackgroundElement,
    Spacer=Spacer
}

impl PanelSpaceElement for CosmicMappedInternal {
    fn toplevel(&self) -> Option<&ToplevelSurface> {
        match self {
            CosmicMappedInternal::Window(w) => w.toplevel(),
            _ => None,
        }
    }
}

impl WaylandFocus for CosmicMappedInternal {
    fn wl_surface(
        &self,
    ) -> Option<Cow<'_, smithay::reexports::wayland_server::protocol::wl_surface::WlSurface>> {
        match self {
            CosmicMappedInternal::Window(w) => w.wl_surface(),
            _ => None,
        }
    }
}

space_elements! {
    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    pub PopupMappedInternal;
    Popup=OverflowPopupElement,
    Window=Window
}

impl PanelSpaceElement for PopupMappedInternal {
    fn toplevel(&self) -> Option<&ToplevelSurface> {
        match self {
            PopupMappedInternal::Window(w) => w.toplevel(),
            _ => None,
        }
    }
}

impl WaylandFocus for PopupMappedInternal {
    fn wl_surface(
        &self,
    ) -> Option<Cow<'_, smithay::reexports::wayland_server::protocol::wl_surface::WlSurface>> {
        match self {
            PopupMappedInternal::Window(w) => w.wl_surface(),
            _ => None,
        }
    }
}
