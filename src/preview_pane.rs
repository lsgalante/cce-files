//! The file-preview pane — Preview (text lines / image pixels) over Details
//! (name + metadata rows), drawn in cce-ui `SectionContext` titled frames.
//!
//! Moved in-crate from cce-ui's `PreviewState` (its only consumer was this app)
//! and flattened to the RowList idiom: a plain struct whose `push_prims` emits
//! rects and texts ONCE into a `PageContent`, per-text clip bounds pre-clamped
//! to the pane rect. This replaces the old Adapted<W> widget whose paint ran
//! twice per frame (quads via `Paint::paint`, text via the legacy-labels
//! hatch) and the caller-side text clamp in `rebuild_layout`. Scrolling is
//! `wheel()`, called imperatively from the app like `RowList::wheel`.

use std::path::PathBuf;

use cce_ui::layout::SectionContext;
use cce_ui::scene::layout::{fit_rect, FitMode, Rect};
use cce_ui::widget::display::{truncate_head, truncate_tail};
use cce_ui::widget::MouseScrollDelta;

use crate::pages::PageContent;

#[derive(Debug, Clone)]
pub struct PreviewPane {
    rect: (f32, f32, f32, f32),
    pub path: Option<PathBuf>,
    pub path_display: String,
    pub name: String,
    pub is_dir: bool,
    pub size: String,
    pub permissions: String,
    pub modified: String,
    pub file_type: String,
    pub target: String, // for symlinks
    pub content_preview: Option<String>,
    /// The uploaded preview texture as (image id, native w, native h). Owned
    /// exclusively through [`PreviewPane::set_image`] — the sole upload/free
    /// site, so a stale id can never leak against the renderer's image budget.
    image_tex: Option<(u32, u32, u32)>,
    pub scroll_line: usize,
}

impl Default for PreviewPane {
    fn default() -> Self {
        Self {
            rect: (0.0, 0.0, 0.0, 0.0),
            path: None,
            path_display: String::new(),
            name: String::new(),
            is_dir: false,
            size: String::new(),
            permissions: String::new(),
            modified: String::new(),
            file_type: String::new(),
            target: String::new(),
            content_preview: None,
            image_tex: None,
            scroll_line: 0,
        }
    }
}

impl PreviewPane {
    /// Replace (or clear) the preview texture. Always frees the previous id
    /// first; upload happens here — at update() level, never during paint.
    /// `img` is flat RGBA8 pixels + native dimensions.
    pub fn set_image(&mut self, img: Option<(Vec<u8>, u32, u32)>) {
        if let Some((id, _, _)) = self.image_tex.take() {
            cce_ui::vk::free_image(id);
        }
        if let Some((pixels, w, h)) = img {
            let id = cce_ui::vk::upload_rgba(pixels, w, h);
            self.image_tex = Some((id, w, h));
        }
    }

