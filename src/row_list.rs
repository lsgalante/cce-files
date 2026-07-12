//! App-owned column list replacing the dissolved `List`/`ScrollBox` embedded bases
//! (Phase 6z) — the column-mode flavor: columns (Flex/Absolute/RightOffset), rows with
//! icon + cells, hover/press/selection overlays, click + double-click, and the scroll
//! frame (wheel, scrollbar thumb drag / track jump). The geometry, colors, truncation,
//! and hit math are the legacy `List` column branch verbatim; the search box that used
//! to live inside the `List` is a standalone app widget now.
//!
//! One deliberate paint fix: `render_widget(List)` emitted the plain quads (scrollbar,
//! row overlays) BEFORE the rounded background, washing them under the translucent bg —
//! the same sandwich the settings lists had (Phase 6v). `push_prims` draws bg first.

use cce_ui::widget::{Justification, MouseScrollDelta};

/// Column sizing (moved here with the cce-ui `List` deletion — RowList is the only
/// remaining consumer of the column model).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColumnWidth {
    Flex,
    Absolute(f32),
    RightOffset(f32),
}

#[derive(Debug, Clone)]
pub struct ListColumn {
    pub name: String,
    pub width: ColumnWidth,
    pub justification: Justification,
}

#[derive(Debug, Clone)]
pub struct Row {
    pub cells: Vec<String>,
    pub icon: Option<String>,
    pub selected: bool,
}

#[derive(Debug, Clone)]
pub struct RowList {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Rows viewport: the full rect minus the in-frame search strip when it is open.
    pub viewport_h: f32,
    /// Row height with `List::new`'s silent adjustment to `max(item_height, list_font + 14)`.
    pub item_height: f32,
    pub item_gap: f32,
    pub scroll_y: f32,
    pub content_h: f32,
    pub columns: Vec<ListColumn>,
    pub rows: Vec<Row>,
    pub hovered_row: Option<usize>,
    pub pressed_row: Option<usize>,
    clicked_row: Option<usize>,
    double_clicked_row: Option<usize>,
    last_click_time: Option<std::time::Instant>,
    pub dragging: bool,
    drag_offset_y: f32,
}

impl RowList {
    pub fn new(item_height: f32, item_gap: f32) -> Self {
        let (_, font_size) = cce_ui::layout::list_font_parsed();
        Self {
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
            viewport_h: 0.0,
            item_height: item_height.max(font_size + 14.0),
            item_gap,
            scroll_y: 0.0,
            content_h: 0.0,
            columns: Vec::new(),
            rows: Vec::new(),
            hovered_row: None,
            pressed_row: None,
            clicked_row: None,
            double_clicked_row: None,
            last_click_time: None,
            dragging: false,
            drag_offset_y: 0.0,
        }
    }

    /// `search_offset` shrinks the rows viewport from the bottom (the legacy
    /// `List::set_rect` search reservation); the background still covers the full rect.
    pub fn set_rect(&mut self, x: f32, y: f32, w: f32, h: f32, search_offset: f32) {
        self.x = x;
        self.y = y;
        self.w = w;
        self.h = h;
        self.viewport_h = (h - search_offset).max(0.0);
    }

    /// `List::update_bounds_from_rows`: content height from the row count, scroll clamped.
    pub fn update_bounds_from_rows(&mut self) {
        self.content_h = self.rows.len() as f32 * (self.item_height + self.item_gap) + 4.0;
        self.scroll_y = self.scroll_y.clamp(0.0, self.max_scroll());
    }

    fn max_scroll(&self) -> f32 {
        (self.content_h - self.viewport_h).max(0.0)
    }

    /// Keep `selected_idx`'s row inside the viewport (the browse view's auto-scroll).
    pub fn scroll_into_view(&mut self, selected_idx: usize) {
        let item_y = selected_idx as f32 * (self.item_height + self.item_gap) + 2.0;
        if self.viewport_h > 0.0 {
            if item_y < self.scroll_y {
                self.scroll_y = item_y;
            } else if item_y + self.item_height > self.scroll_y + self.viewport_h {
                self.scroll_y = item_y + self.item_height - self.viewport_h;
            }
        }
    }

