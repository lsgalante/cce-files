//! The Space page — a GrandPerspective-style treemap of disk usage.
//!
//! Every file in the scanned subtree is one rectangle whose *area* is its size,
//! nested inside its directory's rectangle. That is the whole idea: the thing
//! eating your disk is the biggest shape on screen, however deep it is buried.
//!
//! Three pieces live here:
//! - [`squarify`], the Bruls/Huizing/van Wijk squarified layout, which keeps
//!   tiles near-square instead of the slivers a naive slice-and-dice produces;
//! - [`SpaceState::relayout`], which walks the scanned tree recursively and
//!   flattens it into a [`Tile`] list, culling anything too small to see;
//! - [`view`], which paints that list into a `PageContent`.
//!
//! The scan itself is `services::scan`, driven from `main.rs` — this module is
//! given a finished tree.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cce_ui::widget::{Adapted, Breadcrumb, PathController};

use crate::pages::PageContent;
use crate::pages::browse::BrowseState;
use crate::services::scan::TreeNode;
use crate::util::format_size;

/// Below this, in either dimension, a tile is too small to read and is not
/// emitted at all — its bytes stay accounted for in the parent's area, which
/// still shows as filled. This is what bounds tile count on a large tree far
/// more effectively than [`MAX_TILES`].
const MIN_TILE: f32 = 3.0;

/// A directory smaller than this is drawn as one aggregate block rather than
/// recursed into: below it the frame and padding would eat the children.
const MIN_RECURSE: f32 = 20.0;

/// Hard ceiling on emitted tiles, so a pathological tree cannot make a frame
/// rebuild unbounded. Reached only when MIN_TILE culling has not already.
const MAX_TILES: usize = 24_000;

/// Inset applied to a directory's rect before laying out its children — the
/// gap that makes nesting legible.
const DIR_PAD: f32 = 1.0;

/// Height reserved at the top of a directory's rect for its name, when the
/// rect is big enough to bother.
const DIR_LABEL_H: f32 = 13.0;

/// Directory rects at least this tall get a name strip.
const DIR_LABEL_MIN: f32 = 46.0;

/// File tiles at least this big get their name drawn inside them.
const FILE_LABEL_MIN_W: f32 = 44.0;
const FILE_LABEL_MIN_H: f32 = 15.0;

/// Rows reserved at the bottom of the pane for the hover/summary readout.
const FOOTER_H: f32 = 18.0;

// ── File-type colors ────────────────────────────────────────────────

/// The category a file's extension puts it in. Area says how big a thing is;
/// hue says what kind of thing it is, which is how you tell "my photo library"
/// from "one enormous VM image" at a glance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Image,
    Video,
    Audio,
    Code,
    Document,
    Archive,
    Binary,
    Other,
}

