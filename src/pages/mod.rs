pub mod browse;
pub mod preview;
pub mod keybindings;

use iced::widget::{button, text};
use iced::{Color, Element, Length};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Browse,
    Keybindings,
}

impl Page {
    pub const ALL: [Page; 2] = [
        Page::Browse,
        Page::Keybindings,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Page::Browse => "Browse",
            Page::Keybindings => "Keys",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Page::Browse => "📁",
            Page::Keybindings => "⌨",
        }
    }
}

pub fn sidebar_button(page: Page, active: bool) -> Element<'static, super::Message> {
    let bg = if active {
        Color::from_rgb8(0x2a, 0x4a, 0x2e)
    } else {
        Color::from_rgb8(0x1e, 0x2e, 0x20)
    };

    let fg = if active {
        Color::from_rgb8(0x8f, 0xd4, 0x8f)
    } else {
        Color::from_rgb8(0x99, 0x99, 0xaa)
    };

    button(
        iced::widget::row![]
            .spacing(8)
            .push(text(page.icon()).size(16))
            .push(text(page.label()).size(14).color(fg)),
    )
    .style(move |_theme, _status| button::Style {
        background: Some(bg.into()),
        border: iced::Border {
            radius: 6.0.into(),
            ..iced::Border::default()
        },
        ..button::Style::default()
    })
    .padding([10, 14])
    .width(Length::Fill)
    .on_press(super::Message::SwitchPage(page))
    .into()
}
