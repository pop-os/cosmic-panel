// Element for rendering a panel background

use calloop::LoopHandle;
use cosmic::iced::alignment::{Horizontal, Vertical};
use cosmic::iced::core::Shadow;
use cosmic::iced::widget::rule;
use cosmic::iced::{Color, Length, id};
use cosmic::widget::{container, space};
use cosmic::{Theme, theme};
use cosmic_panel_config::PanelAnchor;

use crate::iced::{Element, IcedElement, Program};
use crate::xdg_shell_wrapper::shared_state::GlobalState;

pub type BackgroundElement = IcedElement<Background>;

/// Border drawn around the panel
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelBorder {
    None,
    /// 4 sides
    Full,
    /// single line on the side facing away from the anchored edge
    Side(PanelAnchor),
}

pub fn background_element(
    id: id::Id,
    logical_width: i32,
    logical_height: i32,
    radius: [f32; 4],
    loop_handle: LoopHandle<'static, GlobalState>,
    theme: Theme,
    panel_id: usize,
    logical_pos: [f32; 2],
    color: [f32; 4],
    scale: f64,
    border: PanelBorder,
    border_width: f32,
) -> BackgroundElement {
    IcedElement::new(
        Background {
            id,
            logical_width,
            logical_height,
            radius,
            logical_pos: (logical_pos[0].round() as i32, logical_pos[1].round() as i32),
            color,
            scale,
            border,
            border_width,
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
    pub radius: [f32; 4],
    pub logical_pos: (i32, i32),
    pub color: [f32; 4],
    pub scale: f64,
    pub border: PanelBorder,
    pub border_width: f32,
}

impl Program for Background {
    type Message = ();

    fn view(&self) -> Element<'_, ()> {
        let width = self.logical_width as f32;
        let height = self.logical_height as f32;
        let radius_arr: [f32; 4] = self.radius;
        let [top_left, top_right, bottom_right, bottom_left] = radius_arr;

        let color = self.color;
        // no border when panel is fully transparent
        let border = if color[3] < 0.01 { PanelBorder::None } else { self.border };
        let border_width = if matches!(border, PanelBorder::Full) { self.border_width } else { 0. };

        // for a single edge, avoide the corner radius (panel won't have curved corner in this mode
        // anyways)
        let (line_horizontal, inset) = match border {
            PanelBorder::Side(PanelAnchor::Bottom) => (true, (top_left, top_right)),
            PanelBorder::Side(PanelAnchor::Top) => (true, (bottom_left, bottom_right)),
            PanelBorder::Side(PanelAnchor::Left) => (false, (top_right, bottom_right)),
            PanelBorder::Side(PanelAnchor::Right) => (false, (top_left, bottom_left)),
            _ => (true, (0., 0.)),
        };
        let line = self.border_width;
        let content = if matches!(border, PanelBorder::Side(_)) {
            let fill_mode = rule::FillMode::AsymmetricPadding(inset.0 as u16, inset.1 as u16);
            Element::from(
                if line_horizontal { rule::horizontal(line) } else { rule::vertical(line) }.class(
                    theme::Rule::custom(move |theme| rule::Style {
                        color: theme.cosmic().bg_divider().into(),
                        radius: 0.into(),
                        fill_mode,
                        snap: true,
                    }),
                ),
            )
        } else {
            Element::from(space::horizontal().width(Length::Fixed(width)))
        };

        let mut bg = container(content)
            .width(Length::Fixed(width))
            .height(Length::Fixed(height))
            .class(theme::Container::custom(move |theme| {
                let cosmic = theme.cosmic();

                container::Style {
                    text_color: Some(cosmic.background(theme.transparent).on.into()),
                    background: Some(Color::from(color).into()),
                    border: cosmic::iced::Border {
                        radius: radius_arr.into(),
                        width: border_width,
                        color: cosmic.bg_divider().into(),
                    },
                    shadow: Shadow::default(),
                    snap: true,
                    icon_color: Some(cosmic.background(theme.transparent).on.into()),
                }
            }));
        bg = match border {
            PanelBorder::Side(PanelAnchor::Bottom) => bg.align_y(Vertical::Top),
            PanelBorder::Side(PanelAnchor::Top) => bg.align_y(Vertical::Bottom),
            PanelBorder::Side(PanelAnchor::Left) => bg.align_x(Horizontal::Right),
            PanelBorder::Side(PanelAnchor::Right) => bg.align_x(Horizontal::Left),
            _ => bg,
        };
        Element::from(bg)
    }
}
