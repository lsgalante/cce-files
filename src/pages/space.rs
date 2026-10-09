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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cce_ui::context::UiContext;
use cce_ui::widget::Handle;
use cce_ui::widget::{Adapted, Breadcrumb, PathController};

use crate::pages::PageContent;
use crate::pages::browse::BrowseState;
use crate::services::scan::TreeNode;
use crate::util::{format_size, truncate_px};

/// Below this, in either dimension, a tile is too small to read and is not
/// emitted at all — its bytes stay accounted for in the parent's area, which
/// still shows as filled. This is what bounds tile count on a large tree far
/// more effectively than [`MAX_TILES`].
const MIN_TILE: f32 = 3.0;

/// A directory smaller than this is drawn as one aggregate block rather than
/// recursed into: below it the frame and padding would eat the children.
const MIN_RECURSE: f32 = 20.0;

/// A directory's entries smaller than this many px² are not laid out one by
/// one but lumped into a single "N smaller items" block. Each alone would be
/// culled under `MIN_TILE`, and a folder of thousands of them left a dark
/// hole as big as all of them together — read, like an unfilled block, as
/// empty space. Twice `MIN_TILE`² catches what squarify would cut to slivers.
const REST_AREA: f32 = MIN_TILE * MIN_TILE * 2.0;

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

/// Rows reserved at the bottom of the pane: the hover/summary readout, then
/// the colour legend under it.
const FOOTER_H: f32 = 34.0;
const FOOTER_LINE: f32 = 15.0;
const FOOTER_FONT: f32 = 10.0;
/// The face `pc.text` falls back to, which the footer measures in — the
/// preview pane's convention.
const FOOTER_FAMILY: &str = "sans-serif";

/// A legend swatch's side, its gap to its label, and the gap between entries.
const SWATCH: f32 = 8.0;
const SWATCH_GAP: f32 = 4.0;
const LEGEND_GAP: f32 = 14.0;

/// The dark seam left between neighbouring file tiles, half off each side.
/// Without it, files of one kind side by side fused into one slab: ten
/// films read as one, three thousand object files as a single file. A tile
/// narrower than `FILE_GAP_MIN` keeps its full width, since the gap would eat it.
const FILE_GAP: f32 = 1.0;
const FILE_GAP_MIN: f32 = 4.0;

/// Opacity of the frame around the hovered tile's top-level folder: there
/// to be found, not to compete with the hover outline itself.
const TOP_FOLDER_ALPHA: f32 = 0.45;

/// How far a directory drawn as one block is pulled from its kind's colour
/// toward the frame: dim enough that it never reads as one big file of that
/// kind, bright enough that it never reads as empty.
const AGGREGATE_DIM: f32 = 0.6;

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

    /// Position in [`CATEGORIES`], which lists them in declaration order.
    pub fn index(self) -> usize {
        self as usize
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

/// What a subtree holds, by kind. The root's is the legend; every
/// directory's names the colour it wears when drawn as one block.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Breakdown {
    /// Bytes per [`Category`], indexed by [`Category::index`].
    pub bytes: [u64; 8],
    pub files: u64,
}

impl Breakdown {
    /// The kind holding the most bytes; `None` when nothing has any.
    pub fn dominant(&self) -> Option<Category> {
        let mut best = None;
        let mut most = 0;
        for c in CATEGORIES {
            if self.bytes[c.index()] > most {
                most = self.bytes[c.index()];
                best = Some(c);
            }
        }
        best
    }

    /// The kinds present, largest first — the legend's order.
    pub fn legend(&self) -> Vec<(Category, u64)> {
        let mut out: Vec<(Category, u64)> = CATEGORIES
            .into_iter()
            .map(|c| (c, self.bytes[c.index()]))
            .filter(|&(_, b)| b > 0)
            .collect();
        out.sort_by(|a, b| b.1.cmp(&a.1));
        out
    }
}

