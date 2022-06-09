// SPDX-License-Identifier: MPL-2.0-only

//! Config for cosmic-panel

use slog::Logger;
use std::{collections::HashMap, env, fmt, fs::File, ops::Range, time::Duration};

use sctk::reexports::protocols::wlr::unstable::layer_shell::v1::client::{
    zwlr_layer_shell_v1, zwlr_layer_surface_v1,
};
use serde::{Deserialize, Serialize};
use xdg::BaseDirectories;

/// Edge to which the panel is anchored
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
    /// anchored to center
    Center,
}

impl Default for Anchor {
    fn default() -> Self {
        Anchor::Top
    }
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
        } else if align.is_empty() {
            Ok(Self::Center)
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
            Self::Center => {
                anchor.insert(zwlr_layer_surface_v1::Anchor::empty());
            }
        };
        anchor
    }
}

#[cfg(feature = "gtk4")]
use gtk4::Orientation;

#[cfg(feature = "gtk4")]

impl Into<Orientation> for Anchor {
    fn into(self) -> Orientation {
        match self {
            Self::Left | Self::Right => Orientation::Vertical,
            Self::Top | Self::Bottom => Orientation::Horizontal,
            Self::Center => Orientation::Horizontal,
        }
    }
}
/// Layer which the cosmic panel is on
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

/// Interactivity level of the cosmic panel
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

/// Configurable size for the cosmic panel
#[derive(Debug, Deserialize, Serialize, Clone)]
pub enum PanelSize {
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
    /// Custom Panel Size range,
    Custom(Range<u32>),
}

/// configurable backgrounds for the cosmic panel
#[derive(Debug, Deserialize, Serialize, Clone)]
pub enum CosmicPanelBackground {
    /// theme default color
    ThemeDefault,
    /// RGBA
    Color([f32; 4]),
}

// TODO configurable interpolation type?
/// configurable autohide behavior
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AutoHide {
    /// time without pointer focus before hiding
    wait_time: u32,
    /// time that it should take to transition
    transition_time: u32,
    /// size of the handle in pixels
    /// should be > 0
    handle_size: u32,
}

/// Config structure for the cosmic panel
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CosmicPanelConfig {
    /// edge which the panel is locked to
    pub anchor: Anchor,
    /// gap between the panel and the edge of the ouput
    pub anchor_gap: bool,
    /// configured layer which the panel is on
    pub layer: Layer,
    /// configured interactivity level for the panel
    pub keyboard_interactivity: KeyboardInteractivity,
    /// configured size for the panel
    pub size: PanelSize,
    /// name of configured output (Intended for dock or panel), or None to place on active output (Intended for wrapping a single application)
    pub output: Option<String>,
    /// customized background, or
    pub background: CosmicPanelBackground,
    /// list of plugins on the left or top of the panel
    pub plugins_left: Option<Vec<(String, u32)>>,
    /// list of plugins in the center of the panel
    pub plugins_center: Option<Vec<(String, u32)>>,
    /// list of plugins on the right or bottom of the panel
    pub plugins_right: Option<Vec<(String, u32)>>,
    /// whether the panel should stretch to the edges of output
    pub expand_to_edges: bool,
    /// padding around the panel
    pub padding: u32,
    /// space between panel plugins
    pub spacing: u32,
    /// exclusive zone
    pub exclusive_zone: bool,
    /// enable autohide feature with the transitions lasting the supplied wait time and duration in millis
    pub autohide: Option<AutoHide>,
}

impl Default for CosmicPanelConfig {
    fn default() -> Self {
        Self {
            anchor: Anchor::Top,
            anchor_gap: false,
            layer: Layer::Top,
            keyboard_interactivity: KeyboardInteractivity::None,
            size: PanelSize::M,
            output: Some("".to_string()),
            background: CosmicPanelBackground::Color([0.5, 0.0, 0.5, 0.5]),
            plugins_left: Default::default(),
            plugins_center: Default::default(),
            plugins_right: Default::default(),
            expand_to_edges: true,
            padding: 4,
            spacing: 4,
            exclusive_zone: true,
            autohide: Some(AutoHide {
                wait_time: 1000,
                transition_time: 200,
                handle_size: 4,
            }),
        }
    }
}

