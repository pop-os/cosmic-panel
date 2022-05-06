// SPDX-License-Identifier: MPL-2.0-only

use std::collections::HashMap;
use std::fs::File;

use serde::{Deserialize, Serialize};
use sctk::reexports::protocols::wlr::unstable::layer_shell::v1::client::{
    zwlr_layer_shell_v1, zwlr_layer_surface_v1,
};
use xdg::BaseDirectories;

#[derive(Debug, Deserialize, Serialize, Copy, Clone)]
pub enum Anchor {
    Left,
    Right,
    Top,
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

#[derive(Debug, Deserialize, Serialize, Copy, Clone)]
pub enum Layer {
    Background,
    Bottom,
    Top,
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

#[derive(Debug, Deserialize, Serialize, Copy, Clone)]
pub enum KeyboardInteractivity {
    None,
    Exclusive,
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

#[derive(Debug, Deserialize, Serialize, Clone)]
pub enum DockSize {
    XS,
    S,
    M,
    L,
    XL,
    Custom(u32),
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CosmicDockConfig {
    pub anchor: Anchor,
    pub layer: Layer,
    pub keyboard_interactivity: KeyboardInteractivity,
    pub size: DockSize,
    pub output: Option<String>,
    pub plugins_left: Vec<String>,
    pub plugins_center: Vec<String>,
    pub plugins_right: Vec<String>,
}

impl Default for CosmicDockConfig {
    fn default() -> Self {
        Self {
            anchor: Anchor::Top,
            layer: Layer::Top,
            keyboard_interactivity: KeyboardInteractivity::OnDemand,
            size: DockSize::M,
            output: None,
            plugins_left: Default::default(),
            plugins_center: Default::default(),
            plugins_right: Default::default(),      
        }
    }
}

impl CosmicDockConfig {
    pub fn load(name: &str) -> Self {
        match Self::get_configs().remove(name.into()) {
            Some(c) => c,
            _ => Self::default(),
        }
    }

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
    
    pub fn get_dimensions(&self) -> (Option<u32>, Option<u32>)  {
        let bar_thickness = match self.size {
            DockSize::XS => 40,
            DockSize::S => 60,
            DockSize::M => 80,
            DockSize::L => 100,
            DockSize::XL => 120,
            DockSize::Custom(c) => c,
        };

        match self.anchor {
            Anchor::Left | Anchor::Right => (Some(bar_thickness), None),
            Anchor::Top | Anchor::Bottom => (None, Some(bar_thickness)),
        }
    }
}