/// Sum `node` by kind, recording every directory's dominant kind under its
/// path along the way. One pass when a scan lands, so a relayout (every frame
/// of a resize) only looks a block's colour up.
fn tally(node: &TreeNode, path: &Path, dominant: &mut HashMap<PathBuf, Category>) -> Breakdown {
    let mut b = Breakdown::default();
    if !node.is_dir {
        b.bytes[Category::of(&node.name).index()] += node.size;
        b.files = 1;
        return b;
    }
    for child in &node.children {
        if child.is_dir {
            let sub = tally(child, &path.join(&child.name), dominant);
            for (acc, n) in b.bytes.iter_mut().zip(sub.bytes) {
                *acc += n;
            }
            b.files += sub.files;
        } else {
            b.bytes[Category::of(&child.name).index()] += child.size;
            b.files += 1;
        }
    }
    if let Some(c) = b.dominant() {
        dominant.insert(path.to_path_buf(), c);
    }
    b
}

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
    /// Set on a directory drawn as one block — too small to open, or nothing
    /// inside it big enough to place: the kind that fills most of it. Painted
    /// in the frame colour, such a block read as empty space however full it was.
    pub aggregate: Option<Category>,
    /// Nonzero on the block standing for that many of a directory's smallest
    /// entries (see [`REST_AREA`]). It carries the directory's own path, so a
    /// click selects and a double-click opens the directory they are in.
    pub rest: usize,
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
    pub breadcrumb: Handle<Adapted<Breadcrumb>>,
    /// The directory the current `tree` describes. Empty until a scan lands.
    pub scanned_dir: PathBuf,
    pub tree: Option<TreeNode>,
    pub tiles: Vec<Tile>,
    /// Rect the current `tiles` were laid out for — a resize invalidates them.
    laid_out: (f32, f32, f32, f32),
    /// The map region as of the last `view`. Input handlers need it to tell a
    /// press on the treemap from one on the window root plate behind it.
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
    /// The scanned tree by kind: the legend, and the file count.
    pub breakdown: Breakdown,
    /// Every directory's dominant kind, by path — what a directory drawn as
    /// one block is coloured by. Filled with `breakdown`, once per scan.
    dominant: HashMap<PathBuf, Category>,
    /// Pointer focus (the app's well-focus tracking): the map well renders as
    /// the tinted carve — accent ring replacing the relief lighting.
    pub focused: bool,
}

