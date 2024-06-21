// SPDX-License-Identifier: MPL-2.0

use std::fmt;

use serde::{Deserialize, Serialize};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

/// Layer which the cosmic panel is on
#[derive(Debug, Deserialize, Serialize, Copy, Clone, PartialEq, Eq)]
pub enum Layer {
    /// background layer
    Background,
    /// Bottom layer
    Bottom,
    /// Top layer
    Top,
    /// Overlay layer
    Overlay,
}

impl From<zwlr_layer_shell_v1::Layer> for Layer {
    fn from(layer: zwlr_layer_shell_v1::Layer) -> Self {
        match layer {
            zwlr_layer_shell_v1::Layer::Background => Self::Background,
            zwlr_layer_shell_v1::Layer::Bottom => Self::Bottom,
            zwlr_layer_shell_v1::Layer::Top => Self::Top,
            zwlr_layer_shell_v1::Layer::Overlay => Self::Overlay,
            _ => Self::Top,
        }
    }
}

impl From<Layer> for zwlr_layer_shell_v1::Layer {
    fn from(val: Layer) -> Self {
        match val {
            Layer::Background => zwlr_layer_shell_v1::Layer::Background,
            Layer::Bottom => zwlr_layer_shell_v1::Layer::Bottom,
            Layer::Top => zwlr_layer_shell_v1::Layer::Top,
            Layer::Overlay => zwlr_layer_shell_v1::Layer::Overlay,
        }
    }
}

/// Interactivity level of the cosmic panel
#[derive(Debug, Deserialize, Serialize, Copy, Clone, PartialEq, Eq)]
pub enum KeyboardInteractivity {
    /// Not interactible
    None,
    /// Only surface which is interactible
    Exclusive,
    /// Interactible when given keyboard focus
    OnDemand,
}

impl From<zwlr_layer_surface_v1::KeyboardInteractivity> for KeyboardInteractivity {
    fn from(kb: zwlr_layer_surface_v1::KeyboardInteractivity) -> Self {
        match kb {
            zwlr_layer_surface_v1::KeyboardInteractivity::None => Self::None,
            zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive => Self::Exclusive,
            zwlr_layer_surface_v1::KeyboardInteractivity::OnDemand => Self::OnDemand,
            _ => Self::None,
        }
    }
}

impl From<KeyboardInteractivity> for zwlr_layer_surface_v1::KeyboardInteractivity {
    fn from(val: KeyboardInteractivity) -> Self {
        match val {
            KeyboardInteractivity::None => zwlr_layer_surface_v1::KeyboardInteractivity::None,
            KeyboardInteractivity::Exclusive => zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive,
            KeyboardInteractivity::OnDemand => zwlr_layer_surface_v1::KeyboardInteractivity::OnDemand,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
pub enum WrapperOutput {
    All,
    Name(Vec<String>),
}

pub trait WrapperConfig: Clone + fmt::Debug + Default {
    fn outputs(&self) -> WrapperOutput;

    fn name(&self) -> &str;
}