static CONFIG_PATH: &'static str = "cosmic-panel/config.ron";

impl CosmicPanelConfig {
    /// load config with the provided name
    pub fn load(name: &str, log: Option<Logger>) -> anyhow::Result<Self> {
        Self::get_configs(log)
            .remove(name)
            .ok_or_else(|| anyhow::anyhow!(format!("Config profile for {} failed to load", name)))
    }

    /// write config to config file
    pub fn write(&self, name: &str, log: Option<Logger>) -> anyhow::Result<()> {
        let mut configs = Self::get_configs(log);
        configs.insert(name.into(), CosmicPanelConfig::default());
        let xdg = BaseDirectories::new()?;
        let f = xdg.place_config_file(CONFIG_PATH).unwrap();
        let f = File::create(f)?;
        ron::ser::to_writer_pretty(&f, &configs, ron::ser::PrettyConfig::default())?;
        Ok(())
    }

    fn get_configs(log: Option<Logger>) -> HashMap<String, Self> {
        let config_path = match BaseDirectories::new().map(|dirs| dirs.find_config_file(CONFIG_PATH)) {
            Ok(Some(path)) => path,
            Ok(None) => { return HashMap::new(); }
            Err(err) => {
                if let Some(log) = log {
                    slog::error!(log, "Failed to get config path: {}", err);
                }
                return HashMap::new();
            }
        };
        let file = match File::open(&config_path) {
            Ok(file) => file,
            Err(err) => {
                if let Some(log) = log {
                    slog::error!(log, "Failed to open '{}': {}", config_path.display(), err);
                }
                return HashMap::new();
            }
        };
        match ron::de::from_reader::<_, HashMap<String, CosmicPanelConfig>>(file) {
            Ok(configs) => configs,
            Err(err) => {
                if let Some(log) = log {
                    slog::error!(log, "Failed to parse '{}': {}", config_path.display(), err);
                }
                HashMap::new()
            }
        }
    }

    /// Utility for loading the Cosmic Panel Config from the ENV variable COSMIC_DOCK_CONFIG
    pub fn load_from_env() -> anyhow::Result<Self> {
        env::var("COSMIC_DOCK_CONFIG").map(|c_name| CosmicPanelConfig::load(&c_name, None))?
    }
}

pub trait XdgWrapperConfig: Clone + fmt::Debug + Default {
    fn output(&self) -> Option<String>;
    fn anchor(&self) -> Anchor;
    fn padding(&self) -> u32;
    fn layer(&self) -> zwlr_layer_shell_v1::Layer;
    fn keyboard_interactivity(&self) -> zwlr_layer_surface_v1::KeyboardInteractivity;
    fn background(&self) -> CosmicPanelBackground {
        CosmicPanelBackground::Color([0.0, 0.0, 0.0, 0.0])
    }

    fn plugins_left(&self) -> Option<Vec<(String, u32)>> {
        None
    }

    fn plugins_center(&self) -> Option<Vec<(String, u32)>> {
        None
    }

    fn plugins_right(&self) -> Option<Vec<(String, u32)>> {
        None
    }

    fn spacing(&self) -> u32 {
        0
    }

    fn get_dimensions(&self, output_dims: (u32, u32)) -> (Option<Range<u32>>, Option<Range<u32>>);

    fn autohide(&self) -> Option<AutoHide> {
        None
    }

    fn exclusive_zone(&self) -> bool {
        false
    }

    fn get_hide_wait(&self) -> Option<Duration> {
        None
    }

    fn get_hide_transition(&self) -> Option<Duration> {
        None
    }

