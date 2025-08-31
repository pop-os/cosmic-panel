use std::{
    borrow::Cow,
    hash::Hash,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use calloop::LoopHandle;
// element for rendering a button that toggles the overflow popup when clicked
use crate::xdg_shell_wrapper::{self, shared_state::GlobalState};
use cosmic::{
    Element,
    iced::{Length, Padding},
    iced_core::id,
    theme::{self, Button},
    widget::{Id, button, layer_container},
};
use smithay::utils::{Logical, Point, Size};

use crate::iced::{IcedElement, Program};

pub type OverflowButtonElement = IcedElement<OverflowButton>;

pub fn overflow_button_element(
    id: id::Id,
    pos: Point<i32, Logical>,
    icon_size: u16,
    button_padding: Padding,
    selected: Arc<AtomicBool>,
    icon: Cow<'static, str>,
    handle: LoopHandle<'static, GlobalState>,
    theme: cosmic::Theme,
    panel_id: usize,
) -> OverflowButtonElement {
    let icon_size = icon_size as f32;
    let Padding { top, right, bottom, left } = button_padding;
    let button_padding = Padding { top, right, bottom, left };
    let size = (
        (icon_size + button_padding.horizontal()).round() as i32,
        (icon_size + button_padding.vertical()).round() as i32,
    );
    IcedElement::new(
        OverflowButton::new(
            id,
            pos,
            icon_size.round() as u16,
            button_padding,
            selected,
            icon,
            panel_id,
        ),
        Size::from(size),
        handle,
        theme,
        panel_id,
        true,
    )
}

pub fn with_id<T>(b: &OverflowButtonElement, f: impl Fn(&Id) -> T) -> T {
    b.with_program(|p| f(&p.id))
}

#[derive(Debug, Clone, Copy)]
pub enum Message {
    TogglePopup,
    HidePopup,
}

#[derive(Debug, Clone)]
pub struct OverflowButton {
    pub id: id::Id,
    pos: Point<i32, Logical>,
    icon_size: u16,
    button_padding: Padding,
    /// Selected if the popup is open
    selected: Arc<AtomicBool>,
    icon: Cow<'static, str>,
    panel_id: usize,
}

impl OverflowButton {
    pub fn new(
        id: id::Id,
        pos: Point<i32, Logical>,
        icon_size: u16,
        button_padding: Padding,
        selected: Arc<AtomicBool>,
        icon: Cow<'static, str>,
        panel_id: usize,
    ) -> Self {
        Self { id, pos, icon_size, button_padding, selected, icon, panel_id }
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
        loop_handle: &calloop::LoopHandle<'static, xdg_shell_wrapper::shared_state::GlobalState>,
    ) -> cosmic::Task<Self::Message> {
        match message {
            Message::TogglePopup => {
                let id = self.id.clone();
                let panel_id = self.panel_id;

                _ = loop_handle.insert_idle(move |state| {
                    let Some(seat) = state.server_state.seats.first() else {
                        return;
                    };
                    let c_seat = (seat.client.last_pointer_press.0, seat.client._seat.clone());
                    state.space.toggle_overflow_popup(
                        panel_id,
                        id.clone(),
                        &state.client_state.compositor_state,
                        state.client_state.fractional_scaling_manager.as_ref(),
                        state.client_state.viewporter_state.as_ref(),
                        &state.client_state.queue_handle,
                        &mut state.client_state.xdg_shell_state,
                        c_seat,
                        false,
                    );
                });
            },
            Message::HidePopup => {
                let id = self.id.clone();
                let panel_id = self.panel_id;

                _ = loop_handle.insert_idle(move |state| {
                    let Some(seat) = state.server_state.seats.first() else {
                        return;
                    };
                    let c_seat = (seat.client.last_pointer_press.0, seat.client._seat.clone());
                    state.space.toggle_overflow_popup(
                        panel_id,
                        id.clone(),
                        &state.client_state.compositor_state,
                        state.client_state.fractional_scaling_manager.as_ref(),
                        state.client_state.viewporter_state.as_ref(),
                        &state.client_state.queue_handle,
                        &mut state.client_state.xdg_shell_state,
                        c_seat,
                        true,
                    );
                });
            },
        }
        cosmic::Task::none()
    }

    fn view(&self) -> crate::iced::Element<'_, Self::Message> {
        Element::from(
            button::custom(
                layer_container(
                    cosmic::widget::icon(cosmic::widget::icon::from_name(self.icon.clone()).into())
                        .class(theme::Svg::Custom(Rc::new(|theme| {
                            cosmic::iced_widget::svg::Style {
                                color: Some(theme.cosmic().background.on.into()),
                            }
                        })))
                        .width(Length::Fixed(self.icon_size as f32))
                        .height(Length::Fixed(self.icon_size as f32)),
                )
                .align_x(cosmic::iced::Alignment::Center)
                .align_y(cosmic::iced::Alignment::Center)
                .width(Length::Fixed(self.icon_size as f32 + self.button_padding.horizontal()))
                .height(Length::Fixed(self.icon_size as f32 + self.button_padding.horizontal())),
            )
            .selected(self.selected.load(Ordering::Relaxed))
            .class(Button::AppletIcon)
            .on_press(Message::TogglePopup),
        )
    }
}
