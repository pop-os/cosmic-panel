use std::{
    borrow::Cow,
    hash::Hash,
    rc::Rc,
    sync::{
        atomic::{self, AtomicBool},
        Arc,
    },
};

use calloop::LoopHandle;
// element for rendering a button that toggles the overflow popup when clicked
use cosmic::{
    iced::{
        alignment::{Horizontal, Vertical},
        Length, Padding,
    },
    iced_core::id,
    theme::{self, Button},
    widget::{layer_container, Id},
    Element,
};
use smithay::{
    desktop::space::SpaceElement,
    utils::{IsAlive, Logical, Point, Rectangle, Size},
};
use xdg_shell_wrapper::shared_state::GlobalState;

use crate::iced::{IcedElement, Program};

pub type OverflowButtonElement = IcedElement<OverflowButton>;

pub fn overflow_button_element(
    id: id::Id,
    pos: Point<i32, Logical>,
    icon_size: u16,
    button_padding: Padding,
    selected: Arc<AtomicBool>,
    icon: Cow<'static, str>,
    handle: LoopHandle<'static, GlobalState<crate::space_container::SpaceContainer>>,
    theme: cosmic::Theme,
) -> OverflowButtonElement {
    let size = (
        (icon_size as f32 + button_padding.horizontal()).round() as i32,
        (icon_size as f32 + button_padding.vertical()).round() as i32,
    );
    IcedElement::new(
        OverflowButton::new(id, pos, icon_size, button_padding, selected, icon),
        Size::from(size),
        handle,
        theme,
    )
}

pub fn with_id<T>(b: &OverflowButtonElement, f: impl Fn(&Id) -> T) -> T {
    b.with_program(|p| f(&p.id))
}

#[derive(Debug, Clone, Copy)]
pub enum Message {
    TogglePopup,
}

#[derive(Debug, Clone)]
pub struct OverflowButton {
    id: id::Id,
    pos: Point<i32, Logical>,
    icon_size: u16,
    button_padding: Padding,
    /// Selected if the popup is open
    selected: Arc<AtomicBool>,
    icon: Cow<'static, str>,
}

impl OverflowButton {
    pub fn new(
        id: id::Id,
        pos: Point<i32, Logical>,
        icon_size: u16,
        button_padding: Padding,
        selected: Arc<AtomicBool>,
        icon: Cow<'static, str>,
    ) -> Self {
        Self { id, pos, icon_size, button_padding, selected, icon }
    }
}

impl PartialEq for OverflowButton {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for OverflowButton {}

impl Hash for OverflowButton {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl Program for OverflowButton {
    type Message = Message;

    fn update(
        &mut self,
        message: Self::Message,
        loop_handle: &calloop::LoopHandle<
            'static,
            xdg_shell_wrapper::shared_state::GlobalState<crate::space_container::SpaceContainer>,
        >,
    ) -> cosmic::Command<Self::Message> {
        match message {
            Message::TogglePopup => {
                let id = self.id.clone();
                loop_handle.insert_idle(move |state| {
                    state.space.toggle_overflow_popup(id);
                });
            },
        }
        cosmic::Command::none()
    }

    fn view(&self) -> crate::iced::Element<'_, Self::Message> {
        Element::from(
            cosmic::widget::button(
                layer_container(
                    cosmic::widget::icon(cosmic::widget::icon::from_name(self.icon.clone()).into())
                        .style(theme::Svg::Custom(Rc::new(|theme| {
                            cosmic::iced_style::svg::Appearance {
                                color: Some(theme.cosmic().background.on.into()),
                            }
                        })))
                        .width(Length::Fixed(self.icon_size as f32))
                        .height(Length::Fixed(self.icon_size as f32)),
                )
                .align_x(Horizontal::Center)
                .align_y(Vertical::Center)
                .width(Length::Fixed(self.icon_size as f32 + self.button_padding.horizontal()))
                .height(Length::Fixed(self.icon_size as f32 + self.button_padding.horizontal())),
            )
            .style(Button::AppletIcon)
            .on_press(Message::TogglePopup),
        )
    }
}
