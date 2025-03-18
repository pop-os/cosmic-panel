//! Config for cosmic-panel

use std::{fmt::Display, ops::Range, str::FromStr, time::Duration};

use anyhow::bail;
use cosmic_config::{cosmic_config_derive::CosmicConfigEntry, Config, CosmicConfigEntry};
use sctk::shell::wlr_layer::Anchor;
use serde::{Deserialize, Serialize};
#[cfg(feature = "wayland-rs")]
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};
#[cfg(feature = "wayland-rs")]
use xdg_shell_wrapper_config::{KeyboardInteractivity, Layer, WrapperConfig, WrapperOutput};

use crate::{NAME, VERSION};

/// Edge to which the panel is anchored
#[derive(Debug, Deserialize, Serialize, Copy, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[derive(Default)]
pub enum PanelAnchor {
    /// anchored to left edge
    Left,
    /// anchored to right edge
    Right,
    /// anchored to top edge
    #[default]
    Top,
    /// anchored to bottom edge
    Bottom,
}

impl Display for PanelAnchor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PanelAnchor::Left => write!(f, "Left"),
            PanelAnchor::Right => write!(f, "Right"),
            PanelAnchor::Top => write!(f, "Top"),
            PanelAnchor::Bottom => write!(f, "Bottom"),
        }
    }
}

impl FromStr for PanelAnchor {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Left" => Ok(Self::Left),
            "Right" => Ok(Self::Right),
            "Top" => Ok(Self::Top),
            "Bottom" => Ok(Self::Bottom),
            _ => Err(anyhow::anyhow!("Not a valid PanelAnchor")),
        }
    }
}

#[cfg(feature = "wayland-rs")]
impl TryFrom<Anchor> for PanelAnchor {
    type Error = anyhow::Error;

    fn try_from(align: Anchor) -> Result<Self, Self::Error> {
        if align.contains(Anchor::LEFT) {
            Ok(Self::Left)
        } else if align.contains(Anchor::RIGHT) {
            Ok(Self::Right)
        } else if align.contains(Anchor::TOP) {
            Ok(Self::Top)
        } else if align.contains(Anchor::BOTTOM) {
            Ok(Self::Bottom)
        } else {
            anyhow::bail!("Invalid Anchor")
        }
    }
}

#[cfg(feature = "wayland-rs")]
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

#[cfg(feature = "wayland-rs")]
impl From<PanelAnchor> for zwlr_layer_surface_v1::Anchor {
    fn from(val: PanelAnchor) -> Self {
        let anchor = zwlr_layer_surface_v1::Anchor::all();
        match val {
            PanelAnchor::Left => anchor.difference(zwlr_layer_surface_v1::Anchor::Right),
            PanelAnchor::Right => anchor.difference(zwlr_layer_surface_v1::Anchor::Left),
            PanelAnchor::Top => anchor.difference(zwlr_layer_surface_v1::Anchor::Bottom),
            PanelAnchor::Bottom => anchor.difference(zwlr_layer_surface_v1::Anchor::Top),
        }
    }
}

#[cfg(feature = "wayland-rs")]
impl From<PanelAnchor> for Anchor {
    fn from(val: PanelAnchor) -> Self {
        let anchor = Anchor::all();
        match val {
            PanelAnchor::Left => anchor.difference(Anchor::RIGHT),
            PanelAnchor::Right => anchor.difference(Anchor::LEFT),
            PanelAnchor::Top => anchor.difference(Anchor::BOTTOM),
            PanelAnchor::Bottom => anchor.difference(Anchor::TOP),
        }
    }
}

/// Configurable size for the cosmic panel
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
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
}

impl PanelSize {
    /// get applet icon dimensions
    pub fn get_applet_icon_size(&self, is_symbolic: bool) -> u32 {
        if is_symbolic {
            match self {
                PanelSize::XS => 16,
                PanelSize::S => 20,
                PanelSize::M => 28,
                PanelSize::L => 32,
                PanelSize::XL => 48,
            }
        } else {
            match self {
                PanelSize::XS => 24,
                PanelSize::S => 32,
                PanelSize::M => 40,
                PanelSize::L => 48,
                PanelSize::XL => 56,
            }
        }
    }

    pub fn get_applet_padding(&self, is_symbolic: bool) -> u16 {
        if is_symbolic {
            match self {
                PanelSize::XS => 8,
                PanelSize::S => 10,
                PanelSize::M => 14,
                PanelSize::L => 16,
                PanelSize::XL => 16,
            }
        } else {
            match self {
                PanelSize::XS => 4,
                PanelSize::S => 4,
                PanelSize::M => 8,
                PanelSize::L => 8,
                PanelSize::XL => 12,
            }
        }
    }

