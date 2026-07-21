pub mod browse;
pub mod preview;
pub mod network;

use cce_ui::layout::RenderTarget;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Browse,
    Network,
}

impl Page {
    pub const ALL: [Page; 2] = [
        Page::Browse,
        Page::Network,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Page::Browse => "Browse",
            Page::Network => "Network",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Page::Browse => "📁",
            Page::Network => "🌐",
        }
    }
}

pub struct PageContent {
    pub rects: Vec<([f32; 4], f32, f32, f32, f32, f32, (bool, bool, bool, bool))>,
    pub texts: Vec<(String, f32, f32, f32, [f32; 4], Option<String>, Option<[f32; 4]>)>,
    pub buttons: Vec<(cce_ui::widget::Adapted<cce_ui::widget::Button>, crate::Message)>,
    /// Relief steps for the control_relief styling — (x, y, w, h, radius, depth,
    /// raised). The flat rects own the faces; these are the edges-only boss/recess
    /// walls emitted over them (the ParametersBg::reliefs idiom for flat-view hosts).
    pub reliefs: Vec<(f32, f32, f32, f32, f32, f32, bool)>,
}

impl PageContent {
    pub fn new() -> Self {
        Self {
            rects: Vec::new(),
            texts: Vec::new(),
            buttons: Vec::new(),
            reliefs: Vec::new(),
        }
    }

    pub fn rect(&mut self, color: [f32; 4], x: f32, y: f32, w: f32, h: f32) {
        self.rects.push((color, x, y, w, h, 0.0, (true, true, true, true)));
    }

    /// A recessed well carved over the control at (x, y, w, h) — no-op when the
    /// DE's control_relief styling is off.
    pub fn relief_recessed(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32) {
        if cce_ui::layout::control_relief() {
            let depth = cce_ui::layout::bevel_width().min(h * 0.2);
            self.reliefs.push((x, y, w, h, radius, depth, false));
        }
    }

    /// A raised plateau over the control at (x, y, w, h) — no-op when the DE's
    /// control_relief styling is off.
    pub fn relief_raised(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32) {
        if cce_ui::layout::control_relief() {
            let depth = cce_ui::layout::bevel_width().min(h * 0.2);
            self.reliefs.push((x, y, w, h, radius, depth, true));
        }
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
        let btn = cce_ui::widget::Button::new(x, y, w, h)
            .with_label(label)
            .with_bg(bg)
            .with_hover_bg(hover_bg)
            .with_label_color(label_color);
        self.buttons.push((btn, action));
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
        let btn = cce_ui::widget::Button::new(x, y, w, h)
            .with_label(label)
            .with_bg(bg)
            .with_hover_bg(hover_bg)
            .with_label_color(label_color)
            .with_left_align(true);
        self.buttons.push((btn, action));
    }
}

impl RenderTarget for PageContent {
    fn rect(&mut self, color: [f32; 4], x: f32, y: f32, w: f32, h: f32) {
        self.rects.push((color, x, y, w, h, 0.0, (true, true, true, true)));
    }

    fn rect_with_radius(&mut self, color: [f32; 4], x: f32, y: f32, w: f32, h: f32, radius: f32) {
        self.rects.push((color, x, y, w, h, radius, (true, true, true, true)));
    }

    fn rect_with_radius_corners(&mut self, color: [f32; 4], x: f32, y: f32, w: f32, h: f32, radius: f32, corners: (bool, bool, bool, bool)) {
        self.rects.push((color, x, y, w, h, radius, corners));
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
