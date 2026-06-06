use crate::pages::PageContent;
use clear_ui::widget::{ColorSelector, Element};
use clear_ui::layout::SectionContext;

// ── Data ────────────────────────────────────────────────────────────

pub struct SettingsState {
    pub color_selector: ColorSelector,
}

impl Default for SettingsState {
    fn default() -> Self {
        let col = clear_ui::color::node_color();
        let col_u8 = [
            (col[0] * 255.0) as u8,
            (col[1] * 255.0) as u8,
            (col[2] * 255.0) as u8,
        ];
        Self {
            color_selector: ColorSelector::new(col_u8).with_label("Node Color"),
        }
    }
}

// ── Keybindings Structs & Data ──────────────────────────────────────

struct Binding {
    keys: &'static str,
    action: &'static str,
}

struct SectionData {
    title: &'static str,
    icon: &'static str,
    bindings: &'static [Binding],
}

const SECTIONS: &[SectionData] = &[
    SectionData {
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
    SectionData {
        title: "Browse",
        icon: "📁",
        bindings: &[
            Binding { keys: "Click", action: "Select file or directory" },
            Binding { keys: "Double-click", action: "Open directory" },
            Binding { keys: "Search", action: "Filter files by name" },
        ],
    },
    SectionData {
        title: "Info",
        icon: "ℹ",
        bindings: &[
            Binding { keys: "Select", action: "View file details in info panel" },
            Binding { keys: "Open dir button", action: "Navigate into selected directory" },
        ],
    },
    SectionData {
        title: "General",
        icon: "⚡",
        bindings: &[
            Binding { keys: "Ctrl + 1-3", action: "Switch page" },
        ],
    },
];

// ── View ────────────────────────────────────────────────────────────

pub fn view(state: &mut SettingsState, cx: f32, cy: f32, cw: f32, _ch: f32) -> PageContent {
    let mut pc = PageContent::new();
    let accent = [0.36, 0.56, 0.38, 1.0];
    let text_fg = [0.83, 0.83, 0.83, 1.0];
    let text_dim = [0.53, 0.53, 0.60, 1.0];

    pc.text("Settings", cx + 12.0, cy + 12.0, 18.0, accent);

    let cs_w = (cw - 24.0).min(300.0);
    let cs_h = 42.0;
    let cs_y = cy + 48.0;

    state.color_selector.set_row_rect(cx + 12.0, cs_w);
    clear_ui::layout::render_widget(&mut pc, &mut state.color_selector, cx + 12.0, cs_y, cs_w, cs_h);

    let y = cs_y + cs_h + 32.0;

    // Keyboard Shortcuts Section
    let mut sec_keys = SectionContext::new(&mut pc, cx, y, cw, "Keyboard Shortcuts", false);
    sec_keys.spacing(8.0);

    for (i, section) in SECTIONS.iter().enumerate() {
        // Section Header
        let header_str = format!("{} {}", section.icon, section.title);
        sec_keys.text(&header_str, 12.0, 0.0, 13.0, text_fg);
        sec_keys.spacing(20.0);

        for b in section.bindings {
            sec_keys.text(b.keys, 12.0, 0.0, 11.0, accent);
            sec_keys.text(b.action, 160.0, 0.0, 11.0, text_dim);
            sec_keys.spacing(16.0);
        }

        if i < SECTIONS.len() - 1 {
            sec_keys.spacing(8.0);
            sec_keys.separator();
            sec_keys.spacing(12.0);
        }
    }

    sec_keys.finish();

    pc
}