impl Category {
    pub fn of(name: &str) -> Category {
        // A leading-dot name with no other dot (`.bashrc`) has no extension —
        // splitting on the last dot would otherwise read "bashrc" as one.
        let ext = name
            .rsplit_once('.')
            .filter(|(stem, _)| !stem.is_empty())
            .map(|(_, e)| e.to_ascii_lowercase());
        match ext.as_deref() {
            Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "tiff" | "tif"
                | "svg" | "svgz" | "psd" | "xcf" | "raw" | "cr2" | "nef" | "heic" | "avif") => Category::Image,
            Some("mp4" | "mkv" | "mov" | "avi" | "webm" | "m4v" | "mpg" | "mpeg" | "wmv"
                | "flv" | "ogv") => Category::Video,
            Some("mp3" | "wav" | "flac" | "ogg" | "opus" | "m4a" | "aac" | "wma" | "aiff"
                | "mid" | "midi") => Category::Audio,
            Some("rs" | "c" | "h" | "cpp" | "hpp" | "cc" | "py" | "js" | "ts" | "jsx" | "tsx"
                | "go" | "java" | "kt" | "rb" | "php" | "swift" | "hs" | "ml" | "lua" | "sh"
                | "bash" | "zsh" | "fish" | "vim" | "el" | "scm" | "clj" | "ex" | "erl"
                | "sql" | "html" | "css" | "scss" | "glsl" | "cl" | "wgsl") => Category::Code,
            Some("txt" | "md" | "rst" | "org" | "pdf" | "doc" | "docx" | "odt" | "rtf"
                | "xls" | "xlsx" | "ods" | "csv" | "tsv" | "ppt" | "pptx" | "odp" | "epub"
                | "mobi" | "tex" | "json" | "toml" | "yaml" | "yml" | "xml" | "kdl"
                | "ini" | "conf" | "cfg" | "log") => Category::Document,
            Some("zip" | "tar" | "gz" | "bz2" | "xz" | "zst" | "7z" | "rar" | "tgz" | "txz"
                | "iso" | "img" | "dmg" | "deb" | "rpm" | "pkg" | "apk" | "jar" | "whl") => Category::Archive,
            Some("so" | "a" | "o" | "dll" | "dylib" | "exe" | "bin" | "elf" | "class"
                | "pyc" | "rlib" | "wasm" | "qcow2" | "vdi" | "vmdk") => Category::Binary,
            _ => Category::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Category::Image => "Images",
            Category::Video => "Video",
            Category::Audio => "Audio",
            Category::Code => "Code",
            Category::Document => "Documents",
            Category::Archive => "Archives",
            Category::Binary => "Binaries",
            Category::Other => "Other",
        }
    }

    /// Written as sRGB hex and converted the same way config colors are, so
    /// these sit in the same space as everything else the renderer is handed.
    pub fn color(self) -> [f32; 4] {
        let hex = match self {
            Category::Image => "#4f9fd1",
            Category::Video => "#9b6bd6",
            Category::Audio => "#4fb98a",
            Category::Code => "#dfb341",
            Category::Document => "#d1685f",
            Category::Archive => "#c77e3e",
            Category::Binary => "#b85c9e",
            Category::Other => "#6b7280",
        };
        cce_ui::color::parse_hex_rgba_linear(hex).unwrap_or([0.4, 0.4, 0.45, 1.0])
    }
}

/// Every category, for the legend.
pub const CATEGORIES: [Category; 8] = [
    Category::Image,
    Category::Video,
    Category::Audio,
    Category::Code,
    Category::Document,
    Category::Archive,
    Category::Binary,
    Category::Other,
];

// ── Tiles ───────────────────────────────────────────────────────────

/// One laid-out rectangle. Directories come before their children in the list,
/// so a reverse scan finds the deepest tile under a point first.
#[derive(Debug, Clone)]
pub struct Tile {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
    pub depth: u32,
    /// (x, y, w, h) in window coordinates.
    pub rect: (f32, f32, f32, f32),
}

impl Tile {
    fn contains(&self, x: f32, y: f32) -> bool {
        let (rx, ry, rw, rh) = self.rect;
        x >= rx && x < rx + rw && y >= ry && y < ry + rh
    }
}

// ── Squarified layout ───────────────────────────────────────────────

/// Lay `values` (which MUST be sorted descending and strictly positive) into
/// `rect`, returning one rect per value in the same order.
///
/// This is the squarified treemap of Bruls, Huizing &amp; van Wijk (2000): fill
/// the rect row by row along its shorter side, growing each row only while
/// doing so improves the worst aspect ratio in it. The result is tiles close
/// to square, which is what makes areas visually comparable — a naive
/// slice-and-dice gives slivers you cannot compare at all.
fn squarify(values: &[f64], rect: (f32, f32, f32, f32)) -> Vec<(f32, f32, f32, f32)> {
    let mut out = Vec::with_capacity(values.len());
    let (rx, ry, rw, rh) = rect;
    let total: f64 = values.iter().sum();
    if values.is_empty() || total <= 0.0 || rw <= 0.0 || rh <= 0.0 {
        return vec![(0.0, 0.0, 0.0, 0.0); values.len()];
    }

    // Work in pixel² so a row's thickness is just its area over its length.
    let scale = (rw as f64) * (rh as f64) / total;
    let areas: Vec<f64> = values.iter().map(|v| v * scale).collect();

    let (mut x, mut y, mut w, mut h) = (rx as f64, ry as f64, rw as f64, rh as f64);
    let mut i = 0;

    while i < areas.len() {
        if w <= 0.0 || h <= 0.0 {
            out.extend(std::iter::repeat((0.0, 0.0, 0.0, 0.0)).take(areas.len() - i));
            break;
        }

        // Rows run along the shorter side; that is the whole trick.
        let horizontal = w >= h;
        let side = if horizontal { h } else { w };

        // Grow the row while the worst aspect ratio in it keeps improving.
        let mut end = i;
        let mut sum = 0.0;
        let mut best = f64::INFINITY;
        while end < areas.len() {
            let next_sum = sum + areas[end];
            // Descending order means the row's max is its first item and its
            // min is the one we are considering adding.
            let worst = worst_ratio(next_sum, areas[i], areas[end], side);
            if end > i && worst > best {
                break;
            }
            best = worst;
            sum = next_sum;
            end += 1;
        }

        // Place the row.
        let thickness = (sum / side).min(if horizontal { w } else { h });
        let mut offset = 0.0;
        for k in i..end {
            let len = if sum > 0.0 { areas[k] / sum * side } else { 0.0 };
            let r = if horizontal {
                (x, y + offset, thickness, len)
            } else {
                (x + offset, y, len, thickness)
            };
            out.push((r.0 as f32, r.1 as f32, r.2 as f32, r.3 as f32));
            offset += len;
        }

        if horizontal {
            x += thickness;
            w -= thickness;
        } else {
            y += thickness;
            h -= thickness;
        }
        i = end;
    }

    out
}

