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
    count: usize,
    scale: f32,
) -> OverflowPopupElement {
    IcedElement::new(
        OverflowPopup { id, logical_width, logical_height, scale, count },
        ((logical_width * scale).round() as i32, (logical_height * scale).round() as i32),
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
    pub scale: f32,
    pub count: usize,
}

impl Program for OverflowPopup {
    type Message = ();

    fn view(&self) -> Element<'_, ()> {
        let width = self.logical_width * self.scale;
        let height = self.logical_height * self.scale;
        let border_width = BORDER_WIDTH as f32 * self.scale;
        let scale = self.scale;
        Element::from(
            cosmic::widget::container(horizontal_space(Length::Fixed(width)))
                .width(Length::Fixed(width))
                .height(Length::Fixed(height))
                .style(theme::Container::custom(move |theme| {
                    let cosmic = theme.cosmic();
                    let mut radius_m = cosmic.corner_radii.radius_m.clone();
                    for r in radius_m.iter_mut() {
                        *r *= scale;
                    }
                    container::Appearance {
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
