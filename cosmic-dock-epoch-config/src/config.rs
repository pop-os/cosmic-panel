// SPDX-License-Identifier: MPL-2.0-only

//! Config for cosmic-dock-epoch

use std::collections::HashMap;
use std::fs::File;
use std::ops::Range;

use sctk::reexports::protocols::wlr::unstable::layer_shell::v1::client::{
    zwlr_layer_shell_v1, zwlr_layer_surface_v1,
};
use serde::{Deserialize, Serialize};
use xdg::BaseDirectories;

/// Edge to which the dock is anchored
#[derive(Debug, Deserialize, Serialize, Copy, Clone)]
pub enum Anchor {
    /// anchored to left edge
    Left,
    /// anchored to right edge
    Right,
    /// anchored to top edge
    Top,
    /// anchored to bottom edge
    Bottom,
}

impl TryFrom<zwlr_layer_surface_v1::Anchor> for Anchor {
    type Error = anyhow::Error;
    fn try_from(align: zwlr_layer_surface_v1::Anchor) -> Result<Self, Self::Error> {
        if align.contains(zwlr_layer_surface_v1::Anchor::Left) {
            Ok(Self::Left)
        } else if align.contains(zwlr_layer_surface_v1::Anchor::Right) {
            Ok(Self::Right)
        } else if align.contains(zwlr_layer_surface_v1::Anchor::Top) {
            Ok(Self::Top)
        } else if align.contains(zwlr_layer_surface_v1::Anchor::Bottom) {
            Ok(Self::Bottom)
        } else {
            anyhow::bail!("Invalid Anchor")
        }
    }
}

impl Into<zwlr_layer_surface_v1::Anchor> for Anchor {
    fn into(self) -> zwlr_layer_surface_v1::Anchor {
        let mut anchor = zwlr_layer_surface_v1::Anchor::empty();
        match self {
            Self::Left => {
                anchor.insert(zwlr_layer_surface_v1::Anchor::Left);
            }
            Self::Right => {
                anchor.insert(zwlr_layer_surface_v1::Anchor::Right);
            }
            Self::Top => {
                anchor.insert(zwlr_layer_surface_v1::Anchor::Top);
            }
            Self::Bottom => {
                anchor.insert(zwlr_layer_surface_v1::Anchor::Bottom);
            }
        };
        anchor
    }
}

/// Layer which the cosmic dock is on
#[derive(Debug, Deserialize, Serialize, Copy, Clone)]
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

impl Into<zwlr_layer_shell_v1::Layer> for Layer {
    fn into(self) -> zwlr_layer_shell_v1::Layer {
        match self {
            Self::Background => zwlr_layer_shell_v1::Layer::Background,
            Self::Bottom => zwlr_layer_shell_v1::Layer::Bottom,
            Self::Top => zwlr_layer_shell_v1::Layer::Top,
            Self::Overlay => zwlr_layer_shell_v1::Layer::Overlay,
        }
    }
}

/// Interactivity level of the cosmic dock
#[derive(Debug, Deserialize, Serialize, Copy, Clone)]
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

impl Into<zwlr_layer_surface_v1::KeyboardInteractivity> for KeyboardInteractivity {
    fn into(self) -> zwlr_layer_surface_v1::KeyboardInteractivity {
        match self {
            Self::None => zwlr_layer_surface_v1::KeyboardInteractivity::None,
            Self::Exclusive => zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive,
            Self::OnDemand => zwlr_layer_surface_v1::KeyboardInteractivity::OnDemand,
        }
    }
}

/// Configurable size for the cosmic dock
#[derive(Debug, Deserialize, Serialize, Clone)]
pub enum DockSize {
    /// XS
    XS,
    /// S
    S,
    /// M
    M,
    /// L
    L,
    /// XL
    XL,
    /// Custom Dock Size range,
    Custom(Range<u32>),
}

