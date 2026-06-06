pub mod browse;
pub mod preview;
pub mod network;
pub mod settings;

use clear_ui::layout::RenderTarget;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Browse,
    Network,
    Settings,
}

impl Page {
    pub const ALL: [Page; 3] = [
        Page::Browse,
        Page::Network,
        Page::Settings,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Page::Browse => "Browse",
            Page::Network => "Network",
            Page::Settings => "Settings",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Page::Browse => "📁",
            Page::Network => "🌐",
            Page::Settings => "⚙",
        }
    }
}

#[derive(Clone)]
pub struct ContentButton {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub bg: [f32; 4],
    pub hover_bg: [f32; 4],
    pub label: String,
    pub label_size: f32,
    pub label_color: [f32; 4],
    pub action: crate::Message,
    pub left_align: bool,
}

pub struct PageContent {
    pub rects: Vec<([f32; 4], f32, f32, f32, f32)>,
    pub texts: Vec<(String, f32, f32, f32, [f32; 4], Option<String>, Option<[f32; 4]>)>,
    pub buttons: Vec<ContentButton>,
}

impl PageContent {
    pub fn new() -> Self {
        Self {
            rects: Vec::new(),
            texts: Vec::new(),
            buttons: Vec::new(),
        }
    }

    pub fn rect(&mut self, color: [f32; 4], x: f32, y: f32, w: f32, h: f32) {
        self.rects.push((color, x, y, w, h));
    }

    pub fn text(&mut self, content: &str, x: f32, y: f32, size: f32, color: [f32; 4]) {
        self.texts.push((content.to_string(), size, x, y, color, None, None));
    }

    pub fn text_with_font(&mut self, content: &str, x: f32, y: f32, size: f32, color: [f32; 4], font: &str) {
        self.texts.push((content.to_string(), size, x, y, color, Some(font.to_string()), None));
    }

    pub fn button(
        &mut self,
        label: &str,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        bg: [f32; 4],
        hover_bg: [f32; 4],
        label_color: [f32; 4],
        action: crate::Message,
    ) {
        self.buttons.push(ContentButton {
            x,
            y,
            w,
            h,
            bg,
            hover_bg,
            label: label.to_string(),
            label_size: 12.0,
            label_color,
            action,
            left_align: false,
        });
    }

    pub fn button_left(
        &mut self,
        label: &str,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        bg: [f32; 4],
        hover_bg: [f32; 4],
        label_color: [f32; 4],
        action: crate::Message,
    ) {
        self.buttons.push(ContentButton {
            x,
            y,
            w,
            h,
            bg,
            hover_bg,
            label: label.to_string(),
            label_size: 12.0,
            label_color,
            action,
            left_align: true,
        });
    }
}

impl RenderTarget for PageContent {
    fn rect(&mut self, color: [f32; 4], x: f32, y: f32, w: f32, h: f32) {
        self.rects.push((color, x, y, w, h));
    }

    fn text(&mut self, content: &str, x: f32, y: f32, size: f32, color: [f32; 4]) {
        self.texts.push((content.to_string(), size, x, y, color, None, None));
    }

    fn text_with_font(&mut self, content: &str, x: f32, y: f32, size: f32, color: [f32; 4], font: &str) {
        self.texts.push((content.to_string(), size, x, y, color, Some(font.to_string()), None));
    }

    fn text_with_bounds(&mut self, content: &str, x: f32, y: f32, size: f32, color: [f32; 4], bounds: Option<[f32; 4]>) {
        self.texts.push((content.to_string(), size, x, y, color, None, bounds));
    }

    fn text_with_font_and_bounds(&mut self, content: &str, x: f32, y: f32, size: f32, color: [f32; 4], font: &str, bounds: Option<[f32; 4]>) {
        self.texts.push((content.to_string(), size, x, y, color, Some(font.to_string()), bounds));
    }
}
