// popup for rendering overflow items in their own space

use calloop::LoopHandle;
use cosmic::{
    iced::{id, Color, Length},
    iced_core::Shadow,
    theme,
    widget::{container, horizontal_space},
    Theme,
};

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
    count: usize,
) -> OverflowPopupElement {
    IcedElement::new(
        OverflowPopup { id, logical_width, logical_height, count },
        ((logical_width).round() as i32, (logical_height).round() as i32),
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
    pub count: usize,
}

impl Program for OverflowPopup {
    type Message = ();

    fn view(&self) -> Element<'_, ()> {
        let width = self.logical_width;
        let height = self.logical_height;
        let border_width = BORDER_WIDTH as f32;
        Element::from(
            cosmic::widget::container(horizontal_space().width(Length::Fixed(width)))
                .width(Length::Fixed(width))
                .height(Length::Fixed(height))
                .class(theme::Container::custom(move |theme| {
                    let cosmic = theme.cosmic();
                    let radius_m = cosmic.corner_radii.radius_m;

                    container::Style {
                        text_color: Some(cosmic.background.on.into()),
                        background: Some(Color::from(cosmic.background.base).into()),
                        border: cosmic::iced::Border {
                            radius: radius_m.into(),
                            width: border_width,
                            color: cosmic.background.divider.into(),
                        },
                        shadow: Shadow::default(),
                        icon_color: Some(cosmic.background.on.into()),
                    }
                })),
        )
    }
}
