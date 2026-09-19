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
use cce_ui::widget::display::{measure_text_width, truncate_tail};
use cce_ui::widget::{Bounds, MouseScrollDelta, ScrollMotion};

/// Truncate `s` to fit `avail` px, measured for real (resvg-backed, cached per
/// string+size — the handful of details strings re-measure only on selection
/// change). `head` replaces the front ("...ail/of/path"), else the back
/// ("name..."). Binary search on kept chars: ~7 probes for a long path.
fn truncate_px(s: &str, family: &str, size: f32, avail: f32, head: bool) -> String {
    if measure_text_width(s, family, size) <= avail {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let build = |keep: usize| -> String {
        if head {
            let tail: String = chars[chars.len() - keep..].iter().collect();
            format!("...{tail}")
        } else {
            let kept: String = chars[..keep].iter().collect();
            format!("{kept}...")
        }
    };
    let (mut lo, mut hi) = (0usize, chars.len().saturating_sub(1));
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        if measure_text_width(&build(mid), family, size) <= avail {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    build(lo)
}

use crate::pages::PageContent;

/// Pitch of the text preview's lines (11px monospace on a 15px advance).
const PREVIEW_LINE_H: f32 = 15.0;
/// One wheel notch moves the text preview three lines (the legacy speed).
const PREVIEW_NOTCH_PX: f32 = 3.0 * PREVIEW_LINE_H;

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
    /// Text-preview scroll offset in pixels — the DRAWN value, which
    /// `scroll_motion` glides (wheel) or coasts (trackpad flick). The paint
    /// derives the first whole line and a sub-line remainder from it, so a
    /// multi-notch wheel slides through the lines instead of stepping.
    scroll_px: f32,
    scroll_motion: ScrollMotion,
    /// Which of the pane's two wells holds pointer focus (the app's well-focus
    /// tracking): that well renders as the tinted carve — accent ring
    /// replacing the relief lighting.
    pub focused_well: Option<PreviewWell>,
}

/// The preview pane's two recessed wells: the file-preview section (top half)
/// and the details section below it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewWell {
    Top,
    Bottom,
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
            scroll_px: 0.0,
            scroll_motion: ScrollMotion::new(),
            focused_well: None,
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

    /// Forget the uploaded texture, freeing it, and say whether there was one.
    ///
    /// For the renderer-replaced path only (see `FilesystemApp::renderer_init`):
    /// the id belongs to a renderer that no longer exists, so this is a drop
    /// rather than a clear — everything else about the shown file stays, and
    /// the caller re-requests the preview to get a live texture back.
    pub fn drop_texture(&mut self) -> bool {
        match self.image_tex.take() {
            Some((id, _, _)) => {
                cce_ui::vk::free_image(id);
                true
            }
            None => false,
        }
    }

    pub fn set_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.rect = (x, y, w, h);
    }

    /// Which well the point lands in, mirroring the paint-time geometry below
    /// (top well = upper half of the pane rect; details well from half + the
    /// plate gap down). `None` when no file is shown — the wells aren't drawn,
    /// so there is nothing to focus.
    pub fn well_at(&self, px: f32, py: f32) -> Option<PreviewWell> {
        if self.path.is_none() {
            return None;
        }
        let (cx, cy, cw, ch) = self.rect;
        if cw <= 0.0 || ch <= 0.0 || px < cx || px > cx + cw {
            return None;
        }
        let half_h = ch * 0.5;
        let details_top = cy + half_h + cce_ui::layout::root_plate_gap();
        if py >= cy && py <= cy + half_h {
            Some(PreviewWell::Top)
        } else if py >= details_top && py <= cy + ch {
            Some(PreviewWell::Bottom)
        } else {
            None
        }
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
        // Inner content-preview region (below the metadata header); the fill
        // spans the full pane width, so the hit region does too.
        let px = prev_x;
        let py = prev_y + 32.0;
        let pw = prev_w;
        let ph = prev_h * 0.5 - 40.0;
        if mx < px || mx > px + pw || my < py || my > py + ph {
            return None;
        }
        Some(self.scroll(delta, prev_h))
    }

    /// Back to the top, motion cancelled (a new file loaded).
    pub fn reset_scroll(&mut self) {
        self.scroll_px = 0.0;
        self.scroll_motion = ScrollMotion::new();
    }

    /// How far the text preview can scroll, in pixels, for a pane `ch` tall:
    /// the lines that don't fit the content well, times the line pitch. Zero
    /// when there is no text or it all fits.
    fn max_scroll_px(&self, ch: f32) -> f32 {
        let Some(content) = &self.content_preview else {
            return 0.0;
        };
        let total_lines = content.lines().count();
        let half_h = ch * 0.5;
        let mut max_visible_lines = 0;
        let mut text_y = 44.0;
        while text_y + 14.0 <= half_h - 16.0 {
            max_visible_lines += 1;
            text_y += 15.0;
        }
        total_lines.saturating_sub(max_visible_lines) as f32 * PREVIEW_LINE_H
    }

    fn scroll(&mut self, delta: &MouseScrollDelta, ch: f32) -> bool {
        let max = self.max_scroll_px(ch);
        if max <= 0.0 {
            if self.scroll_px != 0.0 {
                self.reset_scroll();
                return true;
            }
            return false;
        }
        // A notch is three lines (the legacy `scroll_speed`); a pixel delta
        // is pixels, as before (it used to be divided by the line pitch and
        // rounded to whole lines).
        self.scroll_motion.reconcile(0.0, self.scroll_px);
        let moved = self.scroll_motion.apply(delta, (PREVIEW_NOTCH_PX, PREVIEW_NOTCH_PX), Bounds::max(0.0), Bounds::max(max));
        self.scroll_px = self.scroll_motion.y.pos();
        moved
    }

    /// Advance the text preview's wheel glide / flick coast; true while the
    /// offset is moving, so the host keeps frames coming until it settles.
    pub fn tick(&mut self, dt: f32) -> bool {
        self.scroll_motion.reconcile(0.0, self.scroll_px);
        if !self.scroll_motion.is_animating() {
            return false;
        }
        let max = self.max_scroll_px(self.rect.3);
        let moved = self.scroll_motion.tick(dt, Bounds::max(0.0), Bounds::max(max));
        self.scroll_px = self.scroll_motion.y.pos();
        moved || self.scroll_motion.is_animating()
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
        // For `rect_with_radius_corners` on the well fill — the inherent
        // `rect`/`text` methods still win over the trait's by the usual
        // inherent-first rule, so the calls below are unaffected.
        use cce_ui::layout::RenderTarget;
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
        // Under control_relief the section frames are recessed wells carved
        // into the plate (the list's treatment); the SectionContext 1px line
        // frame is the flat fallback. Titles come from SectionContext::new
        // either way. Frame geometry mirrors SectionContext::finish: the well
        // spans top+7 down to content_y + pad + 12.
        let relief = cce_ui::layout::control_relief();
        let radius = cce_ui::layout::list_corner_radius();

        // 1. Top pane: File Preview Section. The section frame spans the FULL
        // pane rect (left = cx - pad cancels SectionContext's inner pad), so
        // the well edge sits at the pane edge like the list across the split —
        // the visible split gap is exactly root_plate_gap on both sides.
        // The well's top edge sits AT the pane top, aligned with the
        // breadcrumb across the split (finish() draws from top+7, so the
        // fallback frame gets top-7 to land on the same edge).
        {
            let mut preview_sec = SectionContext::new(pc, cx - pad, cy - 7.0, cw + 2.0 * pad, "", false, false);
            preview_sec.content_y = cy + half_h - pad - 12.0;
            if !relief {
                preview_sec.finish();
            }
        }
        if relief {
            if self.focused_well == Some(PreviewWell::Top) {
                pc.relief_recessed_focused(cx, cy, cw, half_h, radius);
            } else {
                pc.relief_recessed(cx, cy, cw, half_h, radius);
            }
        }
        let rect_y = cy + 12.0;
        let rect_h = half_h - pad - 24.0;

        // Content fill spans the FULL well RECT, rounded to the well's radius —
        // `RowList::push_prims` verbatim, so the two read as the same material
        // across the split. It used to be a square-cornered rect inset to the
        // content box (cy + 12, half_h - pad - 24), which left the well floor
        // showing plate colour in a shelf ~7px deep at the top and ~12px at the
        // bottom while the list's fill ran edge to edge into its rim. The
        // content box below is unchanged: text and images already start at
        // rect_y, so widening the fill moves nothing but the shelf.
        let bg_color = cce_ui::color::list_bg_color();
        pc.rect_with_radius_corners(bg_color, cx, cy, cw, half_h, radius, (true, true, true, true));

        if let Some((id, img_w, img_h)) = self.image_tex {
            let fitted = fit_rect(
                img_w,
                img_h,
                Rect { x: cx, y: rect_y, width: cw, height: rect_h },
                FitMode::Contain { max_upscale: 4.0 },
            );
            pc.image(id, fitted.x, fitted.y, fitted.width, fitted.height, 1.0);
        } else if let Some(content) = &self.content_preview {
            // The first whole line scrolled past, plus the sub-line remainder
            // the glide is mid-way through: lines slide under the well's top
            // and bottom edges, clipped to the content box so a partial line
            // renders cut rather than popping.
            let first_line = (self.scroll_px / PREVIEW_LINE_H).floor().max(0.0);
            let frac = self.scroll_px - first_line * PREVIEW_LINE_H;
            let clip_top = rect_y;
            let clip_bottom = rect_y + rect_h - 8.0;
            let clip = [cx, clip_top, cx + cw, clip_bottom];
            let mut text_y = rect_y + 12.0 - frac;
            for line in content.lines().skip(first_line as usize) {
                if text_y >= clip_bottom {
                    break;
                }
                // Chars-per-width from one measured glyph (cached) instead of
                // the old magic 6.8 px/char guess.
                let char_w = measure_text_width("M", "monospace", 11.0).max(1.0);
                let limit = (((cw - 24.0) / char_w).floor() as usize).max(20);
                let line_truncated = truncate_tail(line, limit);
                pc.text_with_font_bounded(&line_truncated, cx + 12.0, text_y, 11.0, text_fg, "monospace", clip);
                text_y += PREVIEW_LINE_H;
            }
        } else {
            pc.text("No preview available", cx + 12.0, rect_y + 12.0, 11.0, text_dim);
        }

        // 2. Bottom pane: Details Section. The visible gap between the wells
        // is exactly root_plate_gap — the same separator width as everywhere
        // else on the plate (it was a hardcoded 12+7=19px before).
        let details_top = cy + half_h + cce_ui::layout::root_plate_gap();
        let icon = if self.is_dir { "📁" } else { "📄" };

        let details = [
            ("Path", &self.path_display),
            ("Type", &self.file_type),
            ("Size", &self.size),
            ("Permissions", &self.permissions),
            ("Modified", &self.modified),
        ];

        let details_content_start_y = details_top + pad + 12.0;

        // The well's bottom edge sits AT the pane bottom, aligned with the
        // list across the split (content-sized before; short panes just show
        // empty well below the rows). finish() draws from top+7, so the
        // fallback frame gets top-7 to land on the same edge.
        {
            let mut details_sec = SectionContext::new(pc, cx - pad, details_top - 7.0, cw + 2.0 * pad, "", false, false);
            details_sec.content_y = cy + ch - pad - 12.0;
            if !relief {
                details_sec.finish();
            }
        }
        if relief {
            if self.focused_well == Some(PreviewWell::Bottom) {
                pc.relief_recessed_focused(cx, details_top, cw, (cy + ch) - details_top, radius);
            } else {
                pc.relief_recessed(cx, details_top, cw, (cy + ch) - details_top, radius);
            }
        }

        let header_y = details_content_start_y + 6.0;
        pc.text(icon, cx + 12.0, header_y, 20.0, text_fg);

        // Pixel-measured budgets against the section frame's inner right edge
        // (None-font text renders sans-serif — measure with the same family).
        let frame_right = cx + cw - 4.0 - pad;
        let name_avail = (frame_right - (cx + 42.0) - 8.0).max(40.0);
        let val_avail = (frame_right - (cx + 112.0) - 8.0).max(40.0);

        let name_truncated = truncate_px(&self.name, "sans-serif", 16.0, name_avail, false);
        pc.text(&name_truncated, cx + 42.0, header_y + 4.0, 16.0, text_fg);

        let mut y = details_content_start_y + 36.0;
        for (label, val) in &details {
            pc.text(label, cx + 12.0, y, 12.0, label_fg);
            let val_str = truncate_px(val, "sans-serif", 12.0, val_avail, true);
            pc.text(&val_str, cx + 112.0, y, 12.0, text_dim);
            y += 20.0;
        }

        if !self.target.is_empty() {
            y += 8.0;
            pc.text("Target", cx + 12.0, y, 12.0, label_fg);
            let target_str = truncate_px(&self.target, "sans-serif", 12.0, val_avail, true);
            pc.text(&target_str, cx + 112.0, y, 12.0, text_dim);
        }
    }
}