impl SpaceState {
    /// The page's state, its breadcrumb inserted into `ctx`.
    pub fn new(ctx: &mut UiContext) -> Self {
        let mut breadcrumb = Breadcrumb::new();
        breadcrumb.set_network_opacity(0.95);
        Self {
            breadcrumb: ctx.insert(breadcrumb),
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
            breakdown: Breakdown::default(),
            dominant: HashMap::new(),
            focused: false,
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
        self.breakdown = Breakdown::default();
        self.dominant.clear();
        self.hovered = None;
        self.scanned_dir = dir.to_path_buf();
        self.laid_out = (0.0, 0.0, 0.0, 0.0);
        self.cancel.clone()
    }

    pub fn scan_finished(&mut self, dir: PathBuf, tree: TreeNode) {
        self.scanning = false;
        self.dominant.clear();
        self.breakdown = tally(&tree, &dir, &mut self.dominant);
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
        self.breakdown = Breakdown::default();
        self.dominant.clear();
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
        place(&tree, &root, rect, 0, &self.dominant, &mut self.tiles);
        self.tree = Some(tree);
        // A relayout renumbers everything; the stale hover index would point
        // at an unrelated tile.
        self.hovered = None;
    }
}

/// Recursively lay `node` into `rect`, appending tiles. The node's own tile is
/// pushed before its children so a reverse hit-test finds the deepest first.
/// A directory none of whose children got a tile is marked an aggregate, in
/// the colour `dominant` gives its path.
fn place(
    node: &TreeNode,
    path: &Path,
    rect: (f32, f32, f32, f32),
    depth: u32,
    dominant: &HashMap<PathBuf, Category>,
    out: &mut Vec<Tile>,
) {
    let (w, h) = (rect.2, rect.3);
    if w < MIN_TILE || h < MIN_TILE || out.len() >= MAX_TILES {
        return;
    }

    let idx = out.len();
    out.push(Tile {
        path: path.to_path_buf(),
        name: node.name.clone(),
        size: node.size,
        is_dir: node.is_dir,
        depth,
        rect,
        aggregate: None,
        rest: 0,
    });

    if !node.is_dir {
        return;
    }
    place_children(node, path, rect, depth, dominant, out);
    if out.len() == idx + 1 {
        out[idx].aggregate = dominant.get(path).copied();
    }
}

/// Lay a directory's children inside its frame, when it is big enough to.
fn place_children(
    node: &TreeNode,
    path: &Path,
    rect: (f32, f32, f32, f32),
    depth: u32,
    dominant: &HashMap<PathBuf, Category>,
    out: &mut Vec<Tile>,
) {
    let (x, y, w, h) = rect;
    if node.children.is_empty() {
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

    // Children are sorted largest first, so the ones under REST_AREA are a
    // tail. Two or more of them become one block; a lone one is laid out as
    // itself and stands or falls by MIN_TILE.
    let total: f64 = kids.iter().map(|c| c.size as f64).sum();
    let px_per_byte = (inner.2 * inner.3) as f64 / total;
    let mut keep = kids
        .iter()
        .position(|c| (c.size as f64 * px_per_byte) < REST_AREA as f64)
        .unwrap_or(kids.len());
    if kids.len() - keep < 2 {
        keep = kids.len();
    }
    let (shown, rest) = kids.split_at(keep);

    // squarify wants its values descending, and the lump may outweigh some
    // of the entries shown alone, so it takes its place among them.
    let rest_size: u64 = rest.iter().map(|c| c.size).sum();
    let mut slots: Vec<(u64, Option<&TreeNode>)> = shown.iter().map(|c| (c.size, Some(*c))).collect();
    if !rest.is_empty() {
        let at = slots.partition_point(|&(size, _)| size >= rest_size);
        slots.insert(at, (rest_size, None));
    }
    let values: Vec<f64> = slots.iter().map(|&(size, _)| size as f64).collect();

    for (&(_, child), r) in slots.iter().zip(squarify(&values, inner)) {
        match child {
            Some(child) => place(child, &path.join(&child.name), r, depth + 1, dominant, out),
            None => {
                if r.2 < MIN_TILE || r.3 < MIN_TILE || out.len() >= MAX_TILES {
                    continue;
                }
                out.push(Tile {
                    path: path.to_path_buf(),
                    name: format!("{} smaller items", group_digits(rest.len() as u64)),
                    size: rest_size,
                    is_dir: true,
                    depth: depth + 1,
                    rect: r,
                    aggregate: rest_kind(rest, path, dominant),
                    rest: rest.len(),
                });
            }
        }
    }
}

/// What fills a lump of small entries most, by bytes: a file by its own
/// kind, a directory by its dominant one.
fn rest_kind(rest: &[&TreeNode], path: &Path, dominant: &HashMap<PathBuf, Category>) -> Option<Category> {
    let mut b = Breakdown::default();
    for c in rest {
        let kind = if c.is_dir { dominant.get(&path.join(&c.name)).copied() } else { Some(Category::of(&c.name)) };
        if let Some(kind) = kind {
            b.bytes[kind.index()] += c.size;
        }
    }
    b.dominant()
}

// ── View ────────────────────────────────────────────────────────────

pub fn view(
    state: &mut SpaceState,
    browse: &BrowseState,
    cx: f32,
    cy: f32,
    cw: f32,
    ch: f32,
    ctx: &mut cce_ui::context::UiContext,
) -> PageContent {
    let mut pc = PageContent::new();

    let top = crate::pages::breadcrumb_header(&mut pc, state.breadcrumb, cx, cy, cw, ctx);

    let mut segments = Vec::new();
    for component in browse.current_dir.components() {
        let s = component.as_os_str().to_string_lossy().to_string();
        if s != "/" && !s.is_empty() {
            segments.push(s);
        }
    }
    ctx[state.breadcrumb].set_path(&segments);

    // The map occupies everything below the header, less the footer readout.
    let map = (cx, top, cw, (cy + ch - top - FOOTER_H).max(0.0));
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
    if state.focused {
        pc.relief_recessed_focused(map.0, map.1, map.2, map.3, radius);
    } else {
        pc.relief_recessed(map.0, map.1, map.2, map.3, radius);
    }

    let text_dim = cce_ui::color::TEXT_DIM;
    let text_fg = cce_ui::color::TEXT_FG;

    if let Some(err) = &state.error {
        pc.text(err, map.0 + cce_ui::layout::plate_padding(), map.1 + cce_ui::layout::plate_padding(), 11.0, text_dim);
        return pc;
    }

    if state.scanning {
        let msg = format!(
            "Scanning {} — {} files, {}",
            browse.current_dir.display(),
            state.scan_files,
            format_size(state.scan_bytes)
        );
        pc.text(&msg, map.0 + cce_ui::layout::plate_padding(), map.1 + cce_ui::layout::plate_padding(), 11.0, text_dim);
        return pc;
    }

    // Inset one pixel so tiles do not sit on top of the well's rim.
    state.relayout((map.0 + 1.0, map.1 + 1.0, (map.2 - 2.0).max(0.0), (map.3 - 2.0).max(0.0)));

    // A root of zero bytes still gets a tile — an empty, dark map that read as
    // a scan that had not finished.
    if state.tiles.is_empty() || state.tree.as_ref().is_none_or(|t| t.size == 0) {
        pc.text("Nothing to show — nothing here takes up any space.", map.0 + cce_ui::layout::plate_padding(), map.1 + cce_ui::layout::plate_padding(), 11.0, text_dim);
        return pc;
    }

    let frame = cce_ui::color::parse_hex_rgba_linear("#20242b").unwrap_or([0.1, 0.1, 0.12, 1.0]);
    for tile in &state.tiles {
        let (tx, ty, tw, th) = tile.rect;
        if tile.is_dir {
            // A directory paints only its frame — its children cover the
            // inside, and where they do not, the gap reads as slack space.
            pc.rect(frame, tx, ty, tw, th);
            // One drawn as a block has no children over it: fill it, inside a
            // frame-coloured rim, in a dimmed copy of what fills it most.
            // An opened directory's name sits on its frame-coloured strip; a
            // block's sits on its fill.
            let mut label = text_dim;
            if let Some(cat) = tile.aggregate {
                let fill = mix(cat.color(), frame, AGGREGATE_DIM);
                if tw > 2.0 && th > 2.0 {
                    pc.rect(fill, tx + 1.0, ty + 1.0, tw - 2.0, th - 2.0);
                }
                label = label_on(fill, text_fg, frame);
            }
            if th >= DIR_LABEL_MIN && tw >= FILE_LABEL_MIN_W {
                pc.text(&elide(&tile.name, tw - 6.0), tx + 3.0, ty + 2.0, 10.0, label);
            }
        } else {
            let (fx, fy, fw, fh) = file_face(tile.rect);
            let fill = Category::of(&tile.name).color();
            pc.rect(fill, fx, fy, fw, fh);
            if tw >= FILE_LABEL_MIN_W && th >= FILE_LABEL_MIN_H {
                pc.text(&elide(&tile.name, tw - 6.0), tx + 3.0, ty + 2.0, 10.0, label_on(fill, text_fg, frame));
            }
        }
    }

    // Selection and hover are drawn as outlines over the tiles. Under them,
    // fainter and heavier, the top-level folder the hovered tile lies in:
    // deep in the map, "which of my folders is this?" is the first question,
    // and a 1px frame on the dark between tiles did not answer it.
    if let Some(top) = state.hovered.and_then(|i| top_folder(&state.tiles, i)) {
        let [r, g, b, _] = cce_ui::color::TEXT_ACCENT;
        outline(&mut pc, state.tiles[top].rect, [r, g, b, TOP_FOLDER_ALPHA], 2.0);
    }
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

    // Footer, line one: whatever the cursor is over, else the summary. The
    // root tile covers the whole map, so "over the root" (a frame gap) counts
    // as over nothing.
    let left = cx + 8.0;
    let avail = (cw - 16.0).max(0.0);
    let line1 = map.1 + map.3 + 3.0;
    let total = state.tree.as_ref().map_or(0, |t| t.size);
    let hovered = state.hovered.and_then(|i| state.tiles.get(i)).filter(|t| t.depth > 0);
    let readout = match hovered {
        Some(t) => readout(t, &state.scanned_dir, total, avail),
        None => summary(total, state.breakdown.files, avail),
    };
    pc.text(&readout, left, line1, FOOTER_FONT, text_dim);

    // Line two: the legend, largest kind first, as many as fit. The hovered
    // tile's kind is lit, so the key answers "what colour is this?" in place.
    let lit = hovered.and_then(|t| if t.is_dir { t.aggregate } else { Some(Category::of(&t.name)) });
    let line2 = line1 + FOOTER_LINE;
    let mut x = left;
    for (cat, bytes) in state.breakdown.legend() {
        let label = format!("{} {}", cat.label(), format_size(bytes));
        let w = SWATCH + SWATCH_GAP + cce_ui::widget::display::measure_text_width(&label, FOOTER_FAMILY, FOOTER_FONT);
        if x + w > left + avail {
            break;
        }
        pc.rect(cat.color(), x, line2 + (FOOTER_LINE - 2.0 - SWATCH) / 2.0, SWATCH, SWATCH);
        let color = if lit == Some(cat) { text_fg } else { text_dim };
        pc.text(&label, x + SWATCH + SWATCH_GAP, line2, FOOTER_FONT, color);
        x += w + LEGEND_GAP;
    }

    pc
}

/// The top-level folder (depth 1) holding tile `idx`, when it lies deeper
/// than that. Tiles are flattened depth-first, parents first, so the nearest
/// depth-1 tile before it is its ancestor.
fn top_folder(tiles: &[Tile], idx: usize) -> Option<usize> {
    if tiles.get(idx)?.depth < 2 {
        return None;
    }
    tiles[..idx].iter().rposition(|t| t.depth == 1)
}

/// The part of a file tile its colour fills: the tile less half of
/// [`FILE_GAP`] on every side, so the frame behind shows as a seam between
/// neighbours. Each axis on its own, so a sliver keeps its length.
fn file_face(rect: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
    let (x, y, w, h) = rect;
    let (gx, gy) = (
        if w >= FILE_GAP_MIN { FILE_GAP / 2.0 } else { 0.0 },
        if h >= FILE_GAP_MIN { FILE_GAP / 2.0 } else { 0.0 },
    );
    (x + gx, y + gy, w - gx * 2.0, h - gy * 2.0)
}

/// The hovered tile, for the footer: its path under the scanned directory
/// (cut from the FRONT, so the name survives), size, share of the whole, and
/// for a block, what fills it.
fn readout(t: &Tile, root: &Path, total: u64, avail: f32) -> String {
    let mut tail = format!("  —  {}  ·  {}", format_size(t.size), share(t.size, total));
    if t.rest > 0 {
        tail.insert_str(0, &format!("  ·  {}", t.name));
    }
    if let Some(cat) = t.aggregate {
        tail.push_str(&format!("  ·  mostly {}", cat.label().to_lowercase()));
    }
    // A lump directly under the root has the root's own path: no relative
    // part to show, so it names the root.
    let mut rel = match t.path.strip_prefix(root) {
        Ok(r) if r.as_os_str().is_empty() => root.display().to_string(),
        Ok(r) => r.display().to_string(),
        Err(_) => t.path.display().to_string(),
    };
    if t.is_dir && !rel.ends_with('/') {
        rel.push('/');
    }
    let tail_w = cce_ui::widget::display::measure_text_width(&tail, FOOTER_FAMILY, FOOTER_FONT);
    let rel = truncate_px(&rel, FOOTER_FAMILY, FOOTER_FONT, (avail - tail_w).max(0.0), true);
    rel + &tail
}

/// The whole scan, for the footer when nothing is hovered — with the one
/// gesture the map has, dropped first when the line is short.
fn summary(total: u64, files: u64, avail: f32) -> String {
    let noun = if files == 1 { "file" } else { "files" };
    let line = format!("{} in {} {noun}", format_size(total), group_digits(files));
    let hinted = format!("{line}  ·  double-click a block to zoom in");
    if cce_ui::widget::display::measure_text_width(&hinted, FOOTER_FAMILY, FOOTER_FONT) <= avail {
        return hinted;
    }
    truncate_px(&line, FOOTER_FAMILY, FOOTER_FONT, avail, false)
}

/// `part` as a share of `total`: whole percent from 10 up, one decimal below,
/// and a floor so a sliver never reads as "0%".
fn share(part: u64, total: u64) -> String {
    if total == 0 {
        return "—".to_string();
    }
    let p = part as f64 / total as f64 * 100.0;
    if p >= 9.95 {
        format!("{p:.0}%")
    } else if p >= 0.1 {
        format!("{p:.1}%")
    } else {
        "<0.1%".to_string()
    }
}

/// `12408` → `12,408`.
fn group_digits(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Whichever of `light` and `dark` reads better on `bg`, by WCAG contrast
/// ratio. Light text on every tile was 1.6:1 on Code's yellow and under 3:1 on
/// most of the palette; the frame's near-black runs 3.8:1 (Binaries) to 7.9:1
/// (Code), and Other's grey, where light still wins at 3.9:1, keeps it.
fn label_on(bg: [f32; 4], light: [f32; 4], dark: [f32; 4]) -> [f32; 4] {
    let l = luminance(bg);
    let contrast = |c: [f32; 4]| {
        let (a, b) = (luminance(c).max(l), luminance(c).min(l));
        (a + 0.05) / (b + 0.05)
    };
    if contrast(dark) > contrast(light) { dark } else { light }
}

/// Relative luminance of a colour already in linear space.
fn luminance(c: [f32; 4]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// `a` pulled toward `b` by `t` (0 = a, 1 = b). Both linear, so is the mix.
fn mix(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3],
    ]
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
        place(&tree, Path::new("/root"), (0.0, 0.0, 400.0, 400.0), 0, &HashMap::new(), &mut tiles);

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
        place(&tree, Path::new("/root"), (0.0, 0.0, 100.0, 100.0), 0, &HashMap::new(), &mut tiles);

        assert!(tiles.len() < 50, "expected culling, got {} tiles", tiles.len());
        assert!(tiles.iter().any(|t| t.name == "huge.bin"));
    }

    #[test]
    fn hit_test_finds_the_deepest_tile() {
        let mut ui = UiContext::new();
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
        let mut state = SpaceState::new(&mut ui);
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
    fn tally_sums_kinds_and_names_each_directory_by_its_largest() {
        let tree = TreeNode {
            name: "root".into(),
            size: 1300,
            is_dir: true,
            children: vec![
                TreeNode {
                    name: "films".into(),
                    size: 1000,
                    is_dir: true,
                    children: vec![file("a.mkv", 900), file("notes.txt", 100)],
                },
                file("b.png", 200),
                file("c.png", 100),
            ],
        };
        let mut dominant = HashMap::new();
        let b = tally(&tree, Path::new("/root"), &mut dominant);

        assert_eq!(b.files, 4);
        assert_eq!(b.bytes[Category::Video.index()], 900);
        assert_eq!(b.bytes[Category::Image.index()], 300);
        assert_eq!(b.bytes[Category::Document.index()], 100);
        assert_eq!(
            b.legend(),
            vec![(Category::Video, 900), (Category::Image, 300), (Category::Document, 100)],
            "only the kinds present, largest first"
        );
        assert_eq!(dominant.get(Path::new("/root/films")), Some(&Category::Video));
        assert_eq!(dominant.get(Path::new("/root")), Some(&Category::Video));
        assert_eq!(Breakdown::default().dominant(), None);
    }

    #[test]
    fn a_directory_drawn_as_one_block_wears_its_dominant_kind() {
        let mut ui = UiContext::new();
        // `sub` gets far less than MIN_RECURSE across, so it is not opened.
        let mut children = vec![file("huge.bin", 1_000_000)];
        children.push(TreeNode {
            name: "sub".into(),
            size: 20_000,
            is_dir: true,
            children: vec![file("x.mp3", 15_000), file("y.txt", 5_000)],
        });
        let tree = TreeNode { name: "root".into(), size: 1_020_000, is_dir: true, children };
        let mut state = SpaceState::new(&mut ui);
        state.scan_finished(PathBuf::from("/root"), tree);
        state.relayout((0.0, 0.0, 200.0, 200.0));

        let sub = state.tiles.iter().find(|t| t.name == "sub").expect("sub is big enough to show");
        assert_eq!(sub.aggregate, Some(Category::Audio));
        assert!(!state.tiles.iter().any(|t| t.name == "x.mp3"), "its children are not placed");
        // An opened directory, and any file, is not an aggregate.
        assert!(state.tiles.iter().filter(|t| t.name != "sub").all(|t| t.aggregate.is_none()));
    }

    #[test]
    fn a_folders_smallest_entries_become_one_block() {
        // Ten thousand 1-byte files beside one big one, in 200 x 200: each
        // alone is far under REST_AREA, together they are a tenth of the map.
        let mut children = vec![file("huge.bin", 90_000)];
        children.extend((0..10_000).map(|i| file(&format!("t{i}.png"), 1)));
        let tree = TreeNode { name: "root".into(), size: 100_000, is_dir: true, children };
        let mut tiles = Vec::new();
        place(&tree, Path::new("/root"), (0.0, 0.0, 200.0, 200.0), 0, &HashMap::new(), &mut tiles);

        assert_eq!(tiles.len(), 3, "root, huge.bin, and one lump: {:?}", tiles.iter().map(|t| &t.name).collect::<Vec<_>>());
        let lump = &tiles[2];
        assert_eq!(lump.rest, 10_000);
        assert_eq!(lump.name, "10,000 smaller items");
        assert_eq!(lump.size, 10_000);
        assert_eq!(lump.path, PathBuf::from("/root"), "it opens the folder it is in");
        assert!(lump.is_dir);
        assert_eq!(lump.aggregate, Some(Category::Image));
        // It gets the area of what it stands for, about a tenth of the map.
        let share = lump.rect.2 * lump.rect.3 / (198.0 * 198.0);
        assert!((share - 0.1).abs() < 0.02, "{share}");
    }

    #[test]
    fn a_single_small_entry_is_not_lumped() {
        let tree = TreeNode {
            name: "root".into(),
            size: 1_000_001,
            is_dir: true,
            children: vec![file("huge.bin", 1_000_000), file("tiny.txt", 1)],
        };
        let mut tiles = Vec::new();
        place(&tree, Path::new("/root"), (0.0, 0.0, 200.0, 200.0), 0, &HashMap::new(), &mut tiles);
        assert!(tiles.iter().all(|t| t.rest == 0));
    }

    #[test]
    fn top_folder_is_the_hovered_tiles_depth_one_ancestor() {
        let tree = TreeNode {
            name: "root".into(),
            size: 1000,
            is_dir: true,
            children: vec![
                TreeNode {
                    name: "a".into(),
                    size: 600,
                    is_dir: true,
                    children: vec![TreeNode {
                        name: "deep".into(),
                        size: 600,
                        is_dir: true,
                        children: vec![file("x.bin", 400), file("y.bin", 200)],
                    }],
                },
                TreeNode { name: "b".into(), size: 300, is_dir: true, children: vec![file("z.bin", 300)] },
                file("loose.txt", 100),
            ],
        };
        let mut tiles = Vec::new();
        place(&tree, Path::new("/root"), (0.0, 0.0, 400.0, 400.0), 0, &HashMap::new(), &mut tiles);
        let at = |name: &str| tiles.iter().position(|t| t.name == name).unwrap();

        assert_eq!(top_folder(&tiles, at("y.bin")), Some(at("a")));
        assert_eq!(top_folder(&tiles, at("deep")), Some(at("a")));
        // Laid out after all of `a`, z must not be credited to it.
        assert_eq!(top_folder(&tiles, at("z.bin")), Some(at("b")));
        // At depth 1 or above there is nothing more to frame.
        assert_eq!(top_folder(&tiles, at("a")), None);
        assert_eq!(top_folder(&tiles, at("loose.txt")), None);
        assert_eq!(top_folder(&tiles, 0), None);
    }

    #[test]
    fn neighbouring_files_are_parted_by_a_seam() {
        // Two tiles sharing an edge at x = 50 leave FILE_GAP between faces.
        let a = file_face((0.0, 0.0, 50.0, 40.0));
        let b = file_face((50.0, 0.0, 50.0, 40.0));
        assert!((b.0 - (a.0 + a.2) - FILE_GAP).abs() < 1e-4);
        // A sliver keeps its thin axis whole and loses the gap only along its length.
        assert_eq!(file_face((10.0, 10.0, 3.0, 40.0)), (10.0, 10.0 + FILE_GAP / 2.0, 3.0, 40.0 - FILE_GAP));
    }

    #[test]
    fn labels_take_whichever_text_colour_reads_better() {
        let light = cce_ui::color::TEXT_FG;
        let dark = cce_ui::color::parse_hex_rgba_linear("#20242b").unwrap();
        // Code's yellow, the worst case for light text, takes dark.
        assert_eq!(label_on(Category::Code.color(), light, dark), dark);
        // The frame itself, and a block's dimmed fill, keep light text.
        assert_eq!(label_on(dark, light, dark), light);
        assert_eq!(label_on(mix(Category::Video.color(), dark, AGGREGATE_DIM), light, dark), light);
    }

    #[test]
    fn share_and_digit_grouping() {
        assert_eq!(share(18, 100), "18%");
        assert_eq!(share(42, 1000), "4.2%");
        assert_eq!(share(1, 1_000_000), "<0.1%");
        assert_eq!(share(5, 0), "—");
        assert_eq!(group_digits(7), "7");
        assert_eq!(group_digits(1000), "1,000");
        assert_eq!(group_digits(12408), "12,408");
        assert_eq!(group_digits(1234567), "1,234,567");
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