    pub fn set_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.rect = (x, y, w, h);
    }

    pub fn rect(&self) -> (f32, f32, f32, f32) {
        self.rect
    }

    /// Wheel routing: `None` if the pointer is outside the inner content-preview
    /// region (the caller falls through to the list/graph), `Some(changed)` if
    /// the event is consumed — scrolled or not, a wheel over the preview region
    /// never reaches the widgets beneath it.
    pub fn wheel(&mut self, delta: &MouseScrollDelta, mx: f32, my: f32) -> Option<bool> {
        let (prev_x, prev_y, prev_w, prev_h) = self.rect;
        // Inner content-preview region (below the metadata header).
        let px = prev_x + 12.0;
        let py = prev_y + 32.0;
        let pw = prev_w - 24.0;
        let ph = prev_h * 0.5 - 40.0;
        if mx < px || mx > px + pw || my < py || my > py + ph {
            return None;
        }
        Some(self.scroll(delta, prev_h))
    }

    fn scroll(&mut self, delta: &MouseScrollDelta, ch: f32) -> bool {
        let content = match &self.content_preview {
            Some(c) => c,
            None => return false,
        };
        let total_lines = content.lines().count();
        let half_h = ch * 0.5;
        let mut max_visible_lines = 0;
        let mut text_y = 44.0;
        while text_y + 14.0 <= half_h - 16.0 {
            max_visible_lines += 1;
            text_y += 15.0;
        }
        if total_lines <= max_visible_lines {
            if self.scroll_line != 0 {
                self.scroll_line = 0;
                return true;
            }
            return false;
        }
        let max_scroll = total_lines.saturating_sub(max_visible_lines);
        let scroll_speed = 3.0;
        let diff = match delta {
            MouseScrollDelta::LineDelta(_, y) => -y * scroll_speed,
            MouseScrollDelta::PixelDelta(pos) => -pos.y as f32 / 15.0,
        };
        let prev_scroll = self.scroll_line;
        let new_scroll = (self.scroll_line as f32 + diff).round() as isize;
        self.scroll_line = new_scroll.clamp(0, max_scroll as isize) as usize;
        self.scroll_line != prev_scroll
    }

    /// One pass: sections, fills, image runs, and text — every text emitted with
    /// bounds intersected against the pane rect (the old caller-side clamp).
    pub fn push_prims(&self, pc: &mut PageContent) {
        let text_start = pc.texts.len();
        self.emit(pc);

        let (cx, cy, cw, ch) = self.rect;
        let (pane_l, pane_t, pane_r, pane_b) = (cx, cy, cx + cw, cy + ch);
        let mut i = text_start;
        while i < pc.texts.len() {
            let bounds = &mut pc.texts[i].6;
            let b = bounds.unwrap_or([pane_l, pane_t, pane_r, pane_b]);
            let clamped = [
                b[0].max(pane_l),
                b[1].max(pane_t),
                b[2].min(pane_r),
                b[3].min(pane_b),
            ];
            if clamped[2] <= clamped[0] || clamped[3] <= clamped[1] {
                pc.texts.remove(i);
                continue;
            }
            *bounds = Some(clamped);
            i += 1;
        }
    }

    fn emit(&self, pc: &mut PageContent) {
        let (cx, cy, cw, ch) = self.rect;

        let text_fg = cce_ui::color::TEXT_FG;
        let text_dim = cce_ui::color::TEXT_DIM;
        let label_fg = cce_ui::color::TEXT_ACCENT;

        if self.path.is_none() {
            pc.text("Select a file to view details", cx + 12.0, cy + 12.0, 13.0, text_dim);
            return;
        }

        let half_h = ch * 0.5;
        let pad = cce_ui::layout::section_padding();

        // 1. Top pane: File Preview Section
        let mut preview_sec = SectionContext::new(pc, cx + 4.0, cy + 12.0, cw - 8.0, "Preview", false, false);
        let rect_y = cy + pad + 31.0;
        let rect_h = half_h - 2.0 * pad - 43.0;
        preview_sec.content_y = cy + half_h - pad - 12.0;
        preview_sec.finish();

        let bg_color = cce_ui::color::list_bg_color();
        pc.rect(bg_color, cx + 12.0, rect_y, cw - 24.0, rect_h);

        if let Some((id, img_w, img_h)) = self.image_tex {
            let fitted = fit_rect(
                img_w,
                img_h,
                Rect { x: cx + 12.0, y: rect_y, width: cw - 24.0, height: rect_h },
                FitMode::Contain { max_upscale: 4.0 },
            );
            pc.image(id, fitted.x, fitted.y, fitted.width, fitted.height, 1.0);
        } else if let Some(content) = &self.content_preview {
            let mut text_y = rect_y + 12.0;
            for line in content.lines().skip(self.scroll_line) {
                if text_y + 14.0 > rect_y + rect_h - 8.0 {
                    break;
                }
                let limit = (((cw - 40.0) / 6.8).floor() as usize).max(20);
                let line_truncated = truncate_tail(line, limit);
                pc.text_with_font(&line_truncated, cx + 20.0, text_y, 11.0, text_fg, "monospace");
                text_y += 15.0;
            }
        } else {
            pc.text("No preview available", cx + 20.0, rect_y + 12.0, 11.0, text_dim);
        }

        // 2. Bottom pane: Details Section
        let bottom_y = cy + half_h + 12.0;
        let icon = if self.is_dir { "📁" } else { "📄" };

        let details = [
            ("Path", &self.path_display),
            ("Type", &self.file_type),
            ("Size", &self.size),
            ("Permissions", &self.permissions),
            ("Modified", &self.modified),
        ];

        let details_content_start_y = bottom_y + pad + 19.0;
        let mut details_content_end_y = details_content_start_y + 36.0 + details.len() as f32 * 20.0;
        if !self.target.is_empty() {
            details_content_end_y += 24.0;
        }

        let mut details_sec = SectionContext::new(pc, cx + 4.0, bottom_y, cw - 8.0, "Details", false, false);
        details_sec.content_y = details_content_end_y;
        details_sec.finish();

        let header_y = details_content_start_y + 6.0;
        pc.text(icon, cx + 12.0, header_y, 20.0, text_fg);

        let name_truncated = truncate_tail(&self.name, 30);
        pc.text(&name_truncated, cx + 42.0, header_y + 4.0, 16.0, text_fg);

        let mut y = details_content_start_y + 36.0;
        for (label, val) in &details {
            pc.text(label, cx + 12.0, y, 12.0, label_fg);
            let val_str = truncate_head(val, 40);
            pc.text(&val_str, cx + 112.0, y, 12.0, text_dim);
            y += 20.0;
        }

        if !self.target.is_empty() {
            y += 8.0;
            pc.text("Target", cx + 12.0, y, 12.0, label_fg);
            let target_str = truncate_head(&self.target, 40);
            pc.text(&target_str, cx + 112.0, y, 12.0, text_dim);
        }
    }
}
