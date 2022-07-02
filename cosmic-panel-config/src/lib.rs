// SPDX-License-Identifier: MPL-2.0-only

//! Config for cosmic-panel

use slog::Logger;
use xdg_shell_wrapper::config::{WrapperConfig, KeyboardInteractivity, Layer};
use std::{collections::HashMap, env, fs::File, ops::Range, time::Duration};

use sctk::reexports::protocols::wlr::unstable::layer_shell::v1::client::{
    zwlr_layer_shell_v1, zwlr_layer_surface_v1,
};
use serde::{Deserialize, Serialize};
use xdg::BaseDirectories;

/// Edge to which the panel is anchored
#[derive(Debug, Deserialize, Serialize, Copy, Clone)]
pub enum PanelAnchor {
    /// anchored to left edge
    Left,
    /// anchored to right edge
    Right,
    /// anchored to top edge
    Top,
    /// anchored to bottom edge
    Bottom,
}

impl Default for PanelAnchor {
    fn default() -> Self {
        PanelAnchor::Top
    }
}

impl TryFrom<zwlr_layer_surface_v1::Anchor> for PanelAnchor {
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

impl Into<zwlr_layer_surface_v1::Anchor> for PanelAnchor {
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

#[cfg(feature = "gtk4")]
use gtk4::Orientation;

#[cfg(feature = "gtk4")]

impl Into<Orientation> for PanelAnchor {
    fn into(self) -> Orientation {
        match self {
            Self::Left | Self::Right => Orientation::Vertical,
            Self::Top | Self::Bottom => Orientation::Horizontal,
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
    /// profile name for this config
    name: String,
    /// edge which the panel is locked to
    pub anchor: PanelAnchor,
    /// gap between the panel and the edge of the ouput
    pub anchor_gap: bool,
    /// configured layer which the panel is on
    pub layer: Layer,
    /// configured interactivity level for the panel
    pub keyboard_interactivity: KeyboardInteractivity,
    /// configured size for the panel
    pub size: PanelSize,
    /// name of configured output (Intended for dock or panel), or None to place on active output (Intended for wrapping a single application)
    pub output: String,
    /// customized background, or
    pub background: CosmicPanelBackground,
    /// list of plugins on the left or top of the panel
    pub plugins_left: Option<Vec<String>>,
    /// list of plugins in the center of the panel
    pub plugins_center: Option<Vec<String>>,
    /// list of plugins on the right or bottom of the panel
    pub plugins_right: Option<Vec<String>>,
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
            name: String::new(),
            anchor: PanelAnchor::Top,
            anchor_gap: false,
            layer: Layer::Top,
            keyboard_interactivity: KeyboardInteractivity::None,
            size: PanelSize::M,
            output: "".to_string(),
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
    pub fn write(&self, log: Option<Logger>) -> anyhow::Result<()> {
        let mut configs = Self::get_configs(log);
        configs.insert(self.name.clone(), CosmicPanelConfig::default());
        let xdg = BaseDirectories::new()?;
        let f = xdg.place_config_file(CONFIG_PATH).unwrap();
        let f = File::create(f)?;
        ron::ser::to_writer_pretty(&f, &configs, ron::ser::PrettyConfig::default())?;
        Ok(())
    }

    fn get_configs(log: Option<Logger>) -> HashMap<String, Self> {
        let config_path =
            match BaseDirectories::new().map(|dirs| dirs.find_config_file(CONFIG_PATH)) {
                Ok(Some(path)) => path,
                Ok(None) => {
                    return HashMap::new();
                }
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

    /// get applet icon dimensions
    pub fn get_applet_icon_size(&self) -> u32 {
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
    pub fn get_hide_wait(&self) -> Option<Duration> {
        self.autohide
            .as_ref()
            .map(|AutoHide { wait_time, .. }| Duration::from_millis((*wait_time).into()))
    }

    /// if autohide is configured, returns the duration of time which the panel hide / show transition should last
    pub fn get_hide_transition(&self) -> Option<Duration> {
        self.autohide.as_ref().map(
            |AutoHide {
                 transition_time, ..
             }| Duration::from_millis((*transition_time).into()),
        )
    }

    /// if autohide is configured, returns the size of the handle of the panel which should be exposed
    pub fn get_hide_handle(&self) -> Option<u32> {
        self.autohide
            .as_ref()
            .map(|AutoHide { handle_size, .. }| *handle_size)
    }

    pub fn background(&self) -> CosmicPanelBackground {
        self.background.clone()
    }

    pub fn spacing(&self) -> u32 {
        self.spacing
    }

    pub fn exclusive_zone(&self) -> bool {
        self.exclusive_zone
    }

    pub fn autohide(&self) -> Option<AutoHide> {
        self.autohide.clone()
    }

    /// get whether the panel should expand to cover the edges of the output
    pub fn expand_to_edges(&self) -> bool {
        self.expand_to_edges || self.plugins_left.is_some() || self.plugins_right.is_some()
    }

    pub fn plugins_left(&self) -> Option<Vec<String>> {
        self.plugins_left.clone()
    }

    pub fn plugins_center(&self) -> Option<Vec<String>> {
        self.plugins_center.clone()
    }

    pub fn plugins_right(&self) -> Option<Vec<String>> {
        self.plugins_right.clone()
    }

    pub fn anchor(&self) -> PanelAnchor {
        self.anchor.clone()
    }

    pub fn padding(&self) -> u32 {
        self.padding
    }

    /// get constraints for the thickness of the panel bar
    pub fn get_dimensions(&self, output_dims: (u32, u32)) -> (Option<Range<u32>>, Option<Range<u32>>) {
            let mut bar_thickness = match &self.size {
                PanelSize::XS => (8..61),
                PanelSize::S => (8..81),
                PanelSize::M => (8..101),
                PanelSize::L => (8..121),
                PanelSize::XL => (8..141),
                PanelSize::Custom(c) => c.clone(),
            };
            assert!(2 * self.padding < bar_thickness.end);
            bar_thickness.end -= 2 * self.padding;
    
            match self.anchor {
                PanelAnchor::Left | PanelAnchor::Right => (
                    Some(bar_thickness),
                    if self.expand_to_edges() {
                        Some(output_dims.1..output_dims.1 + 1)
                    } else {
                        None
                    },
                ),
                PanelAnchor::Top | PanelAnchor::Bottom => (
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
    
}

impl WrapperConfig for CosmicPanelConfig {
    fn output(&self) -> Option<String> {
        Some(self.output.clone())
    }

    fn layer(&self) -> zwlr_layer_shell_v1::Layer {
        self.layer.into()
    }

    fn keyboard_interactivity(&self) -> zwlr_layer_surface_v1::KeyboardInteractivity {
        self.keyboard_interactivity.into()
    }

    fn name(&self) -> &str {
        &self.name
    }
}