/// Worst (largest) aspect ratio produced by a row of total area `sum` laid
/// along a side of length `side`, containing items of area `max` and `min`.
fn worst_ratio(sum: f64, max: f64, min: f64, side: f64) -> f64 {
    if sum <= 0.0 || side <= 0.0 || min <= 0.0 {
        return f64::INFINITY;
    }
    let s2 = sum * sum;
    let w2 = side * side;
    (w2 * max / s2).max(s2 / (w2 * min))
}

// ── Messages ────────────────────────────────────────────────────────

/// Results coming back from a `FsRequest::ScanTree`. Each carries the
/// directory it is about, because a scan that has been superseded can still
/// deliver messages after the app has moved on.
#[derive(Debug, Clone)]
pub enum SpaceMessage {
    Progress { dir: PathBuf, files: u64, bytes: u64 },
    Scanned { dir: PathBuf, tree: TreeNode },
    Failed(String),
}

pub fn update(state: &mut SpaceState, msg: SpaceMessage) {
    match msg {
        SpaceMessage::Progress { dir, files, bytes } => {
            // Late progress from a scan we no longer care about.
            if !state.scanning || state.scanned_dir != dir {
                return;
            }
            state.scan_files = files;
            state.scan_bytes = bytes;
        }
        SpaceMessage::Scanned { dir, tree } => {
            if state.scanned_dir != dir {
                return;
            }
            state.scan_finished(dir, tree);
        }
        SpaceMessage::Failed(err) => state.scan_failed(err),
    }
}

// ── Page state ──────────────────────────────────────────────────────

pub struct SpaceState {
    pub breadcrumb: Adapted<Breadcrumb>,
    /// The directory the current `tree` describes. Empty until a scan lands.
    pub scanned_dir: PathBuf,
    pub tree: Option<TreeNode>,
    pub tiles: Vec<Tile>,
    /// Rect the current `tiles` were laid out for — a resize invalidates them.
    laid_out: (f32, f32, f32, f32),
    /// The map region as of the last `view`. Input handlers need it to tell a
    /// press on the treemap from one on the window backplate behind it.
    pub map_rect: (f32, f32, f32, f32),
    pub hovered: Option<usize>,
    /// Selection is held by path, not index: a relayout renumbers every tile.
    pub selected_path: Option<PathBuf>,
    pub scanning: bool,
    pub scan_files: u64,
    pub scan_bytes: u64,
    /// Raised to abandon the in-flight scan when a newer one supersedes it.
    pub cancel: Arc<AtomicBool>,
    pub error: Option<String>,
}

impl Default for SpaceState {
    fn default() -> Self {
        let mut breadcrumb = Breadcrumb::new();
        breadcrumb.set_network_opacity(0.95);
        Self {
            breadcrumb,
            scanned_dir: PathBuf::new(),
            tree: None,
            tiles: Vec::new(),
            laid_out: (0.0, 0.0, 0.0, 0.0),
            map_rect: (0.0, 0.0, 0.0, 0.0),
            hovered: None,
            selected_path: None,
            scanning: false,
            scan_files: 0,
            scan_bytes: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            error: None,
        }
    }
}

impl SpaceState {
    /// True when `dir` is not what the current tree describes — the caller
    /// should kick off a scan.
    pub fn needs_scan(&self, dir: &Path) -> bool {
        !self.scanning && (self.tree.is_none() || self.scanned_dir != dir)
    }