    pub fn get_applet_icon_size_with_padding(&self, is_symbolic: bool) -> u32 {
        self.get_applet_icon_size(is_symbolic) + self.get_applet_padding(is_symbolic) as u32 * 2
    }
}

impl Display for PanelSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PanelSize::XS => write!(f, "XS"),
            PanelSize::S => write!(f, "S"),
            PanelSize::M => write!(f, "M"),
            PanelSize::L => write!(f, "L"),
            PanelSize::XL => write!(f, "XL"),
        }
    }
}

impl FromStr for PanelSize {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "XS" => Ok(Self::XS),
            "S" => Ok(Self::S),
            "M" => Ok(Self::M),
            "L" => Ok(Self::L),
            "XL" => Ok(Self::XL),
            _ => Err(anyhow::anyhow!("Not a valid PanelSize")),
        }
    }
}

/// configurable backgrounds for the cosmic panel
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub enum CosmicPanelBackground {
    /// theme default color with optional transparency
    ThemeDefault,
    /// theme default dark
    Dark,
    /// theme default light
    Light,
    /// RGBA
    Color([f32; 3]),
}

const fn _default_true() -> bool {
    true
}

// TODO configurable interpolation type?
/// configurable autohide behavior
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AutoHide {
    /// time in milliseconds without pointer focus before hiding
    pub wait_time: u32,
    /// time in milliseconds that it should take to transition
    pub transition_time: u32,
    /// size of the handle in pixels
    /// should be > 0
    pub handle_size: u32,
}

impl Default for AutoHide {
    fn default() -> Self {
        Self { wait_time: 1000, transition_time: 200, handle_size: 4 }
    }
}

/// Configuration for the panel's ouput
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub enum CosmicPanelOuput {
    /// show panel on all outputs
    All,
    /// show panel on the active output
    Active,
    /// show panel on a specific output
    Name(String),
}

impl Display for CosmicPanelOuput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CosmicPanelOuput::All => write!(f, "All"),
            CosmicPanelOuput::Active => write!(f, "Active"),
            CosmicPanelOuput::Name(n) => write!(f, "Name({})", n),
        }
    }
}

impl FromStr for CosmicPanelOuput {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "All" => Ok(Self::All),
            "Active" => Ok(Self::Active),
            s if s.len() >= 6 && &s[..5] == "Name(" && s.ends_with(')') => {
                Ok(Self::Name(s[5..s.len() - 1].to_string()))
            },
            _ => bail!("Failed to parse output."),
        }
    }
}

#[cfg(feature = "wayland-rs")]
impl From<CosmicPanelOuput> for WrapperOutput {
    fn from(val: CosmicPanelOuput) -> Self {
        match val {
            CosmicPanelOuput::All => WrapperOutput::All,
            CosmicPanelOuput::Active => WrapperOutput::Name(vec![]),
            CosmicPanelOuput::Name(n) => WrapperOutput::Name(vec![n]),
        }
    }
}

#[cfg(feature = "wayland-rs")]
// TODO refactor to have separate dock mode config & panel mode config
/// Config structure for the cosmic panel
#[derive(Debug, Deserialize, Serialize, Clone, CosmicConfigEntry)]
#[version = 1]
#[serde(deny_unknown_fields)]
pub struct CosmicPanelConfig {
    /// profile name for this config, should be unique
    pub name: String,
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
    /// name of configured output (Intended for dock or panel), or None to place
    /// on active output (Intended for wrapping a single application)
    pub output: CosmicPanelOuput,
    /// customized background, or
    pub background: CosmicPanelBackground,
    /// list of plugins on the left / top and right / bottom of the panel
    pub plugins_wings: Option<(Vec<String>, Vec<String>)>,
    /// list of plugins in the center of the panel
    pub plugins_center: Option<Vec<String>>,
    /// optional size override for wings
    pub size_wings: Option<(Option<PanelSize>, Option<PanelSize>)>,
    /// optional size override for center
    pub size_center: Option<PanelSize>,
    /// whether the panel should stretch to the edges of output
    pub expand_to_edges: bool,
    /// padding around the panel
    pub padding: u32,
    /// space between panel plugins
    pub spacing: u32,
    pub border_radius: u32,
    // TODO autohide & exclusive zone should not be able to both be enabled at once
    /// exclusive zone
    pub exclusive_zone: bool,
    /// enable autohide feature with the transitions lasting the supplied wait
    /// time and duration in millis
    pub autohide: Option<AutoHide>,
    /// margin between the panel and the edge of the output
    pub margin: u16,
    /// opacity of the panel
    pub opacity: f32,
    /// autohover popup delay duration in milliseconds
    /// If None, then it is disabled
    pub autohover_delay_ms: Option<u32>,
}

