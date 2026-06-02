use crate::pages::PageContent;

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

pub fn view(_state: &KeybindingsState, cx: f32, cy: f32, cw: f32, _ch: f32) -> PageContent {
    let mut pc = PageContent::new();
    let accent = [0.36, 0.56, 0.38, 1.0];
    let text_fg = [0.83, 0.83, 0.83, 1.0];
    let text_dim = [0.53, 0.53, 0.60, 1.0];

    pc.text("Keybindings", cx + 12.0, cy + 12.0, 18.0, accent);

    let mut y = cy + 42.0;

    for section in SECTIONS {
        // Section Header
        let header_str = format!("{} {}", section.icon, section.title);
        pc.text(&header_str, cx + 12.0, y, 14.0, text_fg);
        y += 22.0;

        for b in section.bindings {
            pc.text(b.keys, cx + 12.0, y, 12.0, accent);
            pc.text(b.action, cx + 160.0, y, 12.0, text_dim);
            y += 18.0;
        }

        y += 8.0;
        // Divider
        pc.rect([0.15, 0.20, 0.16, 1.0], cx + 12.0, y, cw - 24.0, 1.0);
        y += 12.0;
    }

    pc
}

// ── Update ──────────────────────────────────────────────────────────

pub fn update(_state: &mut KeybindingsState, _msg: KeybindingsMessage) {}
