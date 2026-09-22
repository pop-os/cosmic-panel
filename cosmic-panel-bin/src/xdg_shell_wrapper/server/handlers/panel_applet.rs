// SPDX-License-Identifier: MPL-2.0

use cosmic_panel_config::{CosmicPanelBackground, CosmicPanelConfig, PanelAnchor, PanelSize, Side};
use cosmic_protocols::panel_applet::v1::server::cosmic_panel_applet_manager_v1::{
    self as manager, CosmicPanelAppletManagerV1,
};
use cosmic_protocols::panel_applet::v1::server::cosmic_panel_applet_v1::{
    self as applet, CosmicPanelAppletV1,
};
use smithay::reexports::wayland_server::backend::ObjectId;
use smithay::reexports::wayland_server::{
    Client, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};

use crate::xdg_shell_wrapper::shared_state::GlobalState;

/// The settings of a panel, as far as the applets in it are concerned.
///
/// This mirrors the values that are passed to applets through the environment
/// when they are spawned, but can be updated at runtime.
#[derive(Debug, Clone, PartialEq)]
pub struct PanelSettings {
    /// Name of the panel profile.
    pub name: String,
    /// Name of the output the panel is displayed on.
    pub output: String,
    /// Edge of the output the panel is anchored to.
    pub anchor: PanelAnchor,
    /// Effective size for the applet on the section of the panel it is in.
    pub applet_size: PanelSize,
    /// Spacing between the applets of the panel in logical pixels.
    pub spacing: u32,
    /// Background of the panel.
    pub background: CosmicPanelBackground,
    /// Ratio of the applet padding that overlaps.
    pub padding_overlap: f32,
}

impl PanelSettings {
    pub fn new(config: &CosmicPanelConfig, output: String, side: Side) -> Self {
        Self {
            name: config.name.clone(),
            output,
            anchor: config.anchor,
            applet_size: config.get_effective_applet_size(side),
            spacing: config.spacing,
            background: config.background.clone(),
            padding_overlap: config.padding_overlap(),
        }
    }

    fn send_all(&self, object: &CosmicPanelAppletV1) {
        let (size, custom) = applet_size(&self.applet_size);
        let (background, red, green, blue) = background(&self.background);
        object.panel_name(self.name.clone());
        object.output(self.output.clone());
        object.anchor(anchor(self.anchor));
        object.applet_size(size, custom);
        object.spacing(self.spacing);
        object.background(background, red, green, blue);
        object.padding_overlap(f64::from(self.padding_overlap));
    }

    fn send_changes(&self, object: &CosmicPanelAppletV1, previous: &Self) {
        if self.name != previous.name {
            object.panel_name(self.name.clone());
        }
        if self.output != previous.output {
            object.output(self.output.clone());
        }
        if self.anchor != previous.anchor {
            object.anchor(anchor(self.anchor));
        }
        if self.applet_size != previous.applet_size {
            let (size, custom) = applet_size(&self.applet_size);
            object.applet_size(size, custom);
        }
        if self.spacing != previous.spacing {
            object.spacing(self.spacing);
        }
        if self.background != previous.background {
            let (background, red, green, blue) = background(&self.background);
            object.background(background, red, green, blue);
        }
        if self.padding_overlap != previous.padding_overlap {
            object.padding_overlap(f64::from(self.padding_overlap));
        }
    }
}

#[derive(Debug)]
pub struct AppletSettings {
    object: CosmicPanelAppletV1,
    last_sent: PanelSettings,
}

impl AppletSettings {
    /// Create the settings object of an applet and send it the current
    /// settings.
    pub fn new(object: CosmicPanelAppletV1, settings: PanelSettings) -> Self {
        settings.send_all(&object);
        object.done();
        Self { object, last_sent: settings }
    }

    pub fn update(&mut self, settings: PanelSettings) {
        if self.last_sent == settings {
            return;
        }
        settings.send_changes(&self.object, &self.last_sent);
        self.object.done();
        self.last_sent = settings;
    }

    pub fn object_id(&self) -> ObjectId {
        self.object.id()
    }

    pub fn is_alive(&self) -> bool {
        self.object.is_alive()
    }
}

fn anchor(anchor: PanelAnchor) -> applet::Anchor {
    match anchor {
        PanelAnchor::Left => applet::Anchor::Left,
        PanelAnchor::Right => applet::Anchor::Right,
        PanelAnchor::Top => applet::Anchor::Top,
        PanelAnchor::Bottom => applet::Anchor::Bottom,
    }
}

fn applet_size(size: &PanelSize) -> (applet::AppletSize, u32) {
    match size {
        PanelSize::XS => (applet::AppletSize::ExtraSmall, 0),
        PanelSize::S => (applet::AppletSize::Small, 0),
        PanelSize::M => (applet::AppletSize::Medium, 0),
        PanelSize::L => (applet::AppletSize::Large, 0),
        PanelSize::XL => (applet::AppletSize::ExtraLarge, 0),
        PanelSize::Custom(size) => (applet::AppletSize::Custom, *size),
    }
}

fn background(background: &CosmicPanelBackground) -> (applet::Background, f64, f64, f64) {
    match background {
        CosmicPanelBackground::ThemeDefault => (applet::Background::ThemeDefault, 0.0, 0.0, 0.0),
        CosmicPanelBackground::Dark => (applet::Background::Dark, 0.0, 0.0, 0.0),
        CosmicPanelBackground::Light => (applet::Background::Light, 0.0, 0.0, 0.0),
        CosmicPanelBackground::Color([red, green, blue]) => {
            (applet::Background::Color, f64::from(*red), f64::from(*green), f64::from(*blue))
        },
    }
}

impl GlobalDispatch<CosmicPanelAppletManagerV1, ()> for GlobalState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<CosmicPanelAppletManagerV1>,
        _global_data: &(),
        data_init: &mut smithay::reexports::wayland_server::DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<CosmicPanelAppletManagerV1, ()> for GlobalState {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &CosmicPanelAppletManagerV1,
        request: manager::Request,
        _data: &(),
        _handle: &DisplayHandle,
        data_init: &mut smithay::reexports::wayland_server::DataInit<'_, Self>,
    ) {
        let manager::Request::GetPanelApplet { id } = request else {
            return;
        };

        let client_id = client.id();
        let Some(space) = state.space.space_for_client(Some(&client_id)) else {
            data_init.init(id, ());
            tracing::warn!(
                "Ignoring panel applet settings request of a client that is not an applet"
            );
            return;
        };

        if space.has_applet_settings(&client_id) {
            resource.post_error(
                manager::Error::PanelAppletExists as u32,
                format!("{resource:?} CosmicPanelAppletV1 object already exists for the client"),
            );
            return;
        }

        let object = data_init.init(id, ());
        let Some(settings) = space.panel_settings_for(&client_id) else {
            return;
        };

        space.set_applet_settings(&client_id, AppletSettings::new(object, settings));
    }
}

impl Dispatch<CosmicPanelAppletV1, ()> for GlobalState {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &CosmicPanelAppletV1,
        request: applet::Request,
        _data: &(),
        _handle: &DisplayHandle,
        _data_init: &mut smithay::reexports::wayland_server::DataInit<'_, Self>,
    ) {
        if let applet::Request::Destroy = request {
            let client_id = client.id();
            if let Some(space) = state.space.space_for_client(Some(&client_id)) {
                space.remove_applet_settings(&client_id, resource.id());
            }
        }
    }
}