    /// Abandon any in-flight scan and arm a fresh cancel token for the next
    /// one. Returns the token the new scan should carry.
    pub fn begin_scan(&mut self, dir: &Path) -> Arc<AtomicBool> {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.scanning = true;
        self.scan_files = 0;
        self.scan_bytes = 0;
        self.error = None;
        self.tree = None;
        self.tiles.clear();
        self.hovered = None;
        self.scanned_dir = dir.to_path_buf();
        self.laid_out = (0.0, 0.0, 0.0, 0.0);
        self.cancel.clone()
    }

    pub fn scan_finished(&mut self, dir: PathBuf, tree: TreeNode) {
        self.scanning = false;
        self.scanned_dir = dir;
        self.tree = Some(tree);
        self.tiles.clear();
        self.laid_out = (0.0, 0.0, 0.0, 0.0);
        self.hovered = None;
    }

    pub fn scan_failed(&mut self, err: String) {
        self.scanning = false;
        self.tree = None;
        self.tiles.clear();
        self.error = Some(err);
    }

    /// The deepest tile under the cursor, which is the one the user means.
    pub fn tile_at(&self, x: f32, y: f32) -> Option<usize> {
        self.tiles.iter().rposition(|t| t.contains(x, y))
    }

    /// Recompute tiles for `rect` if the tree or the rect has changed.
    pub fn relayout(&mut self, rect: (f32, f32, f32, f32)) {
        if self.laid_out == rect && !self.tiles.is_empty() {
            return;
        }
        self.tiles.clear();
        self.laid_out = rect;
        let Some(tree) = self.tree.take() else { return };
        let root = self.scanned_dir.clone();
        place(&tree, &root, rect, 0, &mut self.tiles);
        self.tree = Some(tree);
        // A relayout renumbers everything; the stale hover index would point
        // at an unrelated tile.
        self.hovered = None;
    }
}

/// Recursively lay `node` into `rect`, appending tiles. The node's own tile is
/// pushed before its children so a reverse hit-test finds the deepest first.
fn place(node: &TreeNode, path: &Path, rect: (f32, f32, f32, f32), depth: u32, out: &mut Vec<Tile>) {
    let (x, y, w, h) = rect;
    if w < MIN_TILE || h < MIN_TILE || out.len() >= MAX_TILES {
        return;
    }

    out.push(Tile {
        path: path.to_path_buf(),
        name: node.name.clone(),
        size: node.size,
        is_dir: node.is_dir,
        depth,
        rect,
    });

    if !node.is_dir || node.children.is_empty() {
        return;
    }
    // Too small to subdivide usefully: it stays one aggregate block.
    if w < MIN_RECURSE || h < MIN_RECURSE {
        return;
    }

    // Inset for the frame, plus a name strip when there is room for one.
    let label = h >= DIR_LABEL_MIN && w >= FILE_LABEL_MIN_W;
    let top = DIR_PAD + if label { DIR_LABEL_H } else { 0.0 };
    let inner = (
        x + DIR_PAD,
        y + top,
        (w - DIR_PAD * 2.0).max(0.0),
        (h - top - DIR_PAD).max(0.0),
    );
    if inner.2 < MIN_TILE || inner.3 < MIN_TILE {
        return;
    }

    // Zero-byte children have no area to occupy and would divide by zero in
    // the aspect-ratio test; they are simply not drawn.
    let kids: Vec<&TreeNode> = node.children.iter().filter(|c| c.size > 0).collect();
    if kids.is_empty() {
        return;
    }
    let values: Vec<f64> = kids.iter().map(|c| c.size as f64).collect();

    for (child, r) in kids.iter().zip(squarify(&values, inner)) {
        place(child, &path.join(&child.name), r, depth + 1, out);
    }
}

// ── View ────────────────────────────────────────────────────────────