impl PartialEq for CosmicPanelConfig {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.anchor == other.anchor
            && self.anchor_gap == other.anchor_gap
            && self.layer == other.layer
            && self.keyboard_interactivity == other.keyboard_interactivity
            && self.size == other.size
            && self.output == other.output
            && self.background == other.background
            && self.plugins_wings == other.plugins_wings
            && self.plugins_center == other.plugins_center
            && self.size_center == other.size_center
            && self.size_wings == other.size_wings
            && self.expand_to_edges == other.expand_to_edges
            && self.padding == other.padding
            && self.spacing == other.spacing
            && self.border_radius == other.border_radius
            && self.exclusive_zone == other.exclusive_zone
            && self.autohide == other.autohide
            && self.margin == other.margin
            && self.size_center == other.size_center
            && self.size_wings == other.size_wings
            && (self.opacity - other.opacity).abs() < 0.01
    }
}

#[cfg(feature = "wayland-rs")]
impl Default for CosmicPanelConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            anchor: PanelAnchor::Top,
            anchor_gap: false,
            layer: Layer::Top,
            keyboard_interactivity: KeyboardInteractivity::None,
            size: PanelSize::M,
            output: CosmicPanelOuput::All,
            background: CosmicPanelBackground::ThemeDefault,
            plugins_wings: Default::default(),
            plugins_center: Default::default(),
            size_wings: Default::default(),
            size_center: Default::default(),
            expand_to_edges: true,
            padding: 4,
            spacing: 4,
            exclusive_zone: true,
            autohide: Some(AutoHide::default()),
            border_radius: 8,
            margin: 4,
            opacity: 0.8,
            autohover_delay_ms: Some(500),
        }
    }
}

#[derive(Clone, Debug)]
pub enum Side {
    WingStart,
    Center,
    WingEnd,
}

#[cfg(feature = "wayland-rs")]
impl CosmicPanelConfig {
    /// get the applet size given its side
    pub fn get_effective_applet_size(&self, side: Side) -> PanelSize {
        match side {
            Side::WingStart => {
                if let Some((Some(size), _)) = &self.size_wings {
                    size.clone()
                } else {
                    self.size.clone()
                }
            },
            Side::Center => {
                if let Some(size) = &self.size_center {
                    size.clone()
                } else {
                    self.size.clone()
                }
            },
            Side::WingEnd => {
                if let Some((_, Some(size))) = &self.size_wings {
                    size.clone()
                } else {
                    self.size.clone()
                }
            },
        }
    }
    /// get applet icon dimensions
    pub fn get_applet_icon_size(&self, is_symbolic: bool) -> u32 {
        self.size.get_applet_icon_size(is_symbolic)
    }

    pub fn get_applet_padding(&self, is_symbolic: bool) -> u16 {
        self.size.get_applet_padding(is_symbolic)
    }

    /// get the priority of the panel
    /// higher priority panels will be created first and given more space when
    /// competing for space
    pub fn get_priority(&self) -> u32 {
        let mut priority = if self.expand_to_edges() { 10000 } else { 0 };
        if self.autohide().is_none() {
            priority += 1000;
        }
        if self.margin == 0 {
            priority += 200;
        }
        if !self.anchor_gap {
            priority += 100;
        }
        if self.name.to_lowercase().contains("panel") {
            priority += 10;
        }
        priority
    }

    pub fn get_stack_priority(&self) -> u32 {
        let mut priority = if self.expand_to_edges() { 10000 } else { 0 };
        // XXX for stack priority, most significant value is autohide
        if self.autohide().is_none() {
            priority += 100000;
        }
        if self.margin == 0 {
            priority += 200;
        }
        if !self.anchor_gap {
            priority += 100;
        }
        if self.name.to_lowercase().contains("panel") {
            priority += 10;
        }
        priority
    }

    /// get margin between the panel and the edge of the output
    pub fn get_margin(&self) -> u16 {
        self.margin
    }

    /// get the effective anchor gap margin
    pub fn get_effective_anchor_gap(&self) -> u32 {
        if self.anchor_gap {
            self.margin as u32
        } else {
            0
        }
    }

