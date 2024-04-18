// popup for rendering overflow items in their own space

use cosmic::{
    iced::{id, Length},
    widget::horizontal_space,
};

use crate::iced::{Element, Program};

pub struct OverflowPopup {
    id: id::Id,
    logical_width: f32,
    logical_height: f32,
}

impl Program for OverflowPopup {
    type Message = ();

    fn view(&self) -> Element<'_, ()> {
        Element::from(
            cosmic::widget::container(horizontal_space(Length::Fixed(self.logical_width)))
                .width(self.logical_width)
                .height(self.logical_height)
                .style(cosmic::theme::Container::WindowBackground),
        )
    }
}
