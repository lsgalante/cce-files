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
//!
//! **That fix holds only because these prims go into the PAGE's `PageContent`.**
//! `rebuild_layout` partitions `plain_pc`'s rects by radius and appends all the plain
//! ones before all the rounded ones, so anything routed through there is re-sorted
//! rather than drawn in call order. The bg here is rounded (`list_corner_radius`
//! defaults to 4.0) while the scrollbar and row overlays are radius-0, so moving this
//! emission to `plain_pc`/`window_pc` — as `PreviewPane::push_prims` does, which makes
//! it look like the natural thing to do — would sort the bg back after them and
//! reinstate the exact Phase 6v sandwich this note describes. It would also be silent:
//! the bg is translucent, so the overlays wash out rather than disappear.
//!
//! The scrollbar leans on the same call order, twice (the DE's centre-line rule, see
//! cce-ui's CLAUDE.md "Every scrollbar rides a centre line, behind the plate"): its
//! IDLE copy goes out before the bg, so the translucent well dims it, and its FORE
//! copy after the row overlays. Both are pills (rounded), so even a radius partition
//! would keep the two copies either side of the rounded bg — but it would still sink
//! the radius-0 row overlays under the bg, so the note above stands.

use cce_ui::widget::{Bounds, Justification, MouseScrollDelta, ScrollMotion, ScrollbarActivity, LINE_PX};

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
    /// The cce-icons glyph shown ahead of the first cell (`folder`,
    /// `file-image`, …), drawn in the first cell's colour.
    pub icon: Option<&'static str>,
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
    /// The DRAWN offset — `motion` glides it (wheel) or coasts it (trackpad
    /// flick); direct writes (thumb drag, scroll_into_view, the clamp) are
    /// adopted by the motion on its next step.
    pub scroll_y: f32,
    motion: ScrollMotion,
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
    /// The scrollbar's raise/sink latch and fade: it idles behind the well's
    /// translucent bg and takes no press there, until a scroll raises it.
    activity: ScrollbarActivity,
    /// Pointer focus (the app's well-focus tracking): the well renders as the
    /// tinted carve — accent ring replacing the relief lighting.
    pub focused: bool,
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
            motion: ScrollMotion::new(),
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
            activity: ScrollbarActivity::new(),
            focused: false,
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

    fn overflowing(&self) -> bool {
        self.content_h > self.viewport_h
    }

    /// A scroll happened: refresh the hold and latch at once, so the bar is
    /// up (and hittable) in the same frame rather than a tick later.
    fn raise(&mut self) {
        self.activity.bump();
        self.activity.recompute(self.overflowing(), self.dragging);
    }

    /// Whether the scrollbar is raised in front of the rows and takes presses.
    pub fn scrollbar_raised(&self) -> bool {
        self.activity.raised()
    }

    /// Keep `selected_idx`'s row inside the viewport (the browse view's auto-scroll —
    /// what keyboard navigation scrolls the list through). A move raises the bar.
    pub fn scroll_into_view(&mut self, selected_idx: usize) {
        let item_y = selected_idx as f32 * (self.item_height + self.item_gap) + 2.0;
        let old = self.scroll_y;
        if self.viewport_h > 0.0 {
            if item_y < self.scroll_y {
                self.scroll_y = item_y;
            } else if item_y + self.item_height > self.scroll_y + self.viewport_h {
                self.scroll_y = item_y + self.item_height - self.viewport_h;
            }
        }
        if (self.scroll_y - old).abs() > 0.01 {
            self.raise();
        }
    }

    /// Whether (px, py) is inside the list's full rect — public for the app's
    /// right-click menu on the empty space below the rows.
    pub fn hit(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    /// Screen y for row `idx`, or `None` when the row doesn't intersect the
    /// viewport at all (the toolkit ScrollRegion's intersection contract):
    /// partially visible rows ARE returned — their bg quads clamp and their
    /// text is bounds-clipped in `push_prims`, so an edge row renders cut,
    /// not culled.
    fn get_item_draw_y(&self, idx: usize) -> Option<f32> {
        let virtual_y = idx as f32 * (self.item_height + self.item_gap) + 2.0;
        let draw_y = self.y + virtual_y - self.scroll_y;
        if draw_y + self.item_height >= self.y - 1.0 && draw_y <= self.y + self.viewport_h + 1.0 {
            Some(draw_y)
        } else {
            None
        }
    }

    /// The visible row index under (px, py) — the click/hover hit-test, public
    /// for the app's right-click row menu. The viewport gate keeps the hidden
    /// part of an edge-straddling row unhittable: only its visible sliver
    /// matches, mirroring what `push_prims` draws.
    pub fn row_at(&self, px: f32, py: f32) -> Option<usize> {
        if py < self.y || py > self.y + self.viewport_h {
            return None;
        }
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

    // ── Scrollbar: the DE's centre-line bar (cce-ui's sink-behind `ScrollRegion`) ──

    /// (sb_x, track_y, sb_w, track_h, thumb_y, thumb_h): centred on the list's
    /// width, over the rows — no column reserves a lane for it.
    fn scrollbar_geom(&self) -> (f32, f32, f32, f32, f32, f32) {
        let sb_w = cce_ui::layout::centred_scrollbar_width();
        let sb_x = self.x + (self.w - sb_w) * 0.5;
        let track_h = self.viewport_h - 8.0;
        let track_y = self.y + 4.0;
        let visible_ratio = self.viewport_h / self.content_h.max(1.0);
        let thumb_h = if track_h <= 20.0 { track_h } else { (track_h * visible_ratio).clamp(20.0, track_h) };
        let scroll_ratio = if self.max_scroll() > 0.0 { self.scroll_y / self.max_scroll() } else { 0.0 };
        let thumb_y = track_y + scroll_ratio * (track_h - thumb_h);
        (sb_x, track_y, sb_w, track_h, thumb_y, thumb_h)
    }

    /// A press or hover on the bar. A SUNK bar is behind the well's bg and
    /// is not hit: a press on its lane is a press on the row under it.
    fn hit_scrollbar(&self, px: f32, py: f32) -> bool {
        if !self.overflowing() || !self.activity.raised() {
            return false;
        }
        let (sb_x, track_y, sb_w, track_h, _, _) = self.scrollbar_geom();
        px >= sb_x - 4.0 && px <= sb_x + sb_w + 4.0 && py >= track_y && py <= track_y + track_h
    }

    // ── Input ──

    /// Scrollbar drag + row hover (`List::on_cursor_moved` + `ScrollBox` drag).
    pub fn cursor_moved(&mut self, px: f32, py: f32) -> bool {
        let mut changed = false;
        // Gated on raised (`hit_scrollbar`): hover holds a raised bar up and
        // never raises a sunk one.
        self.activity.set_hover(self.hit_scrollbar(px, py));
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
            if was_dragging {
                // The hold starts at the release, so the bar lingers.
                self.activity.bump();
            }
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

    /// Hit-scoped wheel (`ScrollBox::mouse_wheel`). A wheel notch moves the
    /// target and [`Self::tick`] glides the offset there; a trackpad finger
    /// moves the offset now. True when either moved (repaint).
    pub fn wheel(&mut self, delta: &MouseScrollDelta, px: f32, py: f32) -> bool {
        if !self.hit(px, py) {
            return false;
        }
        self.motion.reconcile(0.0, self.scroll_y);
        let moved = self.motion.apply(delta, (LINE_PX, LINE_PX), Bounds::max(0.0), Bounds::max(self.max_scroll()));
        self.scroll_y = self.motion.y.pos();
        if moved {
            self.raise();
        }
        moved
    }

    /// Advance the wheel glide / flick coast and the scrollbar's raise/sink;
    /// true while the offset is moving, the bar's hold is running or its fade
    /// is moving, so the host keeps frames coming until all of it settles.
    pub fn tick(&mut self, dt: f32) -> bool {
        self.motion.reconcile(0.0, self.scroll_y);
        let mut moved = false;
        if self.motion.is_animating() {
            moved = self.motion.tick(dt, Bounds::max(0.0), Bounds::max(self.max_scroll()));
            self.scroll_y = self.motion.y.pos();
            if moved {
                // A glide or coast in motion is a scroll: it keeps the bar up.
                self.activity.bump();
            }
        }
        let animating = self.motion.is_animating();
        let holding = self.activity.holding();
        let flipped = self.activity.tick(dt, self.overflowing(), self.dragging);
        moved || animating || holding || flipped
    }

    /// The pill track and thumb at `alpha` (track/thumb colours scaled).
    fn push_scrollbar(&self, pc: &mut crate::pages::PageContent, alpha: f32) {
        use cce_ui::layout::RenderTarget;
        let a = alpha.clamp(0.0, 1.0);
        if !self.overflowing() || a <= 0.001 {
            return;
        }
        let dim = |mut c: [f32; 4]| {
            c[3] *= a;
            c
        };
        let all = (true, true, true, true);
        let (sb_x, track_y, sb_w, track_h, thumb_y, thumb_h) = self.scrollbar_geom();
        pc.rect_with_radius_corners(
            dim(cce_ui::color::scrollbar_track_color()),
            sb_x,
            track_y,
            sb_w,
            track_h,
            sb_w.min(track_h) * 0.5,
            all,
        );
        pc.rect_with_radius_corners(
            dim(cce_ui::color::scrollbar_thumb_color()),
            sb_x,
            thumb_y,
            sb_w,
            thumb_h,
            sb_w.min(thumb_h) * 0.5,
            all,
        );
    }

    // ── Paint ──

    /// The legacy frame, single-drawn and in the right order: the scrollbar's idle copy,
    /// the rounded bg (this list ran with `show_border = false`), row overlays, the
    /// scrollbar's fore copy, then the row text — the `List` column-branch cell layout
    /// (icon column, primary/secondary sizes and tints, char-estimate truncation,
    /// viewport-inset clip bounds) verbatim.
    pub fn push_prims(&self, pc: &mut crate::pages::PageContent) {
        use cce_ui::layout::RenderTarget;
        let (x, y, w, h) = (self.x, self.y, self.w, self.h);
        // The idle copy, at full alpha and every frame, raised or not: the
        // translucent bg over it is what sinks it, and the fore copy fades in
        // over it, so dropping it at the latch would blink the bar.
        self.push_scrollbar(pc, 1.0);
        pc.rect_with_radius_corners(
            cce_ui::color::list_bg_color(),
            x,
            y,
            w,
            h,
            cce_ui::layout::list_corner_radius(),
            (true, true, true, true),
        );
        // Recessed well like a text box: the list floor sits below the pane
        // surface, its wall carved over the bg and row overlays. Focused, the
        // well swaps that lighting for the accent ring (the tinted carve).
        if self.focused {
            pc.relief_recessed_focused(x, y, w, h, cce_ui::layout::list_corner_radius());
        } else {
            pc.relief_recessed(x, y, w, h, cce_ui::layout::list_corner_radius());
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
                // Clamped to the viewport: `get_item_draw_y` returns partial
                // rows, and an unclamped hover/selection quad would bleed out
                // of the well (exact for a flat quad).
                let qy = draw_y.max(y);
                let qb = (draw_y + self.item_height).min(y + self.viewport_h);
                if qb > qy {
                    pc.rect(bg, x + 2.0, qy, w - 4.0, qb - qy);
                }
            }

            let row_fg = if row.selected { fg } else { plain_fg };
            let row_dim = if row.selected { fg } else { dim };
            let y_primary = cce_ui::layout::center_text_y(draw_y, self.item_height, primary_size);
            let y_secondary = cce_ui::layout::center_text_y(draw_y, self.item_height, secondary_size);

            let mut start_text_offset = 8.0;
            if let Some(icon) = row.icon {
                if !col_bounds.is_empty() {
                    // A glyph the size of the name's text, centred on the row.
                    let side = primary_size;
                    let gy = draw_y + (self.item_height - side) * 0.5;
                    pc.icon_bounded(icon, x + col_bounds[0].0 + 12.0, gy, side, side, row_fg, clip);
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

        // The fore copy, after the row overlays, at the fade: it keeps drawing
        // all the way out rather than stopping at the latch. (A part's icons and
        // reliefs are emitted after all its rects, and text after everything,
        // so the row glyphs, the well's wall and the labels still land over it.)
        self.push_scrollbar(pc, self.activity.fade());
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
            .map(|i| Row { cells: vec![format!("f{i}"), "1 KiB".into()], icon: Some("file"), selected: false })
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
        // Two notches glide to 2 * LINE_PX; settle the motion first.
        for _ in 0..600 {
            if !l.tick(1.0 / 60.0) {
                break;
            }
        }
        assert_eq!(l.scroll_y, 48.0);
        l.scroll_into_view(0);
        assert_eq!(l.scroll_y, 2.0);
        l.scroll_into_view(49);
        let expect = 49.0 * (l.item_height + 2.0) + 2.0 + l.item_height - l.viewport_h;
        assert!((l.scroll_y - expect).abs() < 0.01);
    }

    #[test]
    fn the_scrollbar_rides_the_centre_and_sinks_until_scrolled() {
        let mut l = list_with_rows(50);
        let ih = l.item_height;
        // Centred on the list's width (x 10, w 400), centred-bar thick.
        let (sb_x, track_y, sb_w, _, _, _) = l.scrollbar_geom();
        assert!((sb_x + sb_w * 0.5 - 210.0).abs() < 0.01);
        assert!((sb_w - cce_ui::layout::centred_scrollbar_width()).abs() < 0.01);
        let lane_x = sb_x + sb_w * 0.5;

        // Sunk: a press on the lane is a press on the row under it.
        assert!(!l.scrollbar_raised());
        let row1_y = 20.0 + 1.0 * (ih + 2.0) + 2.0 + ih / 2.0;
        assert!(l.mouse_input(true, lane_x, row1_y));
        assert!(!l.dragging);
        assert!(l.mouse_input(false, lane_x, row1_y));
        assert_eq!(l.take_click(), Some(1));

        // Sunk, the idle copy is drawn before the bg and the fore copy not
        // at all (fade 0); both pills.
        let mut pc = crate::pages::PageContent::new();
        l.push_prims(&mut pc);
        let bg = cce_ui::color::list_bg_color();
        let bg_at = pc.rects.iter().position(|r| r.0 == bg && r.1 == l.x).expect("bg");
        let bars: Vec<usize> = (0..pc.rects.len()).filter(|&i| pc.rects[i].1 == sb_x).collect();
        assert_eq!(bars.len(), 2, "track and thumb, idle copy only");
        assert!(bars.iter().all(|&i| i < bg_at && pc.rects[i].5 > 0.0));

        // A wheel raises it; the fade brings the fore copy in after the rows.
        assert!(l.wheel(&MouseScrollDelta::LineDelta(0.0, -2.0), 50.0, 50.0));
        assert!(l.scrollbar_raised());
        for _ in 0..30 {
            l.tick(1.0 / 60.0);
        }
        assert!(l.activity.fade() > 0.99);
        let mut pc = crate::pages::PageContent::new();
        l.push_prims(&mut pc);
        let bars: Vec<usize> = (0..pc.rects.len()).filter(|&i| pc.rects[i].1 == sb_x).collect();
        assert_eq!(bars.len(), 4, "idle copy and fore copy");
        assert_eq!(bars[2], pc.rects.len() - 2, "the fore copy is the last thing drawn");

        // Raised, a press on the thumb grabs it, not the row under it.
        let (_, _, _, _, thumb_y, thumb_h) = l.scrollbar_geom();
        assert!(l.mouse_input(true, lane_x, thumb_y + thumb_h * 0.5));
        assert!(l.dragging);
        assert!(l.pressed_row.is_none());
        l.mouse_input(false, lane_x, thumb_y + thumb_h * 0.5);
        assert!(!l.dragging);

        // A pointer over the raised bar holds it up past the hold…
        l.cursor_moved(lane_x, track_y + 10.0);
        for _ in 0..180 {
            l.tick(1.0 / 60.0);
        }
        assert!(l.scrollbar_raised(), "hover holds a raised bar");

        // …and once it leaves, the bar sinks and its fore copy fades out.
        l.cursor_moved(50.0, 50.0);
        let mut ticked_while_sinking = false;
        for _ in 0..180 {
            ticked_while_sinking |= l.tick(1.0 / 60.0);
        }
        assert!(ticked_while_sinking, "the hold and the fade keep frames coming");
        assert!(!l.scrollbar_raised());
        assert_eq!(l.activity.fade(), 0.0);
        assert!(!l.tick(1.0 / 60.0), "settled: no more frames");

        // Hover never raises a sunk bar.
        l.cursor_moved(lane_x, track_y + 10.0);
        l.tick(1.0 / 60.0);
        assert!(!l.scrollbar_raised());

        // A programmatic scroll (keyboard navigation's scroll_into_view) raises it.
        l.scroll_into_view(49);
        assert!(l.scrollbar_raised());
    }
}