pub fn view(
    state: &mut SpaceState,
    browse: &BrowseState,
    view_dropdown: &mut Adapted<cce_ui::widget::Dropdown>,
    cx: f32,
    cy: f32,
    cw: f32,
    ch: f32,
    ctx: &mut cce_ui::context::UiContext,
) -> PageContent {
    let mut pc = PageContent::new();

    // Breadcrumb + view dropdown, mirroring the Network page's header so the
    // two views line up when you switch between them.
    let dropdown_w = 120.0;
    let breadcrumb_w = cw - 16.0 - dropdown_w - 12.0;
    cce_ui::layout::render_widget(&mut pc, &mut state.breadcrumb, cx + 4.0, cy + 6.0, breadcrumb_w, 24.0, ctx);
    {
        let rect = cce_ui::scene::layout::Rect { x: cx + 4.0, y: cy + 6.0, width: breadcrumb_w, height: 24.0 };
        crate::pages::breadcrumb_relief(&mut pc, &state.breadcrumb, rect);
    }
    cce_ui::layout::render_widget(&mut pc, view_dropdown, cx + 4.0 + breadcrumb_w + 12.0, cy + 6.0, dropdown_w, 24.0, ctx);

    let mut segments = Vec::new();
    for component in browse.current_dir.components() {
        let s = component.as_os_str().to_string_lossy().to_string();
        if s != "/" && !s.is_empty() {
            segments.push(s);
        }
    }
    state.breadcrumb.set_path(&segments);

    // The map occupies everything below the header, less the footer readout.
    let map = (cx, cy + 34.0, cw, (ch - 34.0 - FOOTER_H).max(0.0));
    state.map_rect = map;
    // Fill and well share one rect AND one radius — `RowList::push_prims`'s
    // pairing, since this pane is the Browse list's opposite number across the
    // same split. It used to fill square (`pc.rect`) under a well carved at
    // `plate_corner_radius` (12.0), so the corners disagreed twice over: with
    // their own fill, and with the r=4 list the pane sits beside.
    //
    // The tiles stay square and unclipped — a treemap cannot follow a curve —
    // so a corner still contradicts the rim. Dropping 12.0 to 4.0 shrinks that
    // residual to what RowList already lives with for its square row overlays.
    // Rounding the fill alone would have been inert: the root directory tile
    // paints a full square rect over the whole map, so the fill is not visible
    // except in the 1px inset.
    let bg = cce_ui::color::list_bg_color();
    let radius = cce_ui::layout::list_corner_radius();
    {
        use cce_ui::layout::RenderTarget;
        pc.rect_with_radius_corners(bg, map.0, map.1, map.2, map.3, radius, (true, true, true, true));
    }
    pc.relief_recessed(map.0, map.1, map.2, map.3, radius);

    let text_dim = cce_ui::color::TEXT_DIM;
    let text_fg = cce_ui::color::TEXT_FG;

    if let Some(err) = &state.error {
        pc.text(err, map.0 + 12.0, map.1 + 12.0, 11.0, text_dim);
        return pc;
    }

    if state.scanning {
        let msg = format!(
            "Scanning {} — {} files, {}",
            browse.current_dir.display(),
            state.scan_files,
            format_size(state.scan_bytes)
        );
        pc.text(&msg, map.0 + 12.0, map.1 + 12.0, 11.0, text_dim);
        return pc;
    }

    // Inset one pixel so tiles do not sit on top of the well's rim.
    state.relayout((map.0 + 1.0, map.1 + 1.0, (map.2 - 2.0).max(0.0), (map.3 - 2.0).max(0.0)));

    if state.tiles.is_empty() {
        pc.text("Nothing to show — the directory is empty.", map.0 + 12.0, map.1 + 12.0, 11.0, text_dim);
        return pc;
    }

    let frame = cce_ui::color::parse_hex_rgba_linear("#20242b").unwrap_or([0.1, 0.1, 0.12, 1.0]);
    for tile in &state.tiles {
        let (tx, ty, tw, th) = tile.rect;
        if tile.is_dir {
            // A directory paints only its frame — its children cover the
            // inside, and where they do not, the gap reads as slack space.
            pc.rect(frame, tx, ty, tw, th);
            if th >= DIR_LABEL_MIN && tw >= FILE_LABEL_MIN_W {
                pc.text(
                    &elide(&tile.name, tw - 6.0),
                    tx + 3.0,
                    ty + 2.0,
                    10.0,
                    text_dim,
                );
            }
        } else {
            pc.rect(Category::of(&tile.name).color(), tx, ty, tw, th);
            if tw >= FILE_LABEL_MIN_W && th >= FILE_LABEL_MIN_H {
                pc.text(&elide(&tile.name, tw - 6.0), tx + 3.0, ty + 2.0, 10.0, text_fg);
            }
        }
    }

    // Selection and hover are drawn as outlines over the tiles.
    if let Some(sel) = &state.selected_path {
        if let Some(t) = state.tiles.iter().find(|t| &t.path == sel) {
            outline(&mut pc, t.rect, cce_ui::color::TEXT_HEADER, 2.0);
        }
    }
    if let Some(idx) = state.hovered {
        if let Some(t) = state.tiles.get(idx) {
            outline(&mut pc, t.rect, cce_ui::color::TEXT_ACCENT, 1.0);
        }
    }

    // Footer: whatever the cursor is over, else the total.
    let footer_y = map.1 + map.3 + 3.0;
    let footer = match state.hovered.and_then(|i| state.tiles.get(i)) {
        Some(t) => format!("{}  —  {}", t.path.display(), format_size(t.size)),
        None => {
            let total = state.tree.as_ref().map(|t| t.size).unwrap_or(0);
            format!("{} in {} tiles", format_size(total), state.tiles.len())
        }
    };
    pc.text(&elide(&footer, cw - 16.0), cx + 8.0, footer_y, 10.0, text_dim);

    pc
}