    /// if autohide is configured, returns the duration of time which the panel
    /// should wait to hide when it has lost focus
    pub fn get_hide_wait(&self) -> Option<Duration> {
        self.autohide
            .as_ref()
            .map(|AutoHide { wait_time, .. }| Duration::from_millis((*wait_time).into()))
    }

    /// if autohide is configured, returns the duration of time which the panel
    /// hide / show transition should last
    pub fn get_hide_transition(&self) -> Option<Duration> {
        self.autohide.as_ref().map(|AutoHide { transition_time, .. }| {
            Duration::from_millis((*transition_time).into())
        })
    }

    /// if autohide is configured, returns the size of the handle of the panel
    /// which should be exposed
    pub fn get_hide_handle(&self) -> Option<u32> {
        self.autohide.as_ref().map(|AutoHide { handle_size, .. }| *handle_size)
    }

    pub fn background(&self) -> CosmicPanelBackground {
        self.background.clone()
    }

    pub fn spacing(&self) -> u32 {
        self.spacing
    }

    pub fn exclusive_zone(&self) -> bool {
        self.exclusive_zone || self.autohide.is_none()
    }

    pub fn autohide(&self) -> Option<AutoHide> {
        self.autohide.clone()
    }

    /// get whether the panel should expand to cover the edges of the output
    pub fn expand_to_edges(&self) -> bool {
        self.expand_to_edges
    }

    pub fn plugins_left(&self) -> Option<Vec<String>> {
        self.plugins_wings.as_ref().map(|w| w.0.clone())
    }

    pub fn plugins_center(&self) -> Option<Vec<String>> {
        self.plugins_center.clone()
    }

    pub fn plugins_right(&self) -> Option<Vec<String>> {
        self.plugins_wings.as_ref().map(|w| w.1.clone())
    }

    pub fn anchor(&self) -> PanelAnchor {
        self.anchor
    }

    pub fn padding(&self) -> u32 {
        self.padding
    }

    pub fn layer(&self) -> zwlr_layer_shell_v1::Layer {
        self.layer.into()
    }

    pub fn keyboard_interactivity(&self) -> zwlr_layer_surface_v1::KeyboardInteractivity {
        self.keyboard_interactivity.into()
    }

    pub fn is_horizontal(&self) -> bool {
        match self.anchor {
            PanelAnchor::Top | PanelAnchor::Bottom => true,
            _ => false,
        }
    }

    pub fn bg_color_override(&self) -> Option<[f32; 4]> {
        match self.background {
            CosmicPanelBackground::Color(c) => Some([c[0], c[1], c[2], self.opacity]),
            _ => None,
        }
    }

    /// get constraints for the thickness of the panel bar
    pub fn get_dimensions(
        &self,
        output_dims: Option<(u32, u32)>,
        suggested_length: Option<u32>,
        gap: Option<u32>,
    ) -> (Option<Range<u32>>, Option<Range<u32>>) {
        let gap = gap.unwrap_or_else(|| self.get_effective_anchor_gap());
        let bar_thickness = match &self.size {
            PanelSize::XS => 8 + gap..61 + gap,
            PanelSize::S => 8 + gap..81 + gap,
            PanelSize::M => 8 + gap..101 + gap,
            PanelSize::L => 8 + gap..121 + gap,
            PanelSize::XL => 8 + gap..141 + gap,
        };
        assert!(2 * self.padding + gap < bar_thickness.end);
        let o_h = suggested_length.unwrap_or_else(|| output_dims.unwrap_or_default().1);
        let o_w = suggested_length.unwrap_or_else(|| output_dims.unwrap_or_default().0);

        match self.anchor {
            PanelAnchor::Left | PanelAnchor::Right => (Some(bar_thickness), Some(o_h..o_h + 1)),
            PanelAnchor::Top | PanelAnchor::Bottom => (Some(o_w..o_w + 1), Some(bar_thickness)),
        }
    }

    pub fn cosmic_config(name: &str) -> Result<Config, cosmic_config::Error> {
        let entry_name = format!("{NAME}.{}", name);
        Config::new(&entry_name, VERSION)
    }

    pub fn maximize(&mut self) {
        self.opacity = 1.0;
        if self.autohide().is_some() {
            return;
        }
        self.expand_to_edges = true;
        self.margin = 0;
        self.border_radius = 0;
        self.anchor_gap = false;
    }
}

#[cfg(feature = "wayland-rs")]
impl WrapperConfig for CosmicPanelConfig {
    fn outputs(&self) -> WrapperOutput {
        self.output.clone().into()
    }

    fn name(&self) -> &str {
        &self.name
    }
}