/// configurable background color for the cosmic dock
#[derive(Debug, Deserialize, Serialize, Clone)]
pub enum CosmicDockBackground {
    /// theme default color
    ThemeDefault,
    /// RGBA
    Color([u8; 4]),
}

/// Config structure for the cosmic dock
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CosmicDockConfig {
    /// edge which the dock is locked to
    pub anchor: Anchor,
    /// gap between the dock and the edge of the ouput
    pub anchor_gap: bool,
    /// configured layer which the dock is on
    pub layer: Layer,
    /// configured interactivity level for the dock
    pub keyboard_interactivity: KeyboardInteractivity,
    /// configured size for the dock
    pub size: DockSize,
    /// configured output, or None to place on all outputs
    pub output: Option<String>,
    /// customized background, or
    pub background: Option<CosmicDockBackground>,
    /// list of plugins on the left or top of the dock
    pub plugins_left: Vec<String>,
    /// list of plugins in the center of the dock
    pub plugins_center: Vec<String>,
    /// list of plugins on the right or bottom of the dock
    pub plugins_right: Vec<String>,
    /// whether the dock should stretch to the edges of output
    pub expand_to_edges: bool
}

impl Default for CosmicDockConfig {
    fn default() -> Self {
        Self {
            anchor: Anchor::Top,
            anchor_gap: false,
            layer: Layer::Top,
            keyboard_interactivity: KeyboardInteractivity::OnDemand,
            size: DockSize::M,
            output: None,
            background: None,
            plugins_left: Default::default(),
            plugins_center: Default::default(),
            plugins_right: Default::default(),
            expand_to_edges: true,
        }
    }
}

impl CosmicDockConfig {
    /// load config with the provided name
    pub fn load(name: &str) -> Self {
        match Self::get_configs().remove(name.into()) {
            Some(c) => c,
            _ => Self::default(),
        }
    }

    /// write config to config file
    pub fn write(&self, name: &str) -> anyhow::Result<()> {
        let mut configs = Self::get_configs();
        configs.insert(name.into(), CosmicDockConfig::default());
        let xdg = BaseDirectories::new()?;
        let f = xdg
            .place_config_file("xdg-shell-wrapper/config.ron")
            .unwrap();
        let f = File::create(f)?;
        ron::ser::to_writer_pretty(&f, &configs, ron::ser::PrettyConfig::default())?;
        return Ok(());
    }

    fn get_configs() -> HashMap<String, Self> {
        match BaseDirectories::new()
            .map(|dirs| dirs.find_config_file("xdg-shell-wrapper/config.ron"))
            .map(|c| c.map(|c| File::open(c)))
            .map(|file| {
                file.map(|file| ron::de::from_reader::<_, HashMap<String, CosmicDockConfig>>(file?))
            }) {
            Ok(Some(Ok(c))) => c,
            _ => HashMap::new(),
        }
    }
    
    /// get whether the dock should expand to cover the edges of the output
    pub fn expand_to_edges(&self) -> bool {
        self.expand_to_edges || self.plugins_left.len() > 0 || self.plugins_right.len() > 0
    }

    /// get constraints for the thickness of the dock bar
    pub fn get_dimensions(&self, output_dims: (u32, u32)) -> (Option<Range<u32>>, Option<Range<u32>>) {
        let bar_thickness = match &self.size {
            DockSize::XS => (10..41),
            DockSize::S => (10..61),
            DockSize::M => (10..81),
            DockSize::L => (10..101),
            DockSize::XL => (10..121),
            DockSize::Custom(c) => c.clone(),
        };

        match self.anchor {
            Anchor::Left | Anchor::Right => (Some(bar_thickness),  if self.expand_to_edges() {Some(output_dims.1..output_dims.1)} else {None}),
            Anchor::Top | Anchor::Bottom => (if self.expand_to_edges() {Some(output_dims.1..output_dims.0)} else {None}, Some(bar_thickness)),
        }
    }
}
