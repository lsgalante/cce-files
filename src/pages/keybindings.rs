use iced::widget::{column, row, rule, text};
use iced::{Color, Element, Length, Task};

use crate::Message;

// ── Data ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct KeybindingsState;

#[derive(Debug, Clone)]
pub enum KeybindingsMessage {}

// ── Static data ────────────────────────────────────────────────────

struct Binding {
    keys: &'static str,
    action: &'static str,
}

struct Section {
    title: &'static str,
    icon: &'static str,
    bindings: &'static [Binding],
}

const SECTIONS: &[Section] = &[
    Section {
        title: "Navigation",
        icon: "🧭",
        bindings: &[
            Binding { keys: "j / Down", action: "Move down" },
            Binding { keys: "k / Up", action: "Move up" },
            Binding { keys: "h / Backspace", action: "Go to parent directory" },
            Binding { keys: "l / Enter", action: "Open directory" },
            Binding { keys: ".", action: "Toggle hidden files" },
            Binding { keys: "Ctrl + F", action: "Focus search" },
        ],
    },
    Section {
        title: "Browse",
        icon: "📁",
        bindings: &[
            Binding { keys: "Click", action: "Select file or directory" },
            Binding { keys: "Double-click", action: "Open directory" },
            Binding { keys: "Search", action: "Filter files by name" },
        ],
    },
    Section {
        title: "Info",
        icon: "ℹ",
        bindings: &[
            Binding { keys: "Select", action: "View file details in info panel" },
            Binding { keys: "Open dir button", action: "Navigate into selected directory" },
        ],
    },
    Section {
        title: "General",
        icon: "⚡",
        bindings: &[
            Binding { keys: "Ctrl + 1-2", action: "Switch page" },
        ],
    },
];

// ── View ────────────────────────────────────────────────────────────

pub fn view(_state: &KeybindingsState) -> Element<'_, Message> {
    let accent = Color::from_rgb8(0x5c, 0x90, 0x60);
    let text_fg = Color::from_rgb8(0xd4, 0xd4, 0xd4);
    let text_dim = Color::from_rgb8(0x88, 0x88, 0x99);

    let mut content = column![text("Keybindings").size(18).color(accent)].spacing(12);

    for section in SECTIONS {
        let header = text(format!("{} {}", section.icon, section.title))
            .size(14)
            .color(text_fg);

        let mut rows = column![].spacing(4);
        for b in section.bindings {
            rows = rows.push(
                row![]
                    .spacing(8)
                    .push(text(b.keys).size(12).color(accent).width(140))
                    .push(text(b.action).size(12).color(text_dim)),
            );
        }

        content = content.push(header);
        content = content.push(rows);
        content = content.push(rule::horizontal(1).style(|_theme| rule::Style {
            color: Color::from_rgb8(0x26, 0x33, 0x28),
            radius: 0.0.into(),
            fill_mode: rule::FillMode::Full,
            snap: true,
        }));
    }

    content.width(Length::Fill).into()
}

// ── Update ──────────────────────────────────────────────────────────

pub fn update(_state: &mut KeybindingsState, _msg: KeybindingsMessage) -> Task<Message> {
    Task::none()
}

// ── Subscription ────────────────────────────────────────────────────

pub fn subscription(_state: &KeybindingsState) -> iced::Subscription<Message> {
    iced::Subscription::none()
}