    fn hit(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    /// `ScrollBox::get_item_draw_y` over the row index (rows fully visible only).
    fn get_item_draw_y(&self, idx: usize) -> Option<f32> {
        let virtual_y = idx as f32 * (self.item_height + self.item_gap) + 2.0;
        let draw_y = self.y + virtual_y - self.scroll_y;
        if draw_y >= self.y - 1.0 && draw_y + self.item_height <= self.y + self.viewport_h + 1.0 {
            Some(draw_y)
        } else {
            None
        }
    }

    fn row_at(&self, px: f32, py: f32) -> Option<usize> {
        for idx in 0..self.rows.len() {
            if let Some(draw_y) = self.get_item_draw_y(idx) {
                if px >= self.x + 2.0 && px <= self.x + self.w - 2.0 && py >= draw_y && py <= draw_y + self.item_height {
                    return Some(idx);
                }
            }
        }
        None
    }

    /// `List::get_column_bounds`, verbatim: per-column (x-offset, width) for our width.
    pub fn column_bounds(&self) -> Vec<(f32, f32)> {
        let cols = &self.columns;
        let list_w = self.w;
        let mut bounds = vec![(0.0, 0.0); cols.len()];
        let mut flex_indices = Vec::new();
        let mut reserved_width = 0.0;

        for (i, col) in cols.iter().enumerate() {
            match col.width {
                ColumnWidth::Absolute(w) => {
                    bounds[i] = (0.0, w);
                    reserved_width += w;
                }
                ColumnWidth::RightOffset(offset) => {
                    let x = list_w - offset;
                    let mut next_x = list_w;
                    for j in (i + 1)..cols.len() {
                        if let ColumnWidth::RightOffset(o) = cols[j].width {
                            next_x = list_w - o;
                            break;
                        }
                    }
                    bounds[i] = (x, (next_x - x).max(0.0));
                }
                ColumnWidth::Flex => flex_indices.push(i),
            }
        }

        let current_x = 0.0;
        let mut right_boundary = list_w;
        for col in cols.iter() {
            if let ColumnWidth::RightOffset(offset) = col.width {
                if list_w - offset < right_boundary {
                    right_boundary = list_w - offset;
                }
            }
        }
        let left_space = (right_boundary - current_x).max(0.0);
        let flex_share = if !flex_indices.is_empty() {
            (left_space - reserved_width).max(0.0) / flex_indices.len() as f32
        } else {
            0.0
        };

        let mut cx = current_x;
        for (i, col) in cols.iter().enumerate() {
            match col.width {
                ColumnWidth::Absolute(w) => {
                    bounds[i] = (cx, w);
                    cx += w;
                }
                ColumnWidth::Flex => {
                    bounds[i] = (cx, flex_share);
                    cx += flex_share;
                }
                ColumnWidth::RightOffset(_) => {}
            }
        }
        bounds
    }

    pub fn take_click(&mut self) -> Option<usize> {
        self.clicked_row.take()
    }

    pub fn take_double_click(&mut self) -> Option<usize> {
        self.double_clicked_row.take()
    }

    // ── Scrollbar (`ScrollBox` geometry, verbatim) ──

    fn scrollbar_geom(&self) -> (f32, f32, f32, f32, f32, f32) {
        let sb_w = cce_ui::layout::scrollbar_width();
        let sb_x = self.x + self.w - sb_w - 4.0;
        let track_h = self.viewport_h - 8.0;
        let track_y = self.y + 4.0;
        let visible_ratio = self.viewport_h / self.content_h.max(1.0);
        let thumb_h = if track_h <= 20.0 { track_h } else { (track_h * visible_ratio).clamp(20.0, track_h) };
        let scroll_ratio = if self.max_scroll() > 0.0 { self.scroll_y / self.max_scroll() } else { 0.0 };
        let thumb_y = track_y + scroll_ratio * (track_h - thumb_h);
        (sb_x, track_y, sb_w, track_h, thumb_y, thumb_h)
    }

    fn hit_scrollbar(&self, px: f32, py: f32) -> bool {
        if self.content_h <= self.viewport_h {
            return false;
        }
        let (sb_x, track_y, sb_w, track_h, _, _) = self.scrollbar_geom();
        px >= sb_x - 4.0 && px <= sb_x + sb_w + 4.0 && py >= track_y && py <= track_y + track_h
    }

    // ── Input ──

    /// Scrollbar drag + row hover (`List::on_cursor_moved` + `ScrollBox` drag).
    pub fn cursor_moved(&mut self, px: f32, py: f32) -> bool {
        let mut changed = false;
        if self.dragging {
            let (_, track_y, _, track_h, _, thumb_h) = self.scrollbar_geom();
            let target = py - self.drag_offset_y;
            let ratio = if track_h - thumb_h > 0.0 {
                ((target - track_y) / (track_h - thumb_h)).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let old = self.scroll_y;
            self.scroll_y = ratio * self.max_scroll();
            if (self.scroll_y - old).abs() > 0.01 {
                changed = true;
            }
        }
        let new_hovered = self.row_at(px, py);
        if new_hovered != self.hovered_row {
            self.hovered_row = new_hovered;
            changed = true;
        }
        changed
    }

    /// `List::mouse_input` for left press/release: scrollbar thumb grab / track jump,
    /// then row press → click (+ double-click within 400ms). Returns handled.
    pub fn mouse_input(&mut self, pressed: bool, px: f32, py: f32) -> bool {
        if pressed {
            if self.hit_scrollbar(px, py) {
                self.dragging = true;
                let (_, track_y, _, track_h, thumb_y, thumb_h) = self.scrollbar_geom();
                let click_offset = py - thumb_y;
                if click_offset >= 0.0 && click_offset <= thumb_h {
                    self.drag_offset_y = click_offset;
                } else {
                    self.drag_offset_y = thumb_h / 2.0;
                    let target = py - self.drag_offset_y;
                    let ratio = if track_h - thumb_h > 0.0 {
                        ((target - track_y) / (track_h - thumb_h)).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    self.scroll_y = ratio * self.max_scroll();
                }
                return true;
            }
            self.dragging = false;
            if let Some(idx) = self.row_at(px, py) {
                self.pressed_row = Some(idx);
                return true;
            }
            false
        } else {
            let was_dragging = std::mem::take(&mut self.dragging);
            let mut clicked = false;
            if let Some(idx) = self.row_at(px, py) {
                if self.pressed_row == Some(idx) {
                    let now = std::time::Instant::now();
                    if let Some(last) = self.last_click_time {
                        if now.duration_since(last) < std::time::Duration::from_millis(400) {
                            self.double_clicked_row = Some(idx);
                        }
                    }
                    self.last_click_time = Some(now);
                    self.clicked_row = Some(idx);
                    clicked = true;
                }
            }
            self.pressed_row = None;
            clicked || was_dragging
        }
    }

    /// Hit-scoped wheel (`ScrollBox::mouse_wheel`).
    pub fn wheel(&mut self, delta: &MouseScrollDelta, px: f32, py: f32) -> bool {
        if !self.hit(px, py) {
            return false;
        }
        let dy = match delta {
            MouseScrollDelta::LineDelta(_, y) => -y * 24.0,
            MouseScrollDelta::PixelDelta(pos) => -pos.y as f32,
        };
        let old = self.scroll_y;
        self.scroll_y = (self.scroll_y + dy).clamp(0.0, self.max_scroll());
        (self.scroll_y - old).abs() > 0.01
    }

    // ── Paint ──

    /// The legacy frame, single-drawn and in the right order: rounded bg (this list ran
    /// with `show_border = false`), scrollbar, row overlays, then the row text — the
    /// `List` column-branch cell layout (icon column, primary/secondary sizes and tints,
    /// char-estimate truncation, viewport-inset clip bounds) verbatim.
    pub fn push_prims(&self, pc: &mut crate::pages::PageContent) {
        use cce_ui::layout::RenderTarget;
        let (x, y, w, h) = (self.x, self.y, self.w, self.h);
        pc.rect_with_radius_corners(
            cce_ui::color::list_bg_color(),
            x,
            y,
            w,
            h,
            cce_ui::layout::list_corner_radius(),
            (true, true, true, true),
        );

        if self.content_h > self.viewport_h {
            let (sb_x, track_y, sb_w, track_h, thumb_y, thumb_h) = self.scrollbar_geom();
            pc.rect(cce_ui::color::scrollbar_track_color(), sb_x, track_y, sb_w, track_h);
            pc.rect(cce_ui::color::scrollbar_thumb_color(), sb_x, thumb_y, sb_w, thumb_h);
        }

        let col_bounds = self.column_bounds();
        let (_, config_size) = cce_ui::layout::list_font_parsed();
        let primary_size = config_size;
        let secondary_size = (config_size - 1.0).max(8.0);
        let list_font = cce_ui::layout::list_font();
        let font = if list_font.is_empty() { None } else { Some(list_font) };

        let view_min = y + 4.0;
        let view_max = y + self.viewport_h - 4.0;
        let clip = Some([x, view_min, x + w, view_max]);

        let fg = [230.0 / 255.0, 230.0 / 255.0, 242.0 / 255.0, 1.0];
        let plain_fg = [178.0 / 255.0, 178.0 / 255.0, 191.0 / 255.0, 1.0];
        let dim = [140.0 / 255.0, 140.0 / 255.0, 153.0 / 255.0, 1.0];

        for (idx, row) in self.rows.iter().enumerate() {
            let Some(draw_y) = self.get_item_draw_y(idx) else { continue };
            let is_hovered = self.hovered_row == Some(idx);
            let is_pressed = self.pressed_row == Some(idx);
            let bg = if row.selected {
                if is_pressed {
                    [0.30, 0.52, 0.78, 0.6]
                } else if is_hovered {
                    [0.30, 0.52, 0.78, 0.5]
                } else {
                    [0.20, 0.40, 0.65, 0.4]
                }
            } else if is_pressed {
                [0.20, 0.20, 0.25, 0.25]
            } else if is_hovered {
                [0.20, 0.20, 0.25, 0.15]
            } else {
                [0.0, 0.0, 0.0, 0.0]
            };
            if bg[3] > 0.001 {
                pc.rect(bg, x + 2.0, draw_y, w - 4.0, self.item_height);
            }

            let row_fg = if row.selected { fg } else { plain_fg };
            let row_dim = if row.selected { fg } else { dim };
            let y_primary = cce_ui::layout::center_text_y(draw_y, self.item_height, primary_size);
            let y_secondary = cce_ui::layout::center_text_y(draw_y, self.item_height, secondary_size);

            let mut start_text_offset = 8.0;
            if let Some(ref icon) = row.icon {
                if !col_bounds.is_empty() {
                    push_text(pc, icon, x + col_bounds[0].0 + 12.0, y_primary, primary_size, row_fg, &font, clip);
                    start_text_offset = 32.0;
                }
            }

            for (c_idx, cell_text) in row.cells.iter().enumerate() {
                if c_idx >= col_bounds.len() {
                    break;
                }
                let (col_x, col_w) = col_bounds[c_idx];
                if col_w <= 0.0 {
                    continue;
                }
                let cell_color = if c_idx == 0 { row_fg } else { row_dim };
                let cell_y = if c_idx == 0 { y_primary } else { y_secondary };
                let cell_size = if c_idx == 0 { primary_size } else { secondary_size };
                let cell_draw_x = if c_idx == 0 { x + col_x + start_text_offset } else { x + col_x };
                let max_w = if c_idx == 0 { col_w - start_text_offset - 8.0 } else { col_w - 8.0 };
                let char_w = cell_size * 0.65;
                let max_chars = (max_w / char_w).max(4.0) as usize;
                let truncated = if cell_text.chars().count() > max_chars {
                    let mut s: String = cell_text.chars().take(max_chars.saturating_sub(3)).collect();
                    s.push_str("...");
                    s
                } else {
                    cell_text.clone()
                };
                push_text(pc, &truncated, cell_draw_x, cell_y, cell_size, cell_color, &font, clip);
            }
        }
    }
}

fn push_text(
    pc: &mut crate::pages::PageContent,
    text: &str,
    x: f32,
    y: f32,
    size: f32,
    color: [f32; 4],
    font: &Option<String>,
    bounds: Option<[f32; 4]>,
) {
    use cce_ui::layout::RenderTarget;
    match font {
        Some(f) => pc.text_with_font_and_bounds(text, x, y, size, color, f, bounds),
        None => pc.text_with_bounds(text, x, y, size, color, bounds),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cce_ui::widget::Justification;

    fn list_with_rows(n: usize) -> RowList {
        let mut l = RowList::new(30.0, 2.0);
        l.set_rect(10.0, 20.0, 400.0, 200.0, 0.0);
        l.columns = vec![
            ListColumn { name: "Name".into(), width: ColumnWidth::Flex, justification: Justification::Left },
            ListColumn { name: "Size".into(), width: ColumnWidth::RightOffset(120.0), justification: Justification::Left },
        ];
        l.rows = (0..n)
            .map(|i| Row { cells: vec![format!("f{i}"), "1 KiB".into()], icon: Some("D".into()), selected: false })
            .collect();
        l.update_bounds_from_rows();
        l
    }

    #[test]
    fn column_bounds_flex_and_right_offset() {
        let l = list_with_rows(1);
        let b = l.column_bounds();
        // RightOffset(120) claims the last 120px; Flex takes what's left of the row.
        assert_eq!(b[1], (280.0, 120.0));
        assert_eq!(b[0].0, 0.0);
        assert!((b[0].1 - 280.0).abs() < 0.01);
    }

    #[test]
    fn click_and_double_click() {
        let mut l = list_with_rows(5);
        let ih = l.item_height;
        let row1_y = 20.0 + 1.0 * (ih + 2.0) + 2.0 + ih / 2.0;
        assert!(l.mouse_input(true, 50.0, row1_y));
        assert!(l.mouse_input(false, 50.0, row1_y));
        assert_eq!(l.take_click(), Some(1));
        assert!(l.take_double_click().is_none());
        // Second click within 400ms → double.
        l.mouse_input(true, 50.0, row1_y);
        l.mouse_input(false, 50.0, row1_y);
        assert_eq!(l.take_double_click(), Some(1));
    }

    #[test]
    fn wheel_scrolls_and_scroll_into_view_clamps() {
        let mut l = list_with_rows(50);
        assert!(l.wheel(&MouseScrollDelta::LineDelta(0.0, -2.0), 50.0, 50.0));
        assert_eq!(l.scroll_y, 48.0);
        l.scroll_into_view(0);
        assert_eq!(l.scroll_y, 2.0);
        l.scroll_into_view(49);
        let expect = 49.0 * (l.item_height + 2.0) + 2.0 + l.item_height - l.viewport_h;
        assert!((l.scroll_y - expect).abs() < 0.01);
    }
}
