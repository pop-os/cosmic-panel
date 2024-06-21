use cosmic::widget::Id;

#[derive(Debug, Clone)]
pub enum PanelIcedMessage {
    /// Toggle the popup
    TogglePopup(Id),
    /// Request a redraw when contents in an element changes
    Redraw,
}