    fn get_hide_handle(&self) -> Option<u32> {
        None
    }

    fn get_applet_icon_size(&self) -> u32 {
        0
    }

    fn expand_to_edges(&self) -> bool {
        false
    }
}

impl XdgWrapperConfig for CosmicPanelConfig {
    fn plugins_left(&self) -> Option<Vec<(String, u32)>> {
        self.plugins_left.clone()
    }

    fn plugins_center(&self) -> Option<Vec<(String, u32)>> {
        self.plugins_center.clone()
    }

    fn plugins_right(&self) -> Option<Vec<(String, u32)>> {
        self.plugins_right.clone()
    }

    fn output(&self) -> Option<String> {
        self.output.clone()
    }

    fn anchor(&self) -> Anchor {
        self.anchor
    }

    fn padding(&self) -> u32 {
        self.padding
    }

    fn layer(&self) -> zwlr_layer_shell_v1::Layer {
        self.layer.into()
    }

    fn keyboard_interactivity(&self) -> zwlr_layer_surface_v1::KeyboardInteractivity {
        self.keyboard_interactivity.into()
    }

    /// get whether the panel should expand to cover the edges of the output
    fn expand_to_edges(&self) -> bool {
        self.expand_to_edges || self.plugins_left.is_some() || self.plugins_right.is_some()
    }

    /// get constraints for the thickness of the panel bar
    fn get_dimensions(&self, output_dims: (u32, u32)) -> (Option<Range<u32>>, Option<Range<u32>>) {
        let mut bar_thickness = match &self.size {
            PanelSize::XS => (8..41),
            PanelSize::S => (8..61),
            PanelSize::M => (8..81),
            PanelSize::L => (8..101),
            PanelSize::XL => (8..121),
            PanelSize::Custom(c) => c.clone(),
        };
        assert!(2 * self.padding < bar_thickness.end);
        bar_thickness.end -= 2 * self.padding;

        match self.anchor {
            Anchor::Left | Anchor::Right => (
                Some(bar_thickness),
                if self.expand_to_edges() {
                    Some(output_dims.1..output_dims.1 + 1)
                } else {
                    None
                },
            ),
            Anchor::Top | Anchor::Bottom => (
                if self.expand_to_edges() {
                    Some(output_dims.0..output_dims.0 + 1)
                } else {
                    None
                },
                Some(bar_thickness),
            ),
            _ => (None, None),
        }
    }

    /// get applet icon dimensions
    fn get_applet_icon_size(&self) -> u32 {
        match &self.size {
            PanelSize::XS => 18,
            PanelSize::S => 24,
            PanelSize::M => 36,
            PanelSize::L => 48,
            PanelSize::XL => 64,
            PanelSize::Custom(c) => c.end - self.padding,
        }
    }

    /// if autohide is configured, returns the duration of time which the panel should wait to hide when it has lost focus
    fn get_hide_wait(&self) -> Option<Duration> {
        self.autohide
            .as_ref()
            .map(|AutoHide { wait_time, .. }| Duration::from_millis((*wait_time).into()))
    }

    /// if autohide is configured, returns the duration of time which the panel hide / show transition should last
    fn get_hide_transition(&self) -> Option<Duration> {
        self.autohide.as_ref().map(
            |AutoHide {
                 transition_time, ..
             }| Duration::from_millis((*transition_time).into()),
        )
    }

    /// if autohide is configured, returns the size of the handle of the panel which should be exposed
    fn get_hide_handle(&self) -> Option<u32> {
        self.autohide
            .as_ref()
            .map(|AutoHide { handle_size, .. }| *handle_size)
    }

    fn exclusive_zone(&self) -> bool {
        self.exclusive_zone
    }

    fn background(&self) -> CosmicPanelBackground {
        self.background.clone()
    }

    fn spacing(&self) -> u32 {
        self.spacing
    }

    fn autohide(&self) -> Option<AutoHide> {
        self.autohide.clone()
    }
}
