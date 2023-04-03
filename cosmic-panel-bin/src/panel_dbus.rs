use zbus::dbus_interface;

#[derive(Debug)]
pub(crate) struct PanelDbus {
    pub(crate) notification_ids: Vec<String>,
}

#[dbus_interface(name = "com.system76.CosmicPanel")]
impl PanelDbus {
    /// The notification ids for each applet
    #[dbus_interface(property)]
    async fn notification_ids(&self) -> Vec<String> {
        self.notification_ids.clone()
    }
}
