// popup for rendering overflow items in their own space

use calloop::LoopHandle;
use cosmic::{
    iced::{id, Color, Length},
    iced_core::Shadow,
    iced_style::container,
    theme,
    widget::horizontal_space,
    Theme,
};
use cosmic_theme::palette::white_point::B;

use crate::{
    iced::{Element, IcedElement, Program},
    xdg_shell_wrapper::shared_state::GlobalState,
};

pub const BORDER_WIDTH: u32 = 1;

pub type OverflowPopupElement = IcedElement<OverflowPopup>;

pub fn overflow_popup_element(
    id: id::Id,
    logical_width: f32,
    logical_height: f32,
    loop_handle: LoopHandle<'static, GlobalState>,
    theme: Theme,
    panel_id: usize,
    scale: f32,
) -> OverflowPopupElement {
    let logical_width = logical_width * scale;
    let logical_height = logical_height * scale;
    IcedElement::new(
        OverflowPopup { id, logical_width, logical_height },
        (logical_width.round() as i32, logical_height.round() as i32),
        loop_handle,
        theme,
        panel_id,
        false,
    )
}

pub struct OverflowPopup {
    pub id: id::Id,
    pub logical_width: f32,
    pub logical_height: f32,
}

impl Program for OverflowPopup {
    type Message = ();

    fn view(&self) -> Element<'_, ()> {
        Element::from(
            cosmic::widget::container(horizontal_space(Length::Fixed(self.logical_width)))
                .width(Length::Fixed(self.logical_width))
                .height(Length::Fixed(self.logical_height))
                .style(theme::Container::custom(|theme| {
                    let cosmic = theme.cosmic();
                    let corners = cosmic.corner_radii.clone();
                    container::Appearance {
                        text_color: Some(cosmic.background.on.into()),
                        background: Some(Color::from(cosmic.background.base).into()),
                        border: cosmic::iced::Border {
                            radius: corners.radius_m.into(),
                            width: BORDER_WIDTH as f32,
                            color: cosmic.background.divider.into(),
                        },
                        shadow: Shadow::default(),
                        icon_color: Some(cosmic.background.on.into()),
                    }
                })),
        )
    }
}