/// Four thin rects making a border — `PageContent` has no stroke primitive.
fn outline(pc: &mut PageContent, rect: (f32, f32, f32, f32), color: [f32; 4], t: f32) {
    let (x, y, w, h) = rect;
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let t = t.min(w / 2.0).min(h / 2.0);
    pc.rect(color, x, y, w, t);
    pc.rect(color, x, y + h - t, w, t);
    pc.rect(color, x, y + t, t, h - t * 2.0);
    pc.rect(color, x + w - t, y + t, t, h - t * 2.0);
}

/// Trim to what fits in `width` px at the ~10px tile font. An estimate, not a
/// shaping pass — labels here are decoration over an exact rectangle, and
/// running cosmic-text over thousands of tiles per rebuild would not pay.
fn elide(s: &str, width: f32) -> String {
    const CHAR_W: f32 = 5.2;
    let max = (width / CHAR_W).floor().max(0.0) as usize;
    if max == 0 {
        return String::new();
    }
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max <= 1 {
        return "…".to_string();
    }
    s.chars().take(max - 1).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(r: (f32, f32, f32, f32)) -> f32 {
        r.2 * r.3
    }

    #[test]
    fn squarify_covers_the_rect_exactly_once() {
        let values = vec![600.0, 300.0, 100.0, 50.0, 25.0, 25.0];
        let rect = (10.0, 20.0, 400.0, 300.0);
        let out = squarify(&values, rect);

        assert_eq!(out.len(), values.len());
        let total_area: f32 = out.iter().map(|r| area(*r)).sum();
        assert!(
            (total_area - area(rect)).abs() < 1.0,
            "tiles should tile the rect: {total_area} vs {}",
            area(rect)
        );

        // Every tile stays inside the rect.
        for r in &out {
            assert!(r.0 >= rect.0 - 0.01 && r.1 >= rect.1 - 0.01, "{r:?}");
            assert!(r.0 + r.2 <= rect.0 + rect.2 + 0.01, "{r:?}");
            assert!(r.1 + r.3 <= rect.1 + rect.3 + 0.01, "{r:?}");
        }
    }

    #[test]
    fn squarify_areas_are_proportional_to_values() {
        let values = vec![500.0, 250.0, 250.0];
        let rect = (0.0, 0.0, 200.0, 100.0);
        let out = squarify(&values, rect);

        let total = area(rect);
        assert!((area(out[0]) - total * 0.5).abs() < 1.0);
        assert!((area(out[1]) - total * 0.25).abs() < 1.0);
        assert!((area(out[2]) - total * 0.25).abs() < 1.0);
    }

    #[test]
    fn squarify_keeps_tiles_roughly_square() {
        // 64 equal values in a square: a slice-and-dice layout would give
        // 64 slivers of aspect 64:1. Squarified should stay near 1:1.
        let values = vec![1.0; 64];
        let out = squarify(&values, (0.0, 0.0, 400.0, 400.0));
        for r in &out {
            let aspect = (r.2 / r.3).max(r.3 / r.2);
            assert!(aspect < 2.0, "tile too elongated: {r:?} aspect {aspect}");
        }
    }

    #[test]
    fn squarify_handles_degenerate_input() {
        assert!(squarify(&[], (0.0, 0.0, 10.0, 10.0)).is_empty());
        // A zero-area rect still returns one entry per value.
        assert_eq!(squarify(&[1.0, 2.0], (0.0, 0.0, 0.0, 10.0)).len(), 2);
        assert_eq!(squarify(&[0.0, 0.0], (0.0, 0.0, 10.0, 10.0)).len(), 2);
    }

    fn file(name: &str, size: u64) -> TreeNode {
        TreeNode { name: name.into(), size, is_dir: false, children: Vec::new() }
    }

    #[test]
    fn place_nests_children_inside_their_directory() {
        let tree = TreeNode {
            name: "root".into(),
            size: 1000,
            is_dir: true,
            children: vec![
                TreeNode {
                    name: "sub".into(),
                    size: 800,
                    is_dir: true,
                    children: vec![file("big.bin", 800)],
                },
                file("small.txt", 200),
            ],
        };

        let mut tiles = Vec::new();
        place(&tree, Path::new("/root"), (0.0, 0.0, 400.0, 400.0), 0, &mut tiles);

        // Root, sub, big.bin, small.txt.
        assert_eq!(tiles.len(), 4);
        assert_eq!(tiles[0].name, "root");
        assert_eq!(tiles[0].depth, 0);

        // Paths are rebuilt from the names on the way down.
        let big = tiles.iter().find(|t| t.name == "big.bin").unwrap();
        assert_eq!(big.path, PathBuf::from("/root/sub/big.bin"));
        assert_eq!(big.depth, 2);

        // The child sits strictly inside its parent.
        let sub = tiles.iter().find(|t| t.name == "sub").unwrap();
        assert!(big.rect.0 >= sub.rect.0 && big.rect.1 >= sub.rect.1);
        assert!(big.rect.0 + big.rect.2 <= sub.rect.0 + sub.rect.2 + 0.01);
        assert!(big.rect.1 + big.rect.3 <= sub.rect.1 + sub.rect.3 + 0.01);
    }

    #[test]
    fn place_culls_tiles_below_the_minimum() {
        // One huge file and a thousand tiny ones in a small rect: the tiny
        // ones fall under MIN_TILE and are dropped rather than emitted as
        // sub-pixel slivers.
        let mut children = vec![file("huge.bin", 10_000_000)];
        for i in 0..1000 {
            children.push(file(&format!("tiny{i}"), 1));
        }
        let tree = TreeNode { name: "root".into(), size: 10_001_000, is_dir: true, children };

        let mut tiles = Vec::new();
        place(&tree, Path::new("/root"), (0.0, 0.0, 100.0, 100.0), 0, &mut tiles);

        assert!(tiles.len() < 50, "expected culling, got {} tiles", tiles.len());
        assert!(tiles.iter().any(|t| t.name == "huge.bin"));
    }

    #[test]
    fn hit_test_finds_the_deepest_tile() {
        let tree = TreeNode {
            name: "root".into(),
            size: 1000,
            is_dir: true,
            children: vec![TreeNode {
                name: "sub".into(),
                size: 1000,
                is_dir: true,
                children: vec![file("leaf.bin", 1000)],
            }],
        };
        let mut state = SpaceState::default();
        state.scanned_dir = PathBuf::from("/root");
        state.tree = Some(tree);
        state.relayout((0.0, 0.0, 400.0, 400.0));

        // Dead centre is inside root, sub, and leaf — the leaf must win.
        let hit = state.tile_at(200.0, 200.0).unwrap();
        assert_eq!(state.tiles[hit].name, "leaf.bin");

        assert!(state.tile_at(-5.0, 200.0).is_none());
    }

    #[test]
    fn category_maps_extensions() {
        assert_eq!(Category::of("photo.JPG"), Category::Image);
        assert_eq!(Category::of("main.rs"), Category::Code);
        assert_eq!(Category::of("disk.qcow2"), Category::Binary);
        assert_eq!(Category::of("notes.md"), Category::Document);
        assert_eq!(Category::of("bundle.tar.gz"), Category::Archive);
        // No extension at all; and a dotfile, whose "extension" is its name.
        assert_eq!(Category::of("README"), Category::Other);
        assert_eq!(Category::of(".bashrc"), Category::Other);
        // A dotfile that really does carry one is still classified.
        assert_eq!(Category::of(".config.toml"), Category::Document);
    }

    #[test]
    fn elide_respects_width() {
        assert_eq!(elide("hi", 0.0), "");
        assert_eq!(elide("short", 200.0), "short");
        let long = elide("a-very-long-file-name.txt", 30.0);
        assert!(long.ends_with('…'));
        assert!(long.chars().count() <= 6);
    }
}
