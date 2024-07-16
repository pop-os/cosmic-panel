// Element for rendering a panel background

use calloop::LoopHandle;
use cosmic::{
    iced::{id, Color, Length},
    iced_core::Shadow,
    iced_style::container,
    theme,
    widget::horizontal_space,
    Theme,
};

use crate::{
    iced::{Element, IcedElement, Program},
    xdg_shell_wrapper::shared_state::GlobalState,
};

pub type BackgroundElement = IcedElement<Background>;

pub fn background_element(
    id: id::Id,
    logical_width: i32,
    logical_height: i32,
    radius: [u32; 4],
    loop_handle: LoopHandle<'static, GlobalState>,
    theme: Theme,
    panel_id: usize,
    logical_pos: [f32; 2],
    color: [f32; 4],
) -> BackgroundElement {
    IcedElement::new(
        Background {
            id,
            logical_width,
            logical_height,
            radius,
            logical_pos: (logical_pos[0].round() as i32, logical_pos[1].round() as i32),
            color,
        },
        (logical_width, logical_height),
        loop_handle,
        theme,
        panel_id,
        false,
    )
}

pub struct Background {
    pub id: id::Id,
    pub logical_width: i32,
    pub logical_height: i32,
    pub radius: [u32; 4],
    pub logical_pos: (i32, i32),
    pub color: [f32; 4],
}

impl Program for Background {
    type Message = ();

    fn view(&self) -> Element<'_, ()> {
        let width = self.logical_width as f32;
        let height = self.logical_height as f32;
        let mut radius_arr: [f32; 4] = [0.; 4];
        for (r, arr_r) in self.radius.clone().into_iter().zip(radius_arr.iter_mut()) {
            *arr_r = r as f32;
        }
        let color = self.color;
        Element::from(
            cosmic::widget::container(horizontal_space(Length::Fixed(width)))
                .width(Length::Fixed(width))
                .height(Length::Fixed(height))
                .style(theme::Container::custom(move |theme| {
                    let cosmic = theme.cosmic();

                    container::Appearance {
                        text_color: Some(cosmic.background.on.into()),
                        background: Some(Color::from(color).into()),
                        border: cosmic::iced::Border {
                            radius: radius_arr.into(),
                            width: 0.,
                            color: cosmic.background.divider.into(),
                        },
                        shadow: Shadow::default(),
                        icon_color: Some(cosmic.background.on.into()),
                    }
                })),
        )
    }
}
