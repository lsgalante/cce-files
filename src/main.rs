use cce_ui::widget::Handle;
use cce_ui::cosmic_text::FontSystem;

use cce_ui::engine::{Application, LogicalPosition, LogicalSize, WindowSettings};
use cce_ui::widget::{MouseButton, ElementState, MouseScrollDelta, KeyEvent, WidgetHost, PageSelector, Paginator, MenuController};
use cce_ui::widget::{GraphController, PathController};

use notify::{Watcher, RecommendedWatcher, RecursiveMode, Config};

use cce_files::{Message, pages, services};
use cce_files::pages::Page;
use cce_files::pages::browse::is_project_dir;

// ── Layout constants ────────────────────────────────────────────────

const DIALOG_W: f32 = 400.0;
const DIALOG_H: f32 = 160.0;

/// Computed rects for the prompt modal ("Open with…" / "Rename"), so layout
/// and hit-testing agree.
struct PromptRects {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    tb: (f32, f32, f32, f32),
    cancel: (f32, f32, f32, f32),
    ok: (f32, f32, f32, f32),
}

/// What a [`PromptDialog`] submits to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PromptKind {
    /// Run the typed command on `path`.
    OpenWith,
    /// Rename `path` to the typed name, in place.
    Rename,
}

impl PromptKind {
    fn title(self) -> &'static str {
        match self {
            PromptKind::OpenWith => "Open with...",
            PromptKind::Rename => "Rename",
        }
    }
    fn prompt(self) -> &'static str {
        match self {
            PromptKind::OpenWith => "Enter command:",
            PromptKind::Rename => "New name:",
        }
    }
    fn ok_label(self) -> &'static str {
        match self {
            PromptKind::OpenWith => "Open",
            PromptKind::Rename => "Rename",
        }
    }
}

/// The single-line modal prompt: one text well, Cancel + an OK button.
struct PromptDialog {
    kind: PromptKind,
    path: std::path::PathBuf,
    textbox: Handle<cce_ui::widget::Adapted<cce_ui::widget::TextBox>>,
    cancel: ButtonHandle,
    ok: ButtonHandle,
}

type ButtonHandle = Handle<cce_ui::widget::Adapted<cce_ui::widget::Button>>;

/// The chooser's footer buttons (`--select`, `--select-dir`, `--save`):
/// Cancel, and Select or Save.
#[derive(Clone, Copy)]
struct ChooserButtons {
    cancel: ButtonHandle,
    confirm: ButtonHandle,
}

/// A button wearing the toolkit's own face with no tint of its own: a
/// transparent face over the theme's border and relief (the closed-dropdown
/// convention) and a faint neutral hover. The toolkit's Primary face is
/// itself blue-tinted, which is exactly what the chooser's and the prompt's
/// buttons are not supposed to be. Placed and painted by `rebuild_layout`,
/// routed by the input hooks (`route_buttons`).
fn plain_button(label: &str) -> cce_ui::widget::Adapted<cce_ui::widget::Button> {
    cce_ui::widget::Button::new(0.0, 0.0, 0.0, 0.0)
        .with_label(label)
        .with_bg([0.0, 0.0, 0.0, 0.0])
        .with_hover_bg([1.0, 1.0, 1.0, 0.10])
}

fn prompt_rects(win_w: f32, win_h: f32) -> PromptRects {
    let x = (win_w - DIALOG_W) / 2.0;
    let y = (win_h - DIALOG_H) / 2.0;
    let tb_h = cce_ui::layout::textbox_height();
    let btn_h = cce_ui::layout::button_height();
    // The dialog is a pane plate standing on the root: its content insets
    // from the rim by the pane rung, and its two buttons sit one pane gap
    // apart, right-aligned to that inset.
    let pad = cce_ui::layout::plate_padding();
    let gap = cce_ui::layout::plate_gap();
    let (cancel_w, open_w) = (70.0, 80.0);
    let btn_y = y + DIALOG_H - btn_h - pad;
    let open_x = x + DIALOG_W - pad - open_w;
    PromptRects {
        x,
        y,
        w: DIALOG_W,
        h: DIALOG_H,
        // TODO(style): the 60px drop to the well (title, then the prompt at
        // 42) is the dialog's own text rhythm, not a rung.
        tb: (x + pad, y + 60.0, DIALOG_W - 2.0 * pad, tb_h),
        cancel: (open_x - gap - cancel_w, btn_y, cancel_w, btn_h),
        ok: (open_x, btn_y, open_w, btn_h),
    }
}

/// Which side of an occluder a box's surviving span lies on, and where the cut
/// falls — see [`wider_side`].
enum Side {
    /// Keep what lies before the occluder: clamp the box's far edge to this.
    Before(f32),
    /// Keep what lies after it: clamp the box's near edge to this.
    After(f32),
}

/// On one axis, which side of the occluder leaves more of the box — the piece
/// worth keeping when only one can be. `None` when the occluder covers the span
/// outright and neither side survives.
fn wider_side(t_min: f32, t_max: f32, o_min: f32, o_max: f32) -> Option<Side> {
    let before = o_min - t_min;
    let after = t_max - o_max;
    if before <= 0.0 && after <= 0.0 {
        return None;
    }
    if before >= after {
        Some(Side::Before(o_min))
    } else {
        Some(Side::After(o_max))
    }
}

/// Clip a text/label box against overlay faces (`[x, y, w, h]` each) so it
/// does not bleed through menus/dialogs. Returns the adjusted clip bounds, or
/// `None` if the box is fully covered (should be discarded).
fn occlude_against(
    mut bounds: [f32; 4],
    t_min_x: f32,
    t_max_x: f32,
    t_min_y: f32,
    t_max_y: f32,
    overlays: &[[f32; 4]],
) -> Option<[f32; 4]> {
    for &[ox, oy, ow, oh] in overlays {
        let o_min_x = ox;
        let o_max_x = ox + ow;
        let o_min_y = oy;
        let o_max_y = oy + oh;

        if t_max_x > o_min_x && t_min_x < o_max_x && t_max_y > o_min_y && t_min_y < o_max_y {
            if t_min_x >= o_min_x && t_max_x <= o_max_x && t_min_y >= o_min_y && t_max_y <= o_max_y {
                return None;
            }
            // Cut on ONE axis, on ONE side. The result of subtracting a rect
            // from a rect is not a rect, so this picks the largest piece that
            // is: trim x, unless the occluder already spans the box
            // horizontally and only a y cut can uncover anything.
            //
            // Clamping every side independently is what this used to do, and
            // two clamps on one axis cross over into an inverted, empty band —
            // so a box the occluder merely dipped into vanished whole, the
            // part nothing covered along with the rest. That is how hovering a
            // context-menu row erased the file-list row beside it: the hover
            // fill's top edge landed inside that row's text band, and the y
            // clamp it triggered threw away the name sitting well clear of the
            // menu.
            if o_min_x <= t_min_x && o_max_x >= t_max_x {
                match wider_side(t_min_y, t_max_y, o_min_y, o_max_y) {
                    Some(Side::Before(cut)) => bounds[3] = bounds[3].min(cut),
                    Some(Side::After(cut)) => bounds[1] = bounds[1].max(cut),
                    None => return None,
                }
            } else {
                match wider_side(t_min_x, t_max_x, o_min_x, o_max_x) {
                    Some(Side::Before(cut)) => bounds[2] = bounds[2].min(cut),
                    Some(Side::After(cut)) => bounds[0] = bounds[0].max(cut),
                    None => return None,
                }
            }
        }
    }
    Some(bounds)
}

/// Height of the chooser-mode bottom action bar (the band carved into the plate).
const SELECT_BAR_H: f32 = 48.0;

/// Cut the text of `list` away from `overlays` (`[x, y, w, h]` each): the
/// faces of the menu and the dialog, painted over it later in the frame.
/// Text is drawn in one pass after all geometry, so a label under an overlay
/// would otherwise show through it. A label wholly under one is dropped; one
/// partly under keeps the widest piece a single clip can hold
/// ([`occlude_against`]). Its extent is estimated, not shaped.
fn occlude_text(list: &mut cce_ui::scene::paint::DisplayList, overlays: &[[f32; 4]], win: (f32, f32)) {
    use cce_ui::scene::paint::Prim;
    if overlays.is_empty() {
        return;
    }
    list.items.retain_mut(|item| {
        let Prim::Text { text, x, y, font_size, bounds, .. } = &mut item.prim else { return true };
        let start = bounds.unwrap_or([0.0, 0.0, win.0, win.1]);
        let w = text.chars().count() as f32 * *font_size * 0.65;
        match occlude_against(start, *x, *x + w, *y, *y + *font_size * 1.4, overlays) {
            Some(b) => {
                *bounds = Some(b);
                true
            }
            None => false,
        }
    });
}

// ── State ───────────────────────────────────────────────────────────

/// The shared context menu while it shows rows the app runs itself — a
/// list row's, a Space tile's or a graph node's menu, the breadcrumb's, the
/// preview pane's corner menu: which show of it this is
/// (`context_menu::generation`), so a stale pick never runs, and what each
/// row sends (`None` for a header or a separator). The toolkit runs nothing
/// for these rows: a press, Enter or a screen reader's Click on one records
/// it (`context_menu::take_picked`) and the app sends the row's message
/// ([`FilesystemApp::take_menu_pick`]).
struct AppMenu {
    generation: u64,
    rows: Vec<Option<Message>>,
    /// The preview pane's corner menu, whose dot stays lit while it is open.
    corner: bool,
}

/// The breadcrumb menu below its header: Copy Path, then the view switch —
/// a dropdown beside the breadcrumb until 2026-10-06 — offering the views
/// other than the one showing (the page itself says which view it is).
fn breadcrumb_menu_rows(path: String, current: Page) -> Vec<(String, Option<Message>)> {
    let mut rows = vec![("Copy Path".to_string(), Some(Message::CopyPath(path))), ("-".to_string(), None)];
    for page in Page::ALL.into_iter().filter(|&p| p != current) {
        rows.push((page.view_label().to_string(), Some(Message::SwitchPage(page))));
    }
    rows
}

/// The context menu for one path — a Space tile or a graph node — header
/// first. A directory opens by navigating to it, as a double-click does;
/// `openable` false leaves Open out (the directory already showing).
fn path_menu(name: &str, path: std::path::PathBuf, is_dir: bool, openable: bool) -> Vec<(String, Option<Message>)> {
    let kind = if is_dir { "Directory" } else { "File" };
    let mut options = vec![(format!("[{kind}] {name}"), None)];
    let copy = Some(Message::CopyPath(path.to_string_lossy().into_owned()));
    if is_dir {
        if openable {
            options.push(("Open".to_string(), Some(Message::Browse(pages::browse::BrowseMessage::NavigateToPath(path)))));
        }
    } else {
        options.push(("Open".to_string(), Some(Message::OpenFile(path.clone()))));
        options.push(("Open with...".to_string(), Some(Message::PromptOpenWith(path))));
    }
    options.push(("Copy Path".to_string(), copy));
    options
}

/// The Space map's context menu for `tile`. The root tile is the map's own
/// directory, so it offers nothing to open. A "smaller items" block carries
/// its directory's path, so its rows name the folder they act on.
fn space_tile_menu(tile: &pages::space::Tile) -> Vec<(String, Option<Message>)> {
    if tile.rest > 0 {
        let path = tile.path.clone();
        return vec![
            (tile.name.clone(), None),
            ("Open Folder".to_string(), Some(Message::Browse(pages::browse::BrowseMessage::NavigateToPath(path.clone())))),
            ("Copy Folder Path".to_string(), Some(Message::CopyPath(path.to_string_lossy().into_owned()))),
        ];
    }
    path_menu(&tile.name, tile.path.clone(), tile.is_dir, tile.depth > 0)
}

/// The graph's context menu for node `idx`: the parent (when there is one),
/// then the directory showing, then `entries` in order — the order
/// `NetworkState::populate_graph` lays them in.
fn graph_node_menu(idx: usize, current_dir: &std::path::Path, entries: &[pages::browse::DirEntry]) -> Option<Vec<(String, Option<Message>)>> {
    let dir_name = |p: &std::path::Path| p.file_name().map_or_else(|| "/".to_string(), |n| n.to_string_lossy().into_owned());
    let parent = current_dir.parent();
    let offset = if parent.is_some() { 2 } else { 1 };
    match (parent, idx) {
        (Some(parent), 0) => Some(path_menu(&dir_name(parent), parent.to_path_buf(), true, true)),
        _ if idx + 1 == offset => Some(path_menu(&dir_name(current_dir), current_dir.to_path_buf(), true, false)),
        _ => {
            let entry = entries.get(idx.checked_sub(offset)?)?;
            Some(path_menu(&entry.name, entry.path.clone(), entry.is_dir, true))
        }
    }
}

/// App-owned two-pane horizontal split, replacing the dissolved `SplitBox` +
/// `BrowseContainer`/`NetworkContainer` shims (Phase 6y). Those existed to (a) position
/// pane content — but the pages already lay out and render everything from the pane rect,
/// the container copies just coincided (the Phase 0 double-paint) — and (b) own the
/// divider: its quad, hover tint, and proportion drag. This is (b), app-side, with the
/// `SplitBox` two-child horizontal math verbatim.
/// A file as `stat` sees it: what decides whether a preview already read
/// is still the file's. The link itself and, through it, its target — a
/// symlink to a file being rewritten keeps its own stamp.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PreviewStamp {
    link: Option<(u64, u64, std::time::SystemTime)>,
    target: Option<(u64, u64, std::time::SystemTime)>,
}

impl PreviewStamp {
    fn of(path: &std::path::Path) -> Self {
        use std::os::unix::fs::MetadataExt;
        let key = |m: std::fs::Metadata| (m.ino(), m.len(), m.modified().unwrap_or(std::time::UNIX_EPOCH));
        Self {
            link: std::fs::symlink_metadata(path).ok().map(key),
            target: std::fs::metadata(path).ok().map(key),
        }
    }
}

/// Width of the preview pane's column while collapsed to its title stub:
/// room for the label and the corner control that restores it.
const PREVIEW_STUB_W: f32 = 170.0;

struct SplitPane {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    /// Left pane's share of the space (proportions summed to 1.0 in the legacy SplitBox).
    frac: f32,
    min_left: f32,
    min_right: f32,
    gap: f32,
    dragging: bool,
    hovered: bool,
}

impl SplitPane {
    fn new(frac: f32, min_left: f32, min_right: f32, gap: f32) -> Self {
        Self { x: 0.0, y: 0.0, w: 0.0, h: 0.0, frac, min_left, min_right, gap, dragging: false, hovered: false }
    }

    fn set_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.x = x;
        self.y = y;
        self.w = w;
        self.h = h;
    }

    fn left_w(&self) -> f32 {
        self.frac * (self.w - self.gap).max(0.0)
    }

    fn left_rect(&self) -> (f32, f32, f32, f32) {
        (self.x, self.y, self.left_w(), self.h)
    }

    fn right_rect(&self) -> (f32, f32, f32, f32) {
        let lx = self.x + self.left_w() + self.gap;
        (lx, self.y, (self.x + self.w - lx).max(0.0), self.h)
    }

    fn divider_rect(&self) -> (f32, f32, f32, f32) {
        (self.x + self.left_w(), self.y, self.gap, self.h)
    }

    fn hit_divider(&self, px: f32, py: f32) -> bool {
        let (sx, sy, sw, sh) = self.divider_rect();
        px >= sx && px <= sx + sw && py >= sy && py <= sy + sh
    }

    /// Divider drag + hover (`SplitBox::on_cursor_moved`, two-child horizontal case).
    fn cursor_moved(&mut self, px: f32, py: f32) -> bool {
        let mut handled = false;
        if self.dragging {
            let combined = (self.w - self.gap).max(0.0);
            if combined > 0.1 {
                let new_left = (px - self.x - self.gap / 2.0)
                    .clamp(self.min_left, (combined - self.min_right).max(self.min_left));
                let new_frac = new_left / combined;
                if (new_frac - self.frac).abs() > 0.0001 {
                    self.frac = new_frac;
                    handled = true;
                }
            }
        }
        let new_hovered = !self.dragging && self.hit_divider(px, py);
        if new_hovered != self.hovered {
            self.hovered = new_hovered;
            handled = true;
        }
        handled
    }

    /// Left press on the divider grabs it (`SplitBox::mouse_input`).
    fn press(&mut self, px: f32, py: f32) -> bool {
        if self.hit_divider(px, py) {
            self.dragging = true;
            return true;
        }
        false
    }

    /// Returns whether a drag was in progress (the legacy release consumed the event).
    fn release(&mut self) -> bool {
        std::mem::take(&mut self.dragging)
    }

    /// The divider's mark (`SplitBox::extra_quads`): accent while dragging, tint
    /// on hover, and NOTHING at rest.
    ///
    /// At rest the gap is left as bare window plate, which is what the panes
    /// either side already establish — a strip of surface between two plates,
    /// with no line drawn down it. The 1px hairline that used to sit here was
    /// the last painted separator in a UI that is otherwise all lit geometry,
    /// and against 4 logical px of clearance it read as a mark on a plain
    /// rather than as a seam.
    ///
    /// Note what this deliberately does NOT do: it does not make the divider
    /// look like the preview|details seam. That seam's floor measures ~79
    /// against this plate's 96 — it is genuinely RECESSED — and removing marks
    /// cannot recess a flush gap. Matching it would take a carve. (For the
    /// opposite mistake, see the reverted `Prim::Ridge` bead in this file's
    /// history: a crest where that seam has a trough.)
    fn divider_quad(&self) -> Option<(f32, f32, f32, f32, [f32; 4])> {
        let (sx, sy, sw, sh) = self.divider_rect();
        let color = if self.dragging {
            [0.36, 0.56, 0.38, 0.8]
        } else if self.hovered {
            [0.25, 0.25, 0.32, 0.6]
        } else {
            return None;
        };
        Some((sx + sw / 2.0 - 1.0, sy, 2.0, sh, color))
    }
}


/// Browse-page shortcuts, resolved once at startup from input.kdl
/// (`cce-files` domain → `cce-ui` domain), defaulting to vim-ish j/k for
/// selection and h/l (parent/enter) for directories. Arrow keys,
/// Enter-in-save-mode, and Escape stay fixed.
struct BrowseKeys {
    open_file: String,
    enter_dir: String,
    parent_dir: String,
    select_next: String,
    select_prev: String,
    delete_entry: String,
    toggle_hidden: String,
}

impl BrowseKeys {
    fn load() -> Self {
        let get = cce_ui::input::app_chord;
        Self {
            open_file: get("open_file", "enter"),
            enter_dir: get("enter_dir", "l"),
            parent_dir: get("parent_dir", "h"),
            select_next: get("select_next", "j"),
            select_prev: get("select_prev", "k"),
            delete_entry: get("delete_entry", "delete"),
            toggle_hidden: get("toggle_hidden", "."),
        }
    }
}

/// Which recessed well holds pointer focus — the one wearing the accent ring
/// (its relief lighting swapped for the tinted carve's wrapped glint, the
/// same focus language as plates). Follows the last left press that reaches
/// page content; the content well (list / treemap) starts focused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusedWell {
    Content,
    PreviewTop,
    PreviewBottom,
}

struct FilesystemApp {
    current_page: Page,
    browse: pages::browse::BrowseState,
    network: pages::network::NetworkState,
    space: pages::space::SpaceState,
    preview: cce_files::preview_pane::PreviewPane,
    /// What the preview was last asked for: the path, and the file as it
    /// stood then. Every browse message — a watcher refresh, a search
    /// keystroke — used to ask again, and each ask decodes and resizes an
    /// image and spawns `xdg-mime` twice; with a download growing beside a
    /// selected photo that was a full decode per refresh. Now a request goes
    /// out only when this changes.
    preview_requested: Option<(std::path::PathBuf, PreviewStamp)>,
    /// Generation of the newest preview request; a `PreviewLoaded` from any
    /// other is a selection the user has already moved past.
    preview_generation: u64,
    /// See [`FocusedWell`]; drives the per-well `focused` flags each rebuild.
    focused_well: FocusedWell,

    // Command-line chooser options
    select_mode: bool,
    select_directory: bool,
    save_mode: bool,

    // Rendering resources
    /// The window's content as the last rebuild painted it — every part in
    /// its layer order, page text occluded by the overlays above it — and
    /// appended to each frame's display list until the next rebuild.
    frame: cce_ui::scene::paint::DisplayList,
    font_system: FontSystem,
    needs_rebuild: bool,
    /// What the last rebuild's widgets claimed for text input (an editing
    /// TextBox, `cce_ui::text_input::capture`). Claimed again every frame:
    /// a frame that only replays the cached layout paints no widget, and
    /// would otherwise read as the field closing.
    text_claim: Option<[f32; 4]>,
    width: u32,
    height: u32,
    scale_factor: f64,
    /// The chooser's footer buttons, in a chooser mode only.
    chooser: Option<ChooserButtons>,
    cursor_x: f32,
    cursor_y: f32,
    paginator: Handle<cce_ui::widget::Adapted<Paginator>>,
    just_initialized: bool,
    ui_context: cce_ui::context::UiContext,
    watcher: Option<notify::RecommendedWatcher>,
    fs_service: services::fs::FsService,
    /// Whether a renderer has been handed over yet — the first one is the
    /// process's own, any later one is a replacement after a reconnect. See
    /// `renderer_init`.
    seen_renderer: bool,
    /// The app's own messages, for what arrives between events: a row of
    /// an [`AppMenu`] run by a screen reader's Click (taken in `tick`).
    sender: calloop::channel::Sender<Message>,
    prompt_dialog: Option<PromptDialog>,
    browse_split: SplitPane,
    network_split: SplitPane,
    space_split: SplitPane,
    /// The preview pane's plate-dock state (cce-ui RFC 7c-2): collapsing
    /// narrows the pane column to a title stub and the list keeps the space.
    preview_dock: cce_ui::widget::plate_dock::PlateDockState,
    /// The three splits' fracs as they stood before a collapse, restored on
    /// expand.
    preview_prior_fracs: Option<(f32, f32, f32)>,
    /// The shared menu's rows while the app's own are showing; routed
    /// before the toolkit's dispatch.
    app_menu: Option<AppMenu>,
    // Space's double-click is tracked by path, not row index: its tiles are
    // renumbered by every relayout, so an index would not survive a resize.
    last_space_click_time: std::time::Instant,
    last_space_path: Option<std::path::PathBuf>,
    last_click_time: std::time::Instant,
    last_clicked_idx: Option<usize>,
    keys: BrowseKeys,
}


// ── Layout Rebuild ──────────────────────────────────────────────────

impl FilesystemApp {
    /// Show `dialog` in place of the prompt that is up, if any, whose text box
    /// and buttons leave the context with it.
    fn set_prompt(&mut self, dialog: Option<PromptDialog>) {
        if let Some(old) = std::mem::replace(&mut self.prompt_dialog, dialog) {
            self.ui_context.remove(old.textbox);
            self.ui_context.remove(old.cancel);
            self.ui_context.remove(old.ok);
        }
    }

    /// A prompt of `kind` on `path` around `textbox`, its buttons made.
    fn new_prompt(&mut self, kind: PromptKind, path: std::path::PathBuf, textbox: Handle<cce_ui::widget::Adapted<cce_ui::widget::TextBox>>) -> PromptDialog {
        let cancel = self.ui_context.insert(plain_button("Cancel"));
        let ok = self.ui_context.insert(plain_button(kind.ok_label()));
        PromptDialog { kind, path, textbox, cancel, ok }
    }

    /// The buttons standing now, each with what it sends: the prompt's while
    /// one is up — it covers the window — else the chooser's.
    fn live_buttons(&self) -> Vec<(ButtonHandle, Message)> {
        if let Some(d) = &self.prompt_dialog {
            return vec![(d.cancel, Message::PromptCancel), (d.ok, Message::PromptSubmit)];
        }
        match self.chooser {
            Some(c) => vec![(c.cancel, Message::SelectCancel), (c.confirm, Message::SelectOpen)],
            None => Vec::new(),
        }
    }

    /// Route a pointer event to the live buttons — a press arms the one it
    /// lands on, a release commits it (in its rect) or cancels it, a move
    /// enters or leaves. True when one took it.
    fn route_buttons(&mut self, ev: &cce_ui::widget::Event) -> bool {
        let mut taken = false;
        for (h, _) in self.live_buttons() {
            taken |= self.ui_context.propagate_event(ev, h.id());
        }
        taken
    }

    /// Whether `(px, py)` is on a live button.
    fn over_button(&self, px: f32, py: f32) -> bool {
        self.live_buttons().into_iter().any(|(h, _)| {
            let (x, y, w, bh) = self.ui_context[h].rect();
            px >= x && px <= x + w && py >= y && py <= y + bh
        })
    }

    /// What the widgets reported since the last call (`take_events`), as the
    /// messages they send, in order: a live button clicked (a pointer
    /// release on it, Enter / Space while it has the keyboard, an assistive
    /// tool's click), the search box edited (typed, pasted, set by an
    /// assistive tool). A prompt button closes the prompt's field first, so
    /// Submit reads what it holds. Every other report — the save-name and
    /// prompt fields' edits, which are read when they are submitted — is
    /// dropped.
    fn widget_messages(&mut self) -> Vec<Message> {
        use cce_ui::widget::WidgetEvent;
        let buttons = self.live_buttons();
        let search = self.browse.search_box.id();
        let mut out = Vec::new();
        let mut clicked = false;
        for (id, ev) in self.ui_context.take_events() {
            match ev {
                WidgetEvent::Clicked => {
                    if let Some((_, msg)) = buttons.iter().find(|(h, _)| h.id() == id) {
                        out.push(msg.clone());
                        clicked = true;
                    }
                }
                WidgetEvent::Changed if id == search => {
                    let text = self.ui_context[self.browse.search_box].text.clone();
                    out.push(Message::Browse(pages::browse::BrowseMessage::SearchChanged(text)));
                }
                _ => {}
            }
        }
        if clicked && let Some(tb) = self.prompt_dialog.as_ref().map(|d| d.textbox) {
            self.ui_context[tb].unfocus();
            self.ui_context.clear_focus();
        }
        out
    }

    /// Sweep the widgets' reports ([`Self::widget_messages`]) from a hook
    /// that returns one message: every one but the last runs now, the last
    /// is returned, so they arrive in the order they were raised.
    fn drain_widget_events(&mut self, needs_rebuild: &mut bool) -> Option<Message> {
        let mut msgs = self.widget_messages();
        let last = msgs.pop();
        self.run_now(msgs, needs_rebuild);
        last
    }

    /// The widgets' reports swept after a hook that returned `msg`: they
    /// were raised during it, so they run first and `msg` is returned. A
    /// hook with nothing of its own returns the sweep's last.
    fn after_dispatch(&mut self, msg: Option<Message>, needs_rebuild: &mut bool) -> Option<Message> {
        match msg {
            Some(msg) => {
                let msgs = self.widget_messages();
                self.run_now(msgs, needs_rebuild);
                Some(msg)
            }
            None => self.drain_widget_events(needs_rebuild),
        }
    }

    fn run_now(&mut self, msgs: Vec<Message>, needs_rebuild: &mut bool) {
        for msg in msgs {
            let mut exit = false;
            self.update(msg, needs_rebuild, &mut exit);
        }
    }

    /// Kick off a subtree scan if the Space page is showing a directory it has
    /// not scanned. Cheap to call — it no-ops off the Space page, and while a
    /// scan for the same directory is already running.
    fn ensure_space_scan(&mut self) {
        if self.current_page != Page::Space {
            return;
        }
        let dir = self.browse.current_dir.clone();
        if !self.space.needs_scan(&dir) {
            return;
        }
        let cancel = self.space.begin_scan(&dir);
        self.fs_service.send(services::fs::FsRequest::ScanTree(dir, cancel));
    }

    /// Show the app's context menu at (x, y) — a list row's, a Space tile's,
    /// a graph node's. Row 0 of `options` is the header (dim, never run).
    fn open_context_menu(&mut self, x: f32, y: f32, options: Vec<(String, Option<Message>)>) {
        let target = self.ui_context[self.paginator].base().id();
        self.show_app_menu(x, y, options, 1, target, false);
    }

    /// Show the shared menu at (x, y) with rows the app runs: `rows` are
    /// (label, what the row sends), the first `header_count` of them
    /// headers. `target` is the widget a toolkit action would run on — none
    /// of these rows has one, and every pick is routed without a context, so
    /// it only ties the menu to a widget whose hiding hides it.
    fn show_app_menu(
        &mut self,
        x: f32,
        y: f32,
        rows: Vec<(String, Option<Message>)>,
        header_count: usize,
        target: cce_ui::widget::WidgetId,
        corner: bool,
    ) {
        use cce_ui::widget::context_menu as cm;
        let (labels, rows): (Vec<String>, Vec<Option<Message>>) = rows.into_iter().unzip();
        cm::show(x, y, labels, header_count, target);
        self.app_menu = Some(AppMenu { generation: cm::generation(), rows, corner });
    }

    /// Whether the shared menu showing is the app's own — the show
    /// [`AppMenu`] was built for, not since replaced or dismissed.
    fn app_menu_live(&self) -> bool {
        use cce_ui::widget::context_menu as cm;
        cm::is_visible() && self.app_menu.as_ref().is_some_and(|m| m.generation == cm::generation())
    }

    /// The message of the app-menu row run since the last call — a press,
    /// a release, Enter or a screen reader's Click on it — once. A pick from
    /// any other show of the menu is dropped.
    fn take_menu_pick(&mut self) -> Option<Message> {
        let (generation, row) = cce_ui::widget::context_menu::take_picked()?;
        let menu = self.app_menu.take_if(|m| m.generation == generation)?;
        menu.rows.into_iter().nth(row).flatten()
    }

    /// Ask for `path`'s preview unless that is what was last asked for and
    /// the file has not changed since. `force` asks regardless (the texture
    /// was lost with its renderer).
    fn request_preview(&mut self, path: std::path::PathBuf, force: bool) {
        let stamp = PreviewStamp::of(&path);
        if !force
            && self
                .preview_requested
                .as_ref()
                .is_some_and(|(p, st)| *p == path && *st == stamp)
        {
            return;
        }
        self.preview_requested = Some((path.clone(), stamp));
        let generation = self.next_preview_generation();
        self.fs_service.send(services::fs::FsRequest::ReadPreview(path, generation));
    }

    /// Empty the preview pane. Also retires whatever request is in flight,
    /// so a slow read cannot fill the pane back in after it was cleared.
    fn clear_preview(&mut self) {
        self.preview_requested = None;
        self.next_preview_generation();
        pages::preview::update(&mut self.preview, pages::preview::PreviewMessage::Clear);
    }

    fn next_preview_generation(&mut self) -> u64 {
        self.preview_generation += 1;
        self.fs_service
            .preview_latest
            .store(self.preview_generation, std::sync::atomic::Ordering::Relaxed);
        self.preview_generation
    }

    fn start_watching(&mut self, path: std::path::PathBuf) {
        use tokio::sync::mpsc;
        use std::time::Duration;

        let (tx, mut rx) = mpsc::channel::<()>(100);
        let fs_service = self.fs_service.sender.clone();
        let path_clone = path.clone();

        tokio::spawn(async move {
            while rx.recv().await.is_some() {
                tokio::time::sleep(Duration::from_millis(150)).await;
                while rx.try_recv().is_ok() {}

                let _ = fs_service.send(services::fs::FsRequest::RefreshDirectory(path_clone.clone())).await;
            }
        });

        let mut watcher = match RecommendedWatcher::new(
            move |res: Result<notify::Event, notify::Error>| {
                if let Ok(event) = res {
                    match event.kind {
                        notify::EventKind::Create(_) | notify::EventKind::Modify(_) | notify::EventKind::Remove(_) => {
                            let _ = tx.try_send(());
                        }
                        _ => {}
                    }
                }
            },
            Config::default(),
        ) {
            Ok(w) => w,
            Err(e) => {
                log::error!("Failed to create watcher: {:?}", e);
                return;
            }
        };

        if let Err(e) = watcher.watch(&path, RecursiveMode::NonRecursive) {
            log::error!("Failed to watch path {}: {:?}", path.display(), e);
            return;
        }

        self.watcher = Some(watcher);
    }

    /// The rect the preview pane's corner control anchors to: the full pane,
    /// or its title-stub band while stubbed (cce-ui RFC 7c-2).
    fn preview_dock_band(&self) -> (f32, f32, f32, f32) {
        let split = match self.current_page {
            Page::Browse => &self.browse_split,
            Page::Network => &self.network_split,
            Page::Space => &self.space_split,
        };
        let (rx, ry, rw, rh) = split.right_rect();
        if self.preview_dock.stubbed() {
            (rx, ry, rw, cce_ui::widget::plate_dock::STUB_H)
        } else {
            (rx, ry, rw, rh)
        }
    }

    fn dispatch_preview_dock(&mut self, action: cce_ui::widget::plate_dock::PlateDockAction) {
        use cce_ui::widget::plate_dock::PlateDockAction;
        match action {
            PlateDockAction::Collapse => {
                self.preview_prior_fracs =
                    Some((self.browse_split.frac, self.network_split.frac, self.space_split.frac));
                self.preview_dock.collapsed = true;
            }
            PlateDockAction::Expand => {
                if let Some((b, n, sp)) = self.preview_prior_fracs.take() {
                    self.browse_split.frac = b;
                    self.network_split.frac = n;
                    self.space_split.frac = sp;
                }
                self.preview_dock.collapsed = false;
            }
            // No detach model here (yet): standard_menu is called with
            // can_detach = false, so these rows never appear.
            PlateDockAction::Detach | PlateDockAction::Reattach => {}
        }
        self.needs_rebuild = true;
    }

    /// Lay the window out and flatten it into the cached frame, keeping what
    /// its widgets claimed for text input (`text_claim`).
    fn rebuild_layout(&mut self) {
        let ((), claim) = cce_ui::text_input::capture(|| self.rebuild_layout_inner());
        self.text_claim = claim;
    }

    fn rebuild_layout_inner(&mut self) {

        // Well focus → the emitters (each picks the tinted carve when set).
        self.browse.list.focused = self.focused_well == FocusedWell::Content;
        self.space.focused = self.focused_well == FocusedWell::Content;
        self.preview.focused_well = match self.focused_well {
            FocusedWell::PreviewTop => Some(cce_files::preview_pane::PreviewWell::Top),
            FocusedWell::PreviewBottom => Some(cce_files::preview_pane::PreviewWell::Bottom),
            FocusedWell::Content => None,
        };

        self.ui_context.clear_hierarchy();
        self.ui_context[self.browse.save_name_box].prepare_text(&mut self.font_system);
        self.ui_context[self.browse.search_box].prepare_text(&mut self.font_system);

        // root plate container DISSOLVED: top-level widgets register parentless below; the
        // window plate tuple is emitted in the legacy aggregate order (after the plain
        // child quads).

        // Clear all widgets' hierarchy links
        self.ui_context.lend_h(self.paginator, |w, ctx| { w.clear_children(ctx); w.set_parent(None, ctx); });


        self.ui_context.lend_h(self.browse.save_name_box, |w, ctx| { w.clear_children(ctx); w.set_parent(None, ctx); });
        self.ui_context.lend_h(self.browse.breadcrumb, |w, ctx| { w.clear_children(ctx); w.set_parent(None, ctx); });
        self.ui_context.lend_h(self.network.breadcrumb, |w, ctx| { w.clear_children(ctx); w.set_parent(None, ctx); });
        self.ui_context.lend_h(self.network.graph, |w, ctx| { w.clear_children(ctx); w.set_parent(None, ctx); });
        self.ui_context.lend_h(self.space.breadcrumb, |w, ctx| { w.clear_children(ctx); w.set_parent(None, ctx); });
        if let Some(tb) = self.prompt_dialog.as_ref().map(|d| d.textbox) {
            self.ui_context.lend_h(tb, |w, ctx| { w.clear_children(ctx); w.set_parent(None, ctx); });
        }

        let has_sidebar = false;
        let sidebar_w = if has_sidebar { self.ui_context[self.paginator].sidebar_w() } else { 0.0 };
        // Inset from the WINDOW edge: the root plate's roll plus its padding
        // (the padding alone left most of the run on the roll, so the edge
        // read narrower than the gap between the panes).
        let pad = cce_ui::layout::root_plate_inset();
        let browse_x = if has_sidebar { sidebar_w + pad + 1.0 } else { pad };
        let usable_w = self.width as f32 - sidebar_w - (if has_sidebar { 1.0 } else { 0.0 }) - 2.0 * pad;
        let content_y = pad;

        let select_bar_h = SELECT_BAR_H;
        let content_h = if self.select_mode {
            self.height as f32 - 2.0 * pad - select_bar_h
        } else {
            self.height as f32 - 2.0 * pad
        };

        if has_sidebar {
            self.ui_context.lend_h(self.paginator, |w, ctx| w.set_parent(None, ctx));
        }

        // SplitBox + pane containers DISSOLVED (Phase 6y): the split is app state; the
        // pages lay out and render the left pane's content from the pane rect (they
        // always did — the container copies just coincided), the preview renders into
        // the right pane below.
        match self.current_page {
            Page::Browse => self.browse_split.set_rect(browse_x, content_y, usable_w, content_h),
            Page::Network => self.network_split.set_rect(browse_x, content_y, usable_w, content_h),
            Page::Space => self.space_split.set_rect(browse_x, content_y, usable_w, content_h),
        }
        // Collapsed preview (plate-dock, cce-ui RFC 7c-2): every page's pane
        // column narrows to the stub's width — re-forced each layout so a
        // window resize keeps the stub fixed while the list takes the rest.
        if self.preview_dock.collapsed {
            for split in [&mut self.browse_split, &mut self.network_split, &mut self.space_split] {
                let combined = (split.w - split.gap).max(1.0);
                split.frac = (1.0 - PREVIEW_STUB_W / combined).clamp(0.0, 1.0);
            }
        }

        if let Some(tb) = self.prompt_dialog.as_ref().map(|d| d.textbox) {
            self.ui_context.lend_h(tb, |w, ctx| w.set_parent(None, ctx));
        }

        let (win_w, win_h) = (self.width as f32, self.height as f32);
        // The window and page layers, painted in call order. Their text is cut
        // away from the overlays' faces below, which are collected as they
        // are painted (`occlude_text`).
        let mut pc = cce_ui::scene::paint::PaintCtx::new();

        // The dissolved splitter's paint: its divider quad, then the preview
        // pane (the only pane content the pages don't render themselves). The
        // left pane's container copy is gone — the legacy aggregate painted it
        // UNDER the page's own copy, double-compositing every translucent quad.
        // A collapsed preview draws neither divider nor content — the stub
        // band and its corner control paint in display_list, over everything
        // (cce-ui RFC 7c-2).
        if !self.preview_dock.collapsed {
            let split = match self.current_page {
                Page::Browse => &self.browse_split,
                Page::Network => &self.network_split,
                Page::Space => &self.space_split,
            };
            let divider = split.divider_quad();
            let (px_r, py_r, pw_r, ph_r) = split.right_rect();
            if let Some((dx, dy, dw, dh, dc)) = divider {
                pages::fill(&mut pc, dc, dx, dy, dw, dh);
            }
            // The pane cuts its own labels to its rect (the old SplitBox
            // clamp, absorbed).
            self.preview.set_rect(px_r, py_r, pw_r, ph_r);
            self.preview.push_prims(&mut pc);
        }

        // The chooser's bottom action bar, below the content region.
        if let Some(chooser) = self.chooser {
            let bar_y = self.height as f32 - select_bar_h - cce_ui::layout::root_plate_inset();
            // Divider line — under control_relief the bar is a band carved into the
            // plate (see display_list), so the flat line is the fallback only.
            if !cce_ui::layout::control_relief() {
                pages::fill(&mut pc, [0.15, 0.20, 0.16, 1.0], browse_x, bar_y, usable_w, 1.0);
            }

            let btn_h = cce_ui::layout::button_height();
            let btn_y = bar_y + (select_bar_h - btn_h) / 2.0;

            // Cancel and Save/Select: equal widths, one gap, right-aligned to
            // the content region with the bar's own padding — and the
            // toolkit's plain button face, not hand-picked red/green tints.
            let btn_w = 84.0;
            let gap = cce_ui::layout::root_plate_gap();
            // Flush with the panels' right edge — an extra inset here left the
            // buttons hanging short of the column above them.
            let confirm_x = browse_x + usable_w - btn_w;
            let cancel_x = confirm_x - gap - btn_w;
            pages::widget(&mut pc, chooser.cancel, cancel_x, btn_y, btn_w, btn_h, &mut self.ui_context);
            pages::widget(&mut pc, chooser.confirm, confirm_x, btn_y, btn_w, btn_h, &mut self.ui_context);
        }

        // The page, in the left pane. Laid out inside the content region, it
        // needs no cut to it: the list holds its rows to its viewport, the
        // graph is clipped to its canvas.
        match self.current_page {
            Page::Browse => {
                let (bx, by, bw, bh) = self.browse_split.left_rect();
                pages::browse::view(&mut self.browse, &mut pc, bx, by, bw, bh, self.select_mode, &mut self.ui_context);
            }
            Page::Network => {
                let (nx, ny, nw, nh) = self.network_split.left_rect();
                pages::network::view(&mut self.network, &self.browse, &mut pc, nx, ny, nw, nh, &mut self.ui_context);
            }
            Page::Space => {
                let (sx, sy, sw, sh) = self.space_split.left_rect();
                pages::space::view(&mut self.space, &self.browse, &mut pc, sx, sy, sw, sh, &mut self.ui_context);
            }
        }

        // The overlay: the prompt dialog. Its faces are what the text beneath
        // is cut away from. (The context menus are the toolkit's shared menu,
        // painted by display_list over the frame or hosted in a popup.)
        let mut over = cce_ui::scene::paint::PaintCtx::new();
        let mut faces: Vec<[f32; 4]> = Vec::new();
        // The prompt dialog: backdrop, panel, title, and its well and buttons
        // ON the panel. The well painted with the window's widgets until
        // 2026-10-12, under the panel: its caret and selection were covered,
        // and its text, cut away from the backdrop, was dropped.
        if let Some(PromptDialog { kind, textbox, cancel, ok, .. }) = &self.prompt_dialog {
            let (kind, textbox, cancel, ok) = (*kind, *textbox, *cancel, *ok);
            let r = prompt_rects(win_w, win_h);
            let (dialog_x, dialog_y, dialog_w, dialog_h) = (r.x, r.y, r.w, r.h);

            // Semi-transparent backdrop overlay
            pages::fill(&mut over, [0.02, 0.02, 0.03, 0.6], 0.0, 0.0, win_w, win_h);
            // Dialog panel border
            pages::fill(&mut over, [0.22, 0.22, 0.28, 1.0], dialog_x, dialog_y, dialog_w, dialog_h);
            // Dialog panel background
            pages::fill(&mut over, [0.08, 0.08, 0.12, 1.0], dialog_x + 1.0, dialog_y + 1.0, dialog_w - 2.0, dialog_h - 2.0);
            faces.push([0.0, 0.0, win_w, win_h]);
            // Raised dialog plate (control_relief styling).
            pages::raised(&mut over, dialog_x, dialog_y, dialog_w, dialog_h, 0.0);

            // Title and prompt, inset from the plate rim by the pane rung
            // (the same inset the well and buttons take in prompt_rects).
            let dialog_pad = cce_ui::layout::plate_padding();
            pages::text(&mut over, kind.title(), dialog_x + dialog_pad, dialog_y + dialog_pad, 14.0, [1.0, 1.0, 1.0, 1.0], None, None);
            // TODO(style): the prompt's 42px drop is the dialog's text rhythm.
            pages::text(&mut over, kind.prompt(), dialog_x + dialog_pad, dialog_y + 42.0, 11.0, [0.54, 0.54, 0.58, 1.0], None, None);

            let (tb_x, tb_y, tb_w, tb_h) = r.tb;
            pages::widget(&mut over, textbox, tb_x, tb_y, tb_w, tb_h, &mut self.ui_context);
            let (x, y, w, h) = r.cancel;
            pages::widget(&mut over, cancel, x, y, w, h, &mut self.ui_context);
            let (x, y, w, h) = r.ok;
            pages::widget(&mut over, ok, x, y, w, h, &mut self.ui_context);
        }

        let mut frame = pc.finish();
        occlude_text(&mut frame, &faces, (win_w, win_h));
        frame.items.extend(over.finish().items);
        self.frame = frame;
        self.ui_context.clear_dirty();
        self.needs_rebuild = false;
    }
}

// ── Input dispatch ──────────────────────────────────────────────────
//
// The pointer and key hooks' bodies; the trait's hooks call these, then
// sweep what the widgets reported (`after_dispatch`).

impl FilesystemApp {
    /// `handle_mouse_input` before the widget sweep (`after_dispatch`).
    fn mouse_input(&mut self, button: MouseButton, state: ElementState, pos: LogicalPosition, needs_rebuild: &mut bool) -> Option<Message> {
        if button != MouseButton::Left && button != MouseButton::Right {
            return None;
        }

        // The app's own menus (a row's, a tile's, a node's, the breadcrumb's,
        // the preview pane's corner menu) route BEFORE the toolkit's dispatch
        // below, and with no context, so the toolkit runs nothing for their
        // rows — not even the actions its label fallback would read into
        // "Copy Path" or "Delete" — and only records the pick, which the app
        // sends (`take_menu_pick`). A right press off the menu closes it and
        // falls through, so a right-click elsewhere opens the menu there.
        if self.app_menu_live() {
            use cce_ui::widget::context_menu as cm;
            let (px, py) = (pos.x, pos.y);
            if button == MouseButton::Right && state == ElementState::Pressed && !cm::hit_test(px, py) {
                cm::hide();
                self.app_menu = None;
            } else {
                match state {
                    // A row runs (and the menu closes); anywhere else the
                    // press closes it.
                    ElementState::Pressed => {
                        cm::mouse_input(button, state, px, py, None);
                    }
                    // On a release too: under a menu drawn in the window a
                    // press can be taken for a window move before it gets
                    // here, and then only the release arrives.
                    ElementState::Released if button == MouseButton::Left => {
                        if let Some(row) = cm::row_at(px, py) {
                            cm::activate(row, None);
                        }
                    }
                    _ => {}
                }
                *needs_rebuild = true;
                self.needs_rebuild = true;
                return self.take_menu_pick();
            }
        }

        // A left RELEASE on the preview pane's corner control opens its menu
        // (the designer also opens on release; presses do not reliably reach
        // this hook — see above). No press arming: the arming protocol
        // disambiguates click from dock-DRAG, and a single pane has nowhere
        // to dock.
        if button == MouseButton::Left && state == ElementState::Released {
            use cce_ui::widget::plate_dock as dock;
            let band = self.preview_dock_band();
            if let Some(c) = dock::corner_center(band, self.preview_dock.stubbed())
                && dock::corner_hit(c, pos.x, pos.y)
            {
                let rows = dock::standard_menu(self.preview_dock, false)
                    .into_iter()
                    .map(|(label, action)| (label, Some(Message::PreviewDock(action))))
                    .collect();
                let target = self.ui_context[self.paginator].base().id();
                self.show_app_menu(c.0 - dock::CORNER_R, c.1 + dock::CORNER_R, rows, 0, target, true);
                *needs_rebuild = true;
                self.needs_rebuild = true;
                return None;
            }
        }

        // The toolkit's shared context menu (breadcrumb, config-bound controls)
        // swallows the click — select or dismiss — before any widget routing.
        if cce_ui::widget::context_menu::is_visible() {
            if cce_ui::widget::context_menu::mouse_input(button, state, pos.x, pos.y, Some(&mut self.ui_context)) {
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
            return None;
        }

        if let Some(textbox) = self.prompt_dialog.as_ref().map(|d| d.textbox) {
            let r = prompt_rects(self.width as f32, self.height as f32);
            let (dialog_x, dialog_y, dialog_w, dialog_h) = (r.x, r.y, r.w, r.h);
            let (tb_x, tb_y, tb_w, tb_h) = r.tb;
            let ev = cce_ui::widget::Event::MouseButton { button, state, x: pos.x, y: pos.y, local_x: pos.x, local_y: pos.y };

            if state == ElementState::Pressed {
                let clicked_inside = pos.x >= dialog_x && pos.x <= dialog_x + dialog_w && pos.y >= dialog_y && pos.y <= dialog_y + dialog_h;
                if !clicked_inside {
                    self.ui_context[textbox].unfocus();
                    self.ui_context.clear_focus();
                    return Some(Message::PromptCancel);
                }

                if button == MouseButton::Left {
                    if pos.x >= tb_x && pos.x <= tb_x + tb_w && pos.y >= tb_y && pos.y <= tb_y + tb_h {
                        let root = textbox.id();
                        if self.ui_context.propagate_event(&ev, root) {
                            self.ui_context.set_focused_id(textbox.id());
                        }
                    } else if !self.route_buttons(&ev) {
                        // A press on the panel, off the well and the buttons
                        // (a button's press arms it; its release clicks it).
                        self.ui_context[textbox].unfocus();
                        self.ui_context.clear_focus();
                    }
                    *needs_rebuild = true;
                    self.needs_rebuild = true;
                }
            } else if button == MouseButton::Left {
                if self.ui_context[textbox].editing {
                    let root = textbox.id();
                    if self.ui_context.propagate_event(&ev, root) {
                        *needs_rebuild = true;
                        self.needs_rebuild = true;
                    }
                }
                // Cancel or the OK button, released on after a press on it.
                self.route_buttons(&ev);
                *needs_rebuild = true;
                self.needs_rebuild = true;
                return self.drain_widget_events(needs_rebuild);
            }
            return None;
        }

        if button == MouseButton::Right && state == ElementState::Pressed {
            let crumb = match self.current_page {
                Page::Browse => self.browse.breadcrumb,
                Page::Network => self.network.breadcrumb,
                Page::Space => self.space.breadcrumb,
            };
            if self.ui_context[crumb].hit_test(pos.x, pos.y, &self.ui_context) {
                let ev = cce_ui::widget::Event::MouseButton { button, state, x: pos.x, y: pos.y, local_x: pos.x, local_y: pos.y };
                let root = crumb.id();
                // The breadcrumb opens the toolkit's shared context menu itself
                // (open_context_menu → segment header + Copy Path), recording
                // the segment pressed. The app then shows that menu again,
                // where it stands and under the same header, with the view
                // switch added: the toolkit builds a breadcrumb's rows by type
                // name and takes none from the app.
                if self.ui_context.propagate_event(&ev, root) {
                    use cce_ui::widget::context_menu as cm;
                    let breadcrumb = &self.ui_context[crumb];
                    let seg = breadcrumb.right_clicked_seg.unwrap_or(breadcrumb.path.len());
                    let rows = breadcrumb_menu_rows(breadcrumb.path_to_seg(seg), self.current_page);
                    let header = cm::options().into_iter().next().unwrap_or_default();
                    let rows = std::iter::once((header, None)).chain(rows).collect();
                    self.show_app_menu(cm::x(), cm::y(), rows, 1, root, false);
                    *needs_rebuild = true;
                    self.needs_rebuild = true;
                    return None;
                }
            }

            // Row context menu: hit-test the list directly — rows stopped being
            // page_buttons when the List became a widget (Phase 6z), so the old
            // button probe never fired.
            {
                let entry_idx = if self.current_page == Page::Browse {
                    self.browse.list.row_at(pos.x, pos.y)
                } else {
                    None
                };

                if let Some(idx) = entry_idx {
                    if let Some(entry) = self.browse.entries.get(idx) {
                        let is_trash_dir = services::trash::is_trash_files_dir(&self.browse.current_dir);
                        let header = if entry.is_dir {
                            format!("[Directory] {}", entry.name)
                        } else {
                            format!("[File] {}", entry.name)
                        };

                        let mut options = vec![
                            (header, None),
                        ];

                        if entry.is_dir {
                            if is_project_dir(&entry.path) {
                                options.push(("Open Project".to_string(), Some(Message::SelectOpen)));
                                options.push(("Enter Directory".to_string(), Some(Message::Browse(pages::browse::BrowseMessage::NavigateToPath(entry.path.clone())))));
                            } else {
                                options.push(("Open".to_string(), Some(Message::Browse(pages::browse::BrowseMessage::NavigateTo(idx)))));
                            }
                        } else {
                            options.push(("Select".to_string(), Some(Message::Browse(pages::browse::BrowseMessage::SelectEntry(idx)))));
                        }

                        options.push(("Open with...".to_string(), Some(Message::PromptOpenWith(entry.path.clone()))));
                        if !is_trash_dir {
                            options.push(("Rename".to_string(), Some(Message::PromptRename(entry.path.clone()))));
                        }

                        if is_trash_dir {
                            options.push(("Restore".to_string(), Some(Message::Browse(pages::browse::BrowseMessage::RestoreEntry(idx)))));
                            options.push(("Delete Permanently".to_string(), Some(Message::Browse(pages::browse::BrowseMessage::DeleteEntryPermanent(idx)))));
                            options.push(("Empty Trash".to_string(), Some(Message::Browse(pages::browse::BrowseMessage::EmptyTrash))));
                        } else {
                            options.push(("Delete".to_string(), Some(Message::Browse(pages::browse::BrowseMessage::DeleteEntry(idx)))));
                            options.push(("New Folder".to_string(), Some(Message::Browse(pages::browse::BrowseMessage::NewFolder))));
                            if let Some(trash_files) = services::trash::files_dir() {
                                options.push(("Open Trash".to_string(), Some(Message::Browse(pages::browse::BrowseMessage::NavigateToPath(trash_files)))));
                            }
                        }

                        self.open_context_menu(pos.x, pos.y, options);
                        *needs_rebuild = true;
                        self.needs_rebuild = true;
                        return None;
                    }
                } else if let Some(tile) = (self.current_page == Page::Space)
                    .then(|| self.space.tile_at(pos.x, pos.y))
                    .flatten()
                    .and_then(|idx| self.space.tiles.get(idx))
                {
                    let options = space_tile_menu(tile);
                    self.open_context_menu(pos.x, pos.y, options);
                    *needs_rebuild = true;
                    self.needs_rebuild = true;
                    return None;
                } else if let Some(options) = (self.current_page == Page::Network)
                    .then(|| {
                        // Only a node inside the pane: a panned graph's nodes
                        // run on under the breadcrumb and the preview.
                        let (x, y, w, h) = self.network.laid_out_for;
                        let inside = pos.x >= x && pos.x < x + w && pos.y >= y && pos.y < y + h;
                        inside.then(|| self.ui_context[self.network.graph].inner().node_at(pos.x, pos.y)).flatten()
                    })
                    .flatten()
                    .and_then(|idx| graph_node_menu(idx, &self.browse.current_dir, &self.browse.entries))
                {
                    self.open_context_menu(pos.x, pos.y, options);
                    *needs_rebuild = true;
                    self.needs_rebuild = true;
                    return None;
                } else if self.current_page == Page::Browse
                    && self.browse.list.hit(pos.x, pos.y)
                    && pos.y <= self.browse.list.y + self.browse.list.viewport_h
                    && !services::trash::is_trash_files_dir(&self.browse.current_dir)
                {
                    // Empty space below the rows: a menu for the directory itself.
                    let name = self
                        .browse
                        .current_dir
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "/".to_string());
                    let options = vec![
                        (format!("[Directory] {}", name), None),
                        ("New Folder".to_string(), Some(Message::Browse(pages::browse::BrowseMessage::NewFolder))),
                    ];
                    self.open_context_menu(pos.x, pos.y, options);
                    *needs_rebuild = true;
                    self.needs_rebuild = true;
                    return None;
                }
            }
        }

        let mut changed = false;

        log::debug!("MOUSE INPUT: {:?} {:?} pos=({}, {})", button, state, pos.x, pos.y);
        let ev = cce_ui::widget::Event::MouseButton { button, state, x: pos.x, y: pos.y, local_x: pos.x, local_y: pos.y };
        let menu_match = !self.select_mode && {
            // Self-routing composite: handle_event, not propagate (see pointer move).
            self.ui_context.lend_h(self.paginator, |w, ctx| w.handle_event(&ev, ctx)).unwrap_or(false)
        };
        if menu_match {
            if let Some((idx, _)) = self.ui_context[self.paginator].menu_click()
                && idx < Page::ALL.len()
            {
                self.ui_context.clear_focus();
                self.current_page = Page::ALL[idx];
            }
            *needs_rebuild = true;
            self.needs_rebuild = true;
            return None;
        }

        // The dissolved splitter's divider: a left press grabs it (stealing keyboard
        // focus like the legacy ctx.set_focused_ptr / release's clear_focus pair did),
        // a release ends the drag. A collapsed preview owns its column width;
        // the divider is not draggable until the pane expands.
        if button == MouseButton::Left && !self.preview_dock.collapsed {
            let split = match self.current_page {
                Page::Browse => &mut self.browse_split,
                Page::Network => &mut self.network_split,
                Page::Space => &mut self.space_split,
            };
            if state == ElementState::Pressed {
                if split.press(pos.x, pos.y) {
                    self.ui_context.clear_focus();
                    *needs_rebuild = true;
                    self.needs_rebuild = true;
                    return None;
                }
            } else if split.release() {
                *needs_rebuild = true;
                self.needs_rebuild = true;
                return None;
            }
        }

        // Well focus follows the left click, keyed on RELEASE like the
        // plate-dock control above: the routed-widget path consumes left
        // PRESSES before this hook, so only releases reliably arrive here.
        // Every overlay above the page (plate-dock menu, context menu, dialog,
        // divider grab) has already returned, so a release reaching
        // here is on page content: move the accent ring to the well it landed
        // in, then let the release do its normal work. Releases outside any
        // well (toolbar, plate) change nothing.
        if button == MouseButton::Left && state == ElementState::Released {
            let new_focus = if !self.preview_dock.collapsed
                && let Some(well) = self.preview.well_at(pos.x, pos.y)
            {
                Some(match well {
                    cce_files::preview_pane::PreviewWell::Top => FocusedWell::PreviewTop,
                    cce_files::preview_pane::PreviewWell::Bottom => FocusedWell::PreviewBottom,
                })
            } else {
                let split = match self.current_page {
                    Page::Browse => &self.browse_split,
                    Page::Network => &self.network_split,
                    Page::Space => &self.space_split,
                };
                let (lx, ly, lw, lh) = split.left_rect();
                if pos.x >= lx && pos.x <= lx + lw && pos.y >= ly && pos.y <= ly + lh {
                    Some(FocusedWell::Content)
                } else {
                    None
                }
            };
            if let Some(f) = new_focus
                && f != self.focused_well
            {
                self.focused_well = f;
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
        }

        if self.current_page == Page::Browse {
            if self.select_mode
                && self.ui_context.propagate_event(&ev, self.browse.save_name_box.id())
            {
                if state == ElementState::Pressed {
                    self.ui_context.set_focused_id(self.browse.save_name_box.id());
                }
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
            if self.browse.search_visible
                && button == MouseButton::Left
                && self.ui_context.propagate_event(&ev, self.browse.search_box.id())
            {
                self.ui_context.set_focused_id(self.browse.search_box.id());
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
            if button == MouseButton::Left
                && self.browse.list.mouse_input(state == ElementState::Pressed, pos.x, pos.y)
            {
                *needs_rebuild = true;
                self.needs_rebuild = true;
                if let Some(idx) = self.browse.list.take_double_click() {
                    return Some(Message::Browse(pages::browse::BrowseMessage::NavigateTo(idx)));
                }
                if let Some(idx) = self.browse.list.take_click() {
                    return Some(Message::Browse(pages::browse::BrowseMessage::SelectEntry(idx)));
                }
            }
            if button == MouseButton::Left && state == ElementState::Pressed
                && self.ui_context[self.browse.breadcrumb].hit_test(pos.x, pos.y, &self.ui_context)
                && self.ui_context.propagate_event(&ev, self.browse.breadcrumb.id())
                && let Some(seg) = self.ui_context[self.browse.breadcrumb].path_click()
            {
                let target_path = pages::browse::path_to_segment(&self.browse.current_dir, seg);
                self.fs_service.send(services::fs::FsRequest::ReadDirectory(target_path));
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
        } else if self.current_page == Page::Network {
            if button == MouseButton::Left {
                if state == ElementState::Pressed {
                    if self.ui_context[self.network.breadcrumb].hit_test(pos.x, pos.y, &self.ui_context) {
                        if self.ui_context.propagate_event(&ev, self.network.breadcrumb.id())
                            && let Some(seg) = self.ui_context[self.network.breadcrumb].path_click()
                        {
                            let target_path = pages::browse::path_to_segment(&self.browse.current_dir, seg);
                            self.fs_service.send(services::fs::FsRequest::ReadDirectory(target_path));
                            changed = true;
                        }
                    } else if self.ui_context.propagate_event(&ev, self.network.graph.id()) {
                        // Routed press: a node grab records the drag target; the router's
                        // DragStart replaces the immediate drag_begin (3px threshold).
                        changed = true;
                    }
                } else if state == ElementState::Released {
                    // The router delivers DragEnd (commit) before the release reaches
                    // Graph; a committed drag leaves the release arm inert.
                    let was_dragging = self.ui_context.is_dragging;
                    let root = self.network.graph.id();
                    if self.ui_context.propagate_event(&ev, root) || was_dragging {
                        changed = true;
                    }
                }
            }

            // Sync selection from graph to browse state
            let has_parent = self.browse.current_dir.parent().is_some();
            let offset = if has_parent { 2 } else { 1 };
            if let Some(node_sel) = self.ui_context[self.network.graph].selected_node() {
                if node_sel >= offset {
                    let entry_idx = node_sel - offset;
                    if self.browse.selected != Some(entry_idx) {
                        self.browse.selected = Some(entry_idx);
                        if let Some(entry) = self.browse.entries.get(entry_idx) {
                            if self.select_mode {
                                self.ui_context[self.browse.save_name_box].text = entry.name.clone();
                                if self.ui_context[self.browse.save_name_box].editing {
                                    self.ui_context[self.browse.save_name_box].edit_buffer = entry.name.clone();
                                }
                            }
                            let path = entry.path.clone();
                            self.request_preview(path, false);
                        }
                        changed = true;
                    }
                } else {
                    if self.browse.selected.is_some() {
                        self.browse.selected = None;
                        self.clear_preview();
                        changed = true;
                    }
                }
            } else {
                if self.browse.selected.is_some() {
                    self.browse.selected = None;
                    self.clear_preview();
                    changed = true;
                }
            }

            // Sync double click navigation
            if let Some(dbl_idx) = self.ui_context[self.network.graph].double_clicked_node() {
                self.ui_context[self.network.graph].clear_double_clicked_node();
                if has_parent && dbl_idx == 0 {
                    if let Some(parent) = self.browse.current_dir.parent() {
                        let parent_path = parent.to_path_buf();
                        self.fs_service.send(services::fs::FsRequest::ReadDirectory(parent_path));
                        changed = true;
                    }
                } else if dbl_idx >= offset {
                    let entry_idx = dbl_idx - offset;
                    if let Some(entry) = self.browse.entries.get(entry_idx) {
                        if entry.is_dir && !is_project_dir(&entry.path) {
                            let path = entry.path.clone();
                            self.fs_service.send(services::fs::FsRequest::ReadDirectory(path));
                            changed = true;
                        } else if self.select_mode {
                            self.ui_context[self.browse.save_name_box].text = entry.name.clone();
                            if self.ui_context[self.browse.save_name_box].editing {
                                self.ui_context[self.browse.save_name_box].edit_buffer = entry.name.clone();
                            }
                            return Some(Message::SelectOpen);
                        }
                    }
                }
            }

            if changed {
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
        } else if self.current_page == Page::Space {
            if button == MouseButton::Left && state == ElementState::Pressed {
                if self.ui_context[self.space.breadcrumb].hit_test(pos.x, pos.y, &self.ui_context) {
                    if self.ui_context.propagate_event(&ev, self.space.breadcrumb.id())
                        && let Some(seg) = self.ui_context[self.space.breadcrumb].path_click()
                    {
                        let target_path = pages::browse::path_to_segment(&self.browse.current_dir, seg);
                        self.fs_service.send(services::fs::FsRequest::ReadDirectory(target_path));
                        changed = true;
                    }
                } else if let Some(idx) = self.space.tile_at(pos.x, pos.y) {
                    let tile_path = self.space.tiles[idx].path.clone();
                    let is_dir = self.space.tiles[idx].is_dir;

                    // Same temporal double-click as the Browse list — the
                    // toolkit does not deliver a double-click event.
                    let now = std::time::Instant::now();
                    let is_double = self.last_space_path.as_ref() == Some(&tile_path)
                        && now.duration_since(self.last_space_click_time).as_millis() < 500;
                    self.last_space_click_time = now;
                    self.last_space_path = Some(tile_path.clone());

                    if is_double {
                        if is_dir {
                            // Navigating re-roots the map: ReadDirectory moves
                            // current_dir, and ensure_space_scan rescans it.
                            self.fs_service.send(services::fs::FsRequest::ReadDirectory(tile_path));
                        } else {
                            services::fs::open_file(&tile_path);
                        }
                    } else {
                        self.space.selected_path = Some(tile_path.clone());
                        self.request_preview(tile_path, false);
                    }
                    changed = true;
                }
            }
            if changed {
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
        }

        if state == ElementState::Pressed {
            let clicked_search = self.current_page == Page::Browse && self.browse.search_visible && self.ui_context[self.browse.search_box].hit_test(pos.x, pos.y, &self.ui_context);
            let clicked_save_name = self.select_mode && self.current_page == Page::Browse && self.ui_context[self.browse.save_name_box].hit_test(pos.x, pos.y, &self.ui_context);
            if !clicked_search {
                self.ui_context.unfocus_id(self.browse.search_box.id());
            }
            if !clicked_save_name {
                self.ui_context.unfocus_id(self.browse.save_name_box.id());
            }
            if !clicked_search && !clicked_save_name {
                self.ui_context.clear_focus();
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
        }

        // The chooser's buttons: a press arms the one under it, the release
        // on it clicks it.
        if button == MouseButton::Left {
            let ev = cce_ui::widget::Event::MouseButton { button, state, x: pos.x, y: pos.y, local_x: pos.x, local_y: pos.y };
            if self.route_buttons(&ev) {
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
            if let Some(msg) = self.drain_widget_events(needs_rebuild) {
                return Some(msg);
            }
        }

        None
    }

    /// `handle_key_input` before the widget sweep (`after_dispatch`).
    fn key_input(&mut self, event: &KeyEvent, needs_rebuild: &mut bool) -> Option<Message> {
        if event.state != ElementState::Pressed {
            return None;
        }

        // The app's own menu, while it is up, takes the keys a menu reads:
        // Up / Down walk its highlight, Enter runs the highlighted row.
        // (Escape never gets here: the runner closes the menu with it.)
        if self.app_menu_live() {
            use cce_ui::widget::context_menu as cm;
            use cce_ui::widget::{Key, NamedKey};
            match event.logical_key {
                Key::Named(NamedKey::ArrowDown) | Key::Named(NamedKey::ArrowUp) => {
                    let dir = if event.logical_key == Key::Named(NamedKey::ArrowDown) { 1 } else { -1 };
                    cm::step_hovered(dir);
                    *needs_rebuild = true;
                    self.needs_rebuild = true;
                    return None;
                }
                Key::Named(NamedKey::Enter) => {
                    cm::activate_hovered(None);
                    *needs_rebuild = true;
                    self.needs_rebuild = true;
                    return self.take_menu_pick();
                }
                _ => {}
            }
        }

        // A button holding the keyboard (Tab walks to it) takes Enter / Space
        // ahead of the page's own keys and the prompt's Enter-submits.
        if let Some(focused) = self.ui_context.focused_widget
            && self.live_buttons().iter().any(|(h, _)| h.id() == focused)
        {
            let kev = cce_ui::widget::Event::KeyInput(event.clone());
            if self.ui_context.propagate_event(&kev, focused) {
                *needs_rebuild = true;
                self.needs_rebuild = true;
                return self.drain_widget_events(needs_rebuild);
            }
        }

        if let Some(PromptDialog { textbox, .. }) = &mut self.prompt_dialog {
            if event.logical_key == cce_ui::widget::Key::Named(cce_ui::widget::NamedKey::Enter) {
                self.ui_context[*textbox].unfocus();
                self.ui_context.clear_focus();
                return Some(Message::PromptSubmit);
            }
            if event.logical_key == cce_ui::widget::Key::Named(cce_ui::widget::NamedKey::Escape) {
                self.ui_context[*textbox].unfocus();
                self.ui_context.clear_focus();
                return Some(Message::PromptCancel);
            }
            let kev = cce_ui::widget::Event::KeyInput(event.clone());
            let root = textbox.id();
            if self.ui_context.propagate_event(&kev, root) {
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
            return None;
        }

        if self.current_page == Page::Network {
            let kev = cce_ui::widget::Event::KeyInput(event.clone());
            let root = self.network.graph.id();
            if self.ui_context.propagate_event(&kev, root) {
                *needs_rebuild = true;
                self.needs_rebuild = true;
                return None;
            }
        }

        // The dissolved List's search keys, app-side: the open shortcut shows the strip
        // and focuses the box; the close shortcut hides it and clears the filter (the
        // legacy List set just_changed after clearing, which surfaced as an empty
        // SearchChanged); anything else goes to the box while it is open.
        if self.current_page == Page::Browse {
            if !self.browse.search_visible {
                let open_key = cce_ui::color::list_open_search_key();
                if cce_ui::widget::match_key_shortcut(event, &open_key) {
                    self.browse.search_visible = true;
                    self.ui_context.focus_id(self.browse.search_box.id());
                    self.ui_context.set_focused_id(self.browse.search_box.id());
                    *needs_rebuild = true;
                    self.needs_rebuild = true;
                    return None;
                }
            } else {
                let close_key = cce_ui::color::list_close_search_key();
                if cce_ui::widget::match_key_shortcut(event, &close_key) {
                    self.browse.search_visible = false;
                    self.ui_context.unfocus_id(self.browse.search_box.id());
                    self.ui_context.clear_focus();
                    // Emptied outright, the buffer with the text: the box
                    // reports an edit wherever its text and buffer differ,
                    // and a query left in the buffer would filter the list
                    // again at the next sweep, the strip hidden.
                    let search = &mut self.ui_context[self.browse.search_box];
                    search.text.clear();
                    search.edit_buffer.clear();
                    *needs_rebuild = true;
                    self.needs_rebuild = true;
                    return Some(Message::Browse(pages::browse::BrowseMessage::SearchChanged(String::new())));
                }
                let kev = cce_ui::widget::Event::KeyInput(event.clone());
                if self.ui_context.propagate_event(&kev, self.browse.search_box.id()) {
                    *needs_rebuild = true;
                    self.needs_rebuild = true;
                    // An edit comes back as SearchChanged.
                    return self.drain_widget_events(needs_rebuild);
                }
            }
        }

        // If the save_name_box is focused, forward key inputs to it
        if self.select_mode && self.ui_context[self.browse.save_name_box].editing {
            let kev = cce_ui::widget::Event::KeyInput(event.clone());
            if self.ui_context.propagate_event(&kev, self.browse.save_name_box.id()) {
                *needs_rebuild = true;
                self.needs_rebuild = true;
                if event.logical_key == cce_ui::widget::Key::Named(cce_ui::widget::NamedKey::Enter) {
                    self.ui_context.unfocus_id(self.browse.save_name_box.id());
                    self.ui_context.clear_focus();
                    return Some(Message::SelectOpen);
                }
                return None;
            }
        }

        // Global key navigation. Arrow keys, Backspace, and Escape are fixed;
        // the rest resolve through input.kdl (see BrowseKeys).
        if self.current_page == Page::Browse {
            match &event.logical_key {
                cce_ui::widget::Key::Named(cce_ui::widget::NamedKey::ArrowUp) => {
                    if let Some(index) = pages::browse::next_selection_index(&self.browse, pages::browse::BrowseNavigation::Up) {
                        return Some(Message::Browse(pages::browse::BrowseMessage::SelectEntry(index)));
                    }
                }
                cce_ui::widget::Key::Named(cce_ui::widget::NamedKey::ArrowDown) => {
                    if let Some(index) = pages::browse::next_selection_index(&self.browse, pages::browse::BrowseNavigation::Down) {
                        return Some(Message::Browse(pages::browse::BrowseMessage::SelectEntry(index)));
                    }
                }
                cce_ui::widget::Key::Named(cce_ui::widget::NamedKey::Escape) => {
                    if self.select_mode {
                        std::process::exit(1);
                    }
                }
                cce_ui::widget::Key::Named(cce_ui::widget::NamedKey::Backspace) => {
                    if let Some(parent) = self.browse.current_dir.parent() {
                        return Some(Message::Browse(pages::browse::BrowseMessage::NavigateToPath(parent.to_path_buf())));
                    }
                }
                _ => {}
            }

            let m = |chord: &str| cce_ui::widget::match_key_shortcut(event, chord);
            if m(&self.keys.open_file) {
                if let Some(idx) = self.browse.selected {
                    if let Some(entry) = self.browse.entries.get(idx) {
                        if entry.is_dir && !is_project_dir(&entry.path) {
                            return Some(Message::Browse(pages::browse::BrowseMessage::NavigateTo(idx)));
                        } else {
                            return Some(Message::SelectOpen);
                        }
                    }
                } else if self.save_mode {
                    return Some(Message::SelectOpen);
                }
            } else if m(&self.keys.select_next) {
                if let Some(index) = pages::browse::next_selection_index(&self.browse, pages::browse::BrowseNavigation::Down) {
                    return Some(Message::Browse(pages::browse::BrowseMessage::SelectEntry(index)));
                }
            } else if m(&self.keys.select_prev) {
                if let Some(index) = pages::browse::next_selection_index(&self.browse, pages::browse::BrowseNavigation::Up) {
                    return Some(Message::Browse(pages::browse::BrowseMessage::SelectEntry(index)));
                }
            } else if m(&self.keys.enter_dir) {
                if let Some(idx) = self.browse.selected
                    && let Some(entry) = self.browse.entries.get(idx)
                    && entry.is_dir && !is_project_dir(&entry.path)
                {
                    return Some(Message::Browse(pages::browse::BrowseMessage::NavigateTo(idx)));
                }
            } else if m(&self.keys.parent_dir) {
                if let Some(parent) = self.browse.current_dir.parent() {
                    return Some(Message::Browse(pages::browse::BrowseMessage::NavigateToPath(parent.to_path_buf())));
                }
            } else if m(&self.keys.delete_entry) {
                if let Some(idx) = self.browse.selected {
                    return Some(Message::Browse(pages::browse::BrowseMessage::DeleteEntry(idx)));
                }
            } else if m(&self.keys.toggle_hidden) {
                return Some(Message::Browse(pages::browse::BrowseMessage::ToggleHidden));
            }
        }

        None
    }
}

// ── Application Trait Implementation ────────────────────────────────

impl Application for FilesystemApp {
    type Message = Message;

    fn ui_context(&self) -> Option<&cce_ui::context::UiContext> {
        Some(&self.ui_context)
    }

    // The engine ticks the exposed context each loop — this is what drives the
    // registered widgets' animation frames.
    fn ui_context_mut(&mut self) -> Option<&mut cce_ui::context::UiContext> {
        Some(&mut self.ui_context)
    }

    fn is_movable_root_plate_at(&self, px: f32, py: f32) -> bool {
        // 1. If dialog is open, do not drag
        if self.prompt_dialog.is_some() {
            return false;
        }
        // 2. If a context menu is up, do not drag: the press is the menu's
        // (it runs a row or closes the menu). Without this a press over the
        // plate under a menu drawn in the window (no popup) moved the window
        // and only its release reached the app.
        if cce_ui::widget::context_menu::is_visible() {
            return false;
        }
        // 3. If in the sidebar area (when sidebar is active), do not drag
        if false {
            let sidebar_w = self.ui_context[self.paginator].sidebar_w();
            if px <= sidebar_w {
                return false;
            }
        }
        // 4. If over a button (the chooser's), do not drag: the press is its.
        if self.over_button(px, py) {
            return false;
        }
        if self.current_page == Page::Browse {
            if self.browse_split.dragging || self.browse_split.hovered {
                return false;
            }
            // The dissolved List blocked window drags via its registered ScrollBox
            // (blocks_root_plate_drag); veto app-side now or every row press starts a
            // compositor window move and the app never sees it.
            let l = &self.browse.list;
            if px >= l.x && px <= l.x + l.w && py >= l.y && py <= l.y + l.h {
                return false;
            }
        } else if self.current_page == Page::Network {
            if self.network_split.dragging || self.network_split.hovered {
                return false;
            }
        } else if self.current_page == Page::Space {
            if self.space_split.dragging || self.space_split.hovered {
                return false;
            }
            // Same reasoning as the List above: without this veto every press
            // on a tile starts a compositor window move and the app never sees
            // the click.
            let (mx, my, mw, mh) = self.space.map_rect;
            if px >= mx && px <= mx + mw && py >= my && py <= my + mh {
                return false;
            }
        }
        // 5. root plate container dissolved: the surface itself is the movable plate; drag
        // anywhere a drag-blocking widget isn't.
        self.ui_context.drag_allowed_at(px, py)
    }

    fn create(sender: cce_ui::engine::AppSender<Self::Message>) -> Self {
        // The app keeps calloop's sender; `AppSender` converts into it.
        let sender: calloop::channel::Sender<Self::Message> = sender.into();
        // Parse command line arguments
        let args: Vec<String> = std::env::args().collect();
        let select_directory = args.iter().any(|arg| arg == "--select-dir");
        let save_mode = args.iter().any(|arg| arg == "--save");
        let select_mode = save_mode || args.iter().any(|arg| arg == "--select" || arg == "--select-dir");

        // A positional path opens there instead of the remembered directory.
        // This is what `Exec=cce-files %f` passes as the inode/directory
        // handler; without it the desktop entry could claim the type but
        // always land on the last-visited dir, ignoring the folder clicked.
        // Flags are skipped rather than just args[1], so `--select /tmp`
        // works. A path that does not resolve falls through to the last dir,
        // and a file opens its containing directory.
        let start_dir = args
            .iter()
            .skip(1)
            .find(|a| !a.starts_with("--"))
            .and_then(|a| std::fs::canonicalize(a).ok())
            .and_then(|p| if p.is_dir() { Some(p) } else { p.parent().map(|q| q.to_path_buf()) });

        cce_ui::scale::set_scale_factor(1.0);

        // The context owns the widgets; the app keeps their handles.
        let mut ui_context = cce_ui::context::UiContext::new();
        let browse = pages::browse::BrowseState::new(&mut ui_context);
        let _current_dir = browse.current_dir.clone();

        let pages_names = Page::ALL.iter().map(|p| p.label().to_string()).collect::<Vec<_>>();
        let paginator = cce_ui::widget::Paginator::new(pages_names);
        let fs_service = services::fs::FsService::new(sender.clone());
        let chooser = select_mode.then(|| ChooserButtons {
            cancel: ui_context.insert(plain_button("Cancel")),
            confirm: ui_context.insert(plain_button(if save_mode { "Save" } else { "Select" })),
        });
        let initial_w = if select_mode { 900 } else { 1200 };
        let initial_h = if select_mode { 500 } else { 720 };
        let app = Self {
            current_page: Page::Browse,
            browse,
            network: pages::network::NetworkState::new(&mut ui_context),
            space: pages::space::SpaceState::new(&mut ui_context),
            preview: Default::default(),
            preview_requested: None,
            preview_generation: 0,
            focused_well: FocusedWell::Content,
            preview_dock: Default::default(),
            preview_prior_fracs: None,
            app_menu: None,
            select_mode,
            select_directory,
            save_mode,
            frame: Default::default(),
            font_system: cce_ui::create_font_system(),
            needs_rebuild: true,
            text_claim: None,
            width: initial_w,
            height: initial_h,
            scale_factor: 1.0,
            chooser,
            cursor_x: 0.0,
            cursor_y: 0.0,
            paginator: ui_context.insert(paginator),
            just_initialized: true,
            ui_context,
            watcher: None,
            fs_service,
            seen_renderer: false,
            sender: sender.clone(),
            prompt_dialog: None,
            browse_split: SplitPane::new(0.49, 100.0, 100.0, cce_ui::layout::root_plate_gap()),
            network_split: SplitPane::new(0.49, 100.0, 100.0, cce_ui::layout::root_plate_gap()),
            space_split: SplitPane::new(0.49, 100.0, 100.0, cce_ui::layout::root_plate_gap()),
            last_space_click_time: std::time::Instant::now(),
            last_space_path: None,
            last_click_time: std::time::Instant::now(),
            last_clicked_idx: None,
            keys: BrowseKeys::load(),
        };

        
        // Start initial directory loading via FsService. ReadDirectory lands in
        // the same DirectoryLoaded handler ReadLastDir eventually reaches, so an
        // argv path just skips the restore step; current_dir and the breadcrumb
        // are set when the load completes either way.
        match start_dir {
            Some(dir) => app.fs_service.send(services::fs::FsRequest::ReadDirectory(dir)),
            None => app.fs_service.send(services::fs::FsRequest::ReadLastDir),
        }

        // NOTE: do not call rebuild_layout() here. This value is moved out of new()
        // into the engine, which changes its address; the container/splitter widgets
        // capture raw self-pointers during rebuild, so the first rebuild must happen
        // after the move (the engine triggers it on the first frame via needs_rebuild).
        app
    }

    fn settings(&self) -> WindowSettings {
        if self.select_mode {
            let title = if self.save_mode {
                "Save File"
            } else if self.select_directory {
                "Select Directory"
            } else {
                "Select File"
            };
            WindowSettings {
                title: title.to_string(),
                // The cce- prefix matters: the compositor's is_cce_app gate keys
                // blur and the root plate corner radius off it.
                app_id: "cce-filesystem-chooser".to_string(),
                width: 900,
                height: 500,
                fullscreen: false,
                min_size: Some((800, 400)),
            }
        } else {
            WindowSettings {
                title: "Files".to_string(),
                app_id: "cce-files".to_string(),
                width: 1200,
                height: 720,
                fullscreen: false,
                min_size: Some((1020, 600)),
            }
        }
    }

    fn update(&mut self, msg: Self::Message, needs_rebuild: &mut bool, _exit: &mut bool) {
        match msg {
            Message::SwitchPage(page) => {
                self.current_page = page;
                let page_idx = Page::ALL.iter().position(|&p| p == page).unwrap_or(0);
                self.ui_context[self.paginator].set_selected_page(page_idx);
                // Switching to Space is what triggers the first scan — it is
                // far too expensive to run for a page nobody is looking at.
                self.ensure_space_scan();
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
            Message::Browse(msg) => {
                let mut is_file_double_click = false;
                if let pages::browse::BrowseMessage::NavigateTo(idx) = &msg {
                    let now = std::time::Instant::now();
                    if self.last_clicked_idx == Some(*idx) && now.duration_since(self.last_click_time).as_millis() < 500 {
                        is_file_double_click = self.browse.entries.get(*idx).map(|e| !e.is_dir || (self.select_mode && !self.select_directory && is_project_dir(&e.path))).unwrap_or(false);
                    }
                    self.last_click_time = now;
                    self.last_clicked_idx = Some(*idx);
                } else if let pages::browse::BrowseMessage::SelectEntry(idx) = &msg {
                    let now = std::time::Instant::now();
                    self.last_click_time = now;
                    self.last_clicked_idx = Some(*idx);
                }

                let is_directory_loaded = match &msg {
                    pages::browse::BrowseMessage::DirectoryLoaded(path, _) => Some(path.clone()),
                    _ => None,
                };

                if let Some(req) = pages::browse::update(&mut self.browse, msg, &mut self.ui_context) {
                    self.fs_service.send(req);
                }

                if let Some(path) = is_directory_loaded {
                    self.start_watching(path);
                    // Navigating re-scans the new subtree when Space is up.
                    self.ensure_space_scan();
                }

                // Whatever the message, the preview follows the selection —
                // and only re-reads when it, or the selected file, changed.
                let selected_path = self.browse.selected_path();
                if let Some(path) = selected_path {
                    self.request_preview(path, false);
                } else {
                    self.clear_preview();
                }

                if let Some(idx) = self.browse.selected {
                    if let Some(entry) = self.browse.entries.get(idx) && self.select_mode {
                        self.ui_context[self.browse.save_name_box].text = entry.name.clone();
                        if self.ui_context[self.browse.save_name_box].editing {
                            self.ui_context[self.browse.save_name_box].edit_buffer = entry.name.clone();
                        }
                    }
                } else {
                    if self.select_mode {
                        self.ui_context[self.browse.save_name_box].text.clear();
                        if self.ui_context[self.browse.save_name_box].editing {
                            self.ui_context[self.browse.save_name_box].edit_buffer.clear();
                        }
                    }
                }

                *needs_rebuild = true;
                self.needs_rebuild = true;

                if is_file_double_click {
                    self.update(Message::SelectOpen, needs_rebuild, _exit);
                }
            }
            Message::Preview(msg) => {
                if let pages::preview::PreviewMessage::PreviewLoaded { generation, .. } = &msg {
                    // A read the selection has moved past: a slow decode
                    // must not paint over the file now selected.
                    if *generation != self.preview_generation {
                        return;
                    }
                }
                pages::preview::update(&mut self.preview, msg);
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
            Message::Space(msg) => {
                pages::space::update(&mut self.space, msg);
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
            Message::SelectOpen => {
                if self.select_directory {
                    let selected_path = self.browse.selected_path();
                    let path = selected_path.filter(|p| p.is_dir()).unwrap_or_else(|| self.browse.current_dir.clone());
                    println!("{}", path.display());
                    std::process::exit(0);
                } else {
                    let filename = if self.ui_context[self.browse.save_name_box].editing {
                        self.ui_context[self.browse.save_name_box].edit_buffer.trim()
                    } else {
                        self.ui_context[self.browse.save_name_box].text.trim()
                    };
                    if !filename.is_empty() {
                        let path = self.browse.current_dir.join(filename);
                        if path.is_dir() && !is_project_dir(&path) {
                            self.fs_service.send(services::fs::FsRequest::ReadDirectory(path));
                        } else {
                            if self.select_mode {
                                println!("{}", path.display());
                                std::process::exit(0);
                            } else {
                                crate::services::fs::open_file(&path);
                            }
                        }
                    } else {
                        let selected_path = self.browse.selected_path();
                        if let Some(path) = selected_path {
                            if path.is_dir() && !is_project_dir(&path) {
                                self.fs_service.send(services::fs::FsRequest::ReadDirectory(path));
                            } else {
                                if self.select_mode {
                                    println!("{}", path.display());
                                    std::process::exit(0);
                                } else {
                                    crate::services::fs::open_file(&path);
                                }
                            }
                        }
                    }

                }
            }
            Message::SelectCancel => {
                std::process::exit(1);
            }
            Message::PromptOpenWith(path) => {
                let default_cmd = if let Some(mime) = crate::services::fs::get_mime_type(&path) {
                    if let Some((_, cmd)) = crate::services::fs::get_default_application(&mime) {
                        cmd
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                };
                let mut tb = cce_ui::widget::TextBox::new(default_cmd)
                    .with_max_width(None)
                    .with_placeholder("Program/Command");
                tb.focus();
                let textbox = self.ui_context.insert(tb);
                self.ui_context.set_focused_id(textbox.id());
                let dialog = self.new_prompt(PromptKind::OpenWith, path, textbox);
                self.set_prompt(Some(dialog));
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
            Message::PromptRename(path) => {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                // Preselect the stem, not the extension, so typing replaces
                // just the part people usually mean to change.
                let stem_chars = if path.is_dir() {
                    name.chars().count()
                } else {
                    path.file_stem().map(|s| s.to_string_lossy().chars().count()).unwrap_or(0)
                };
                let mut tb = cce_ui::widget::TextBox::new(name).with_max_width(None);
                tb.focus();
                tb.cursor_idx = stem_chars;
                tb.select_anchor = Some(0);
                tb.all_selected = false;
                tb.sync_editor_state();
                let textbox = self.ui_context.insert(tb);
                self.ui_context.set_focused_id(textbox.id());
                let dialog = self.new_prompt(PromptKind::Rename, path, textbox);
                self.set_prompt(Some(dialog));
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
            Message::PromptSubmit => {
                if let Some(PromptDialog { kind, path, textbox, cancel, ok }) = self.prompt_dialog.take() {
                    self.ui_context.remove(cancel);
                    self.ui_context.remove(ok);
                    let value = self.ui_context.remove(textbox).map_or_else(String::new, |textbox| {
                        if textbox.editing {
                            textbox.edit_buffer.trim().to_string()
                        } else {
                            textbox.text.trim().to_string()
                        }
                    });
                    match kind {
                        PromptKind::OpenWith => {
                            if !value.is_empty() {
                                crate::services::fs::spawn_command_for_path(&value, &path);
                            }
                        }
                        PromptKind::Rename => {
                            self.update(
                                Message::Browse(pages::browse::BrowseMessage::RenameEntry(path, value)),
                                needs_rebuild,
                                _exit,
                            );
                        }
                    }
                }
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
            Message::PromptCancel => {
                self.set_prompt(None);
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
            Message::CopyPath(path) => {
                cce_ui::widget::clipboard::copy_to_clipboard(&path);
            }
            Message::PreviewDock(action) => {
                self.dispatch_preview_dock(action);
                *needs_rebuild = true;
            }
            Message::OpenFile(path) => {
                services::fs::open_file(&path);
            }
        }
    }

    fn tick(&mut self, dt: f32, needs_rebuild: &mut bool) {
        // Pump the widget tick walk (the cce-data-editor pattern): animating
        // widgets register as tick receivers and report changed until their
        // transition lands; without this an animation freezes mid-way.
        if self.ui_context.tick(dt) {
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }

        // What arrived between events — a screen reader's Click on a row of
        // the app's own menu or on a button, its edit of the search box — is
        // sent from here; the pointer and the keyboard hooks take theirs as
        // they dispatch.
        if let Some(msg) = self.take_menu_pick() {
            let _ = self.sender.send(msg);
        }
        for msg in self.widget_messages() {
            let _ = self.sender.send(msg);
        }

        if self.just_initialized {
            self.just_initialized = false;
            if self.save_mode {
                self.ui_context.focus_id(self.browse.save_name_box.id());
                self.ui_context.set_focused_id(self.browse.save_name_box.id());
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
        }

        if self.ui_context.lend_h(self.paginator, |w, ctx| w.tick(dt, ctx)).unwrap_or(false) {
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }

        // The app-owned scrollers' wheel glide / flick coast: the wheel only
        // moves their target, these ticks carry the drawn offsets there, so
        // frames must keep coming while either is live.
        if self.browse.list.tick(dt) {
            // Rows slid under a still pointer: re-derive the hover.
            self.browse.list.cursor_moved(self.cursor_x, self.cursor_y);
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }
        if self.preview.tick(dt) {
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }
    }

    /// Re-request the shown file's preview when the renderer is replaced.
    ///
    /// `PreviewPane` holds a **renderer** image id, and a renderer does not
    /// outlive its session: `cce-ui`'s `window_runner` repairs a lost Wayland
    /// transport by opening a new session around the same `Application`, which
    /// rebuilds the renderer and with it the image table. The cached id then
    /// names an image that no longer exists, and a draw for an unknown id is
    /// skipped rather than reported — so a reconnected window kept the file's
    /// name, size and permissions and showed an empty well where the picture
    /// was, until the user selected a different file.
    ///
    /// The pixels are not kept here (they are moved into the upload), so the
    /// repair is the same round trip the selection makes: drop the dead
    /// texture, ask `FsService` for the preview again, and let
    /// `PreviewMessage::PreviewLoaded` upload it into the live renderer.
    ///
    /// Not on the first renderer: nothing has been uploaded yet, and the
    /// initial directory load is already in flight.
    fn renderer_init(&mut self, _renderer: &mut cce_ui::vk::VkRenderer) {
        if !std::mem::replace(&mut self.seen_renderer, true) {
            return;
        }
        let had_texture = self.preview.drop_texture();
        if let Some(path) = self.preview.path.clone() && had_texture {
            log::info!(
                "[preview] renderer replaced; re-reading the preview of {}",
                path.display()
            );
            self.request_preview(path, true);
        }
        self.needs_rebuild = true;
    }

    fn display_list(&mut self, size: LogicalSize, scale: f64) -> Option<cce_ui::scene::paint::DisplayList> {
        // Phase 6 single paint path: the whole frame — geometry and text — is this one list.
        // rebuild_layout paints every source (browse/network page, context menu, dialog)
        // into `self.frame`, with the menu/dialog occlusion already folded into each
        // text's bounds.
        if self.needs_rebuild || self.width != size.width as u32 || self.height != size.height as u32 || self.scale_factor != scale {
            self.width = size.width as u32;
            self.height = size.height as u32;
            self.scale_factor = scale;
            cce_ui::scale::set_scale_factor(scale as f32);
            self.rebuild_layout();
        }
        use cce_ui::scene::layout::Rect;
        let mut pc = cce_ui::scene::paint::PaintCtx::new();
        let (fw, fh) = (self.width as f32, self.height as f32);

        // The standard root plate (cce-ui `PlateSpec::window`): the DE root
        // material at its opacity, the shared silhouette arc, the DE roll.
        pc.root_plate(fw, fh);

        // Band carves, emitted right after the plate: in chooser mode the action
        // bar is a band carved into the bottom. These do NOT CSG-group into the
        // plate's draw, despite sitting immediately behind it — a band flush with
        // the plate's edge suppresses three of its four walls, and an
        // edge-suppressed carve is never eligible (its extended walls would smear
        // across the whole host). The standalone overlay path shades it, which is
        // correct here. `CCE_PLATE_DEBUG=1` names the rule.
        // (The header strip's menubar-style band was removed — the top of the
        // plate is flush; the breadcrumb row sits directly on the surface.)
        if cce_ui::layout::control_relief() {
            let wall = cce_ui::layout::bar_wall_width();
            if self.select_mode {
                let band_h = SELECT_BAR_H + cce_ui::layout::root_plate_inset();
                pc.recess_edges(
                    Rect { x: 0.0, y: fh - band_h, width: fw, height: band_h },
                    (0.0, 0.0, 0.0, 0.0),
                    wall,
                    (true, false, false, false),
                );
            }
        }

        // The content the last rebuild painted, as it painted it.
        pc.append_items(self.frame.items.iter().cloned());

        // The preview pane's plate-dock affordance (cce-ui RFC 7c-2): the
        // title stub while collapsed, and the corner control — drawn over
        // the page content, under the context menu.
        {
            use cce_ui::widget::plate_dock as dock;
            let band = self.preview_dock_band();
            let stubbed = self.preview_dock.stubbed();
            if stubbed {
                let r = cce_ui::layout::plate_corner_radius();
                pc.plate(
                    Rect { x: band.0, y: band.1, width: band.2, height: band.3 },
                    (r, r, r, r),
                    &cce_ui::scene::Material::from_fill(cce_ui::color::page_low_color()),
                    cce_ui::layout::bevel_width().min(band.3 * 0.2),
                );
                pc.text_with(
                    "Preview".to_string(),
                    band.0 + cce_ui::layout::plate_padding(),
                    band.1 + (band.3 - 13.0) / 2.0,
                    13.0,
                    [0xc8, 0xc8, 0xd4],
                    None,
                    Some([band.0, band.1, band.0 + band.2 - dock::CORNER_INSET - dock::CORNER_R, band.1 + band.3]),
                );
            }
            if let Some(c) = dock::corner_center(band, stubbed) {
                let emphasized = dock::corner_hit(c, self.cursor_x, self.cursor_y)
                    || (self.app_menu_live() && self.app_menu.as_ref().is_some_and(|m| m.corner));
                dock::draw_corner_dot(&mut pc, c, emphasized);
            }
        }

        // The toolkit's shared context menu (breadcrumb segments, config-bound
        // controls) draws into the app's display list like every popover. Its
        // labels carry bounds equal to the menu rect — the engine's popover
        // occlusion clamp exempts exactly that, so they render inside the menu
        // while page text beneath stays clamped.
        // The shared menu paints itself as a lit plate (cce-ui), so the
        // breadcrumb's copy-path menu is the same frosted glass as the row
        // menu above rather than the flat quads it used to stack. Labels come
        // with it: `paint_with_labels` carries the menu font's family, which
        // the hand-rolled `paint` + `text_labels()` loop here did not.
        cce_ui::widget::context_menu::paint_with_labels(&mut pc);
        if let Some([x, y, w, h]) = self.text_claim {
            cce_ui::text_input::claim(x, y, w, h);
        }
        Some(pc.finish())
    }

    fn display_list_text(&self) -> bool {
        true
    }

    fn clear_color(&self) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn handle_pointer_move(&mut self, pos: LogicalPosition, needs_rebuild: &mut bool) {
        self.cursor_x = pos.x;
        self.cursor_y = pos.y;

        let mut changed = false;

        // Routed dispatch (6bd shrink): one Event through the router per targeted root.
        let mv = cce_ui::widget::Event::PointerMove { x: pos.x, y: pos.y, local_x: pos.x, local_y: pos.y };
        if self.prompt_dialog.is_some() {
            if let Some(PromptDialog { textbox, .. }) = &mut self.prompt_dialog {
                let root = textbox.id();
                let _ = self.ui_context.propagate_event(&mv, root);
            }
            self.route_buttons(&mv);
            *needs_rebuild = true;
            self.needs_rebuild = true;
            return;
        }

        // The toolkit's shared context menu gets the pointer exclusively while open.
        if cce_ui::widget::context_menu::is_visible() {
            if cce_ui::widget::context_menu::cursor_moved(pos.x, pos.y) {
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
            return;
        }

        if self.current_page == Page::Browse {
            if self.browse_split.cursor_moved(pos.x, pos.y) {
                changed = true;
            }
        } else if self.current_page == Page::Network {
            if self.network_split.cursor_moved(pos.x, pos.y) {
                changed = true;
            }
        } else if self.current_page == Page::Space && self.space_split.cursor_moved(pos.x, pos.y) {
            changed = true;
        }

        if !self.select_mode {
            // Self-routing composite: handle_event, not propagate — the router's
            // children-first descent would let the embedded strip consume this.
            if self.ui_context.lend_h(self.paginator, |w, ctx| w.handle_event(&mv, ctx)).unwrap_or(false) {
                changed = true;
            }
        }

        if self.current_page == Page::Browse {
            if self.select_mode {
                let root = self.browse.save_name_box.id();
                if self.ui_context.propagate_event(&mv, root) {
                    changed = true;
                }
            }
            if self.browse.list.cursor_moved(pos.x, pos.y) {
                changed = true;
            }
            if self.browse.search_visible {
                let root = self.browse.search_box.id();
                if self.ui_context.propagate_event(&mv, root) {
                    changed = true;
                }
            }
            {
                let root = self.browse.breadcrumb.id();
                if self.ui_context.propagate_event(&mv, root) {
                    changed = true;
                }
            }
        } else if self.current_page == Page::Network {
            {
                let root = self.network.breadcrumb.id();
                if self.ui_context.propagate_event(&mv, root) {
                    changed = true;
                }
            }
            // The router forwards DragUpdate to a mid-drag node grab; a plain move runs
            // the hover recompute. Rebuild every move while a drag is live (the router
            // drops drag_update's changed flag).
            {
                let root = self.network.graph.id();
                if self.ui_context.propagate_event(&mv, root) {
                    changed = true;
                }
                if self.ui_context.is_dragging {
                    changed = true;
                }
            }
        } else if self.current_page == Page::Space {
            {
                let root = self.space.breadcrumb.id();
                if self.ui_context.propagate_event(&mv, root) {
                    changed = true;
                }
            }
            // Tile hover drives both the highlight outline and the footer
            // readout, so only a change of tile is worth a rebuild — a move
            // within one tile repaints nothing.
            let hovered = self.space.tile_at(pos.x, pos.y);
            if hovered != self.space.hovered {
                self.space.hovered = hovered;
                changed = true;
            }
        }

        // The chooser's buttons: repaint only when one's hover actually
        // changes (the move reports the enter / leave), so idle pointer moves
        // don't need a redraw. Rebuilding unconditionally re-commits the
        // translucent surface every move, which makes the compositor re-blur
        // the backdrop continuously.
        if self.route_buttons(&mv) {
            changed = true;
        }

        if changed {
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }
    }
    fn handle_mouse_input(&mut self, button: MouseButton, state: ElementState, pos: LogicalPosition, needs_rebuild: &mut bool) -> Option<Self::Message> {
        let msg = self.mouse_input(button, state, pos, needs_rebuild);
        self.after_dispatch(msg, needs_rebuild)
    }

    fn handle_key_input(&mut self, event: &KeyEvent, needs_rebuild: &mut bool) -> Option<Self::Message> {
        let msg = self.key_input(event, needs_rebuild);
        self.after_dispatch(msg, needs_rebuild)
    }

    fn handle_mouse_wheel(&mut self, delta: &MouseScrollDelta, pos: LogicalPosition, needs_rebuild: &mut bool) {
        // Every page shares the preview pane on the right.
        if matches!(self.current_page, Page::Browse | Page::Network | Page::Space) {
            // The pane hit-tests its own laid-out rect and consumes any wheel
            // over its content region, scrolled or not.
            if let Some(changed) = (!self.preview_dock.collapsed)
                .then(|| self.preview.wheel(delta, pos.x, pos.y))
                .flatten()
            {
                if cce_ui::scroll_debug() {
                    eprintln!("[scroll] files: preview consumed at ({:.0},{:.0})", pos.x, pos.y);
                }
                if changed {
                    *needs_rebuild = true;
                    self.needs_rebuild = true;
                }
                return;
            }
        }

        if self.current_page == Page::Browse {
            let hit = self.browse.list.wheel(delta, pos.x, pos.y);
            if cce_ui::scroll_debug() {
                eprintln!("[scroll] files: browse.list.wheel at ({:.0},{:.0}) -> {hit}", pos.x, pos.y);
            }
            if hit {
                // A trackpad finger moves the rows now: keep the hover on the
                // row under the pointer (a wheel glide does this in tick).
                self.browse.list.cursor_moved(self.cursor_x, self.cursor_y);
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
        } else if self.current_page == Page::Network {
            let ev = cce_ui::widget::Event::MouseWheel { delta: *delta, x: pos.x, y: pos.y, local_x: pos.x, local_y: pos.y };
            let root = self.network.graph.id();
            if self.ui_context.propagate_event(&ev, root) {
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
        }
    }

    /// Tab walks the toolbar's plates and wells (cce-ui's navigation in
    /// plate terms); the file rows are the list's to walk.
    fn plate_navigation(&self) -> bool {
        true
    }

    /// The geometry is cached until the next rebuild — a moved focus ring needs one.
    fn focus_stepped(&mut self) {
        self.needs_rebuild = true;
    }

}

// ── Main ────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    cce_ui::engine::run::<FilesystemApp>();
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIN: [f32; 4] = [0.0, 0.0, 1000.0, 700.0];

    fn rect_overlay(x: f32, y: f32, w: f32, h: f32) -> [f32; 4] {
        [x, y, w, h]
    }

    /// A box the overlay covers outright is discarded, not clipped.
    #[test]
    fn a_covered_box_is_dropped() {
        let menu = rect_overlay(300.0, 253.0, 430.0, 120.0);
        assert!(occlude_against(WIN, 320.0, 400.0, 300.0, 317.0, &[menu]).is_none());
    }

    /// The frame's text is cut away from the overlays painted over it: a label
    /// under the menu's face is dropped, one running under its edge is cut at
    /// it, one clear of it is untouched, and the geometry is left alone.
    #[test]
    fn the_frame_text_under_an_overlay_is_cut() {
        use cce_ui::scene::paint::{PaintCtx, Prim};
        let mut pc = PaintCtx::new();
        pc.quad(pages::rect(0.0, 0.0, 10.0, 10.0), [1.0; 4]);
        pages::text(&mut pc, "under", 320.0, 300.0, 12.0, [1.0; 4], None, None);
        pages::text(&mut pc, "a long line running under the menu", 40.0, 288.0, 12.0, [1.0; 4], None, None);
        pages::text(&mut pc, "clear", 40.0, 600.0, 12.0, [1.0; 4], None, None);
        let mut list = pc.finish();
        occlude_text(&mut list, &[rect_overlay(300.0, 253.0, 430.0, 120.0)], (WIN[2], WIN[3]));
        let texts: Vec<(String, Option<[f32; 4]>)> = list
            .items
            .iter()
            .filter_map(|i| match &i.prim {
                Prim::Text { text, bounds, .. } => Some((text.clone(), *bounds)),
                _ => None,
            })
            .collect();
        assert_eq!(list.items.len(), 3, "the fill and two labels");
        assert_eq!(texts[0].0, "a long line running under the menu");
        assert_eq!(texts[0].1.unwrap()[2], 300.0, "cut at the menu's left edge");
        assert_eq!(texts[1], ("clear".to_string(), Some(WIN)));
    }

    /// The regression. A file-list row's name sits well left of the menu, but the
    /// row's box runs under it, and the hovered menu row's fill dips into that
    /// box's band from below without spanning it horizontally. Clamping both axes
    /// crossed the y bounds over into an empty band and the whole line vanished,
    /// the name along with the part under the menu. Only the x cut is legitimate
    /// here: the name must survive whole.
    #[test]
    fn an_overlay_that_only_dips_in_does_not_erase_the_line() {
        let menu = rect_overlay(300.0, 253.0, 430.0, 120.0);
        let hover = rect_overlay(305.0, 294.0, 420.0, 20.0);

        let b = occlude_against(WIN, 40.0, 420.0, 288.0, 305.0, &[menu, hover])
            .expect("a line the overlay only dips into still has a visible run");

        assert_eq!(b[2], 300.0, "clipped at the menu's left edge");
        assert_eq!(b[3], WIN[3], "and NOT clipped vertically — that is the erasure");
        assert_eq!(b[1], WIN[1]);
        assert_eq!(b[0], WIN[0]);
    }

    /// The y cut is still right when the overlay spans the box horizontally: no
    /// horizontal trim can uncover anything, so the surviving run is the strip
    /// above (here) or below the overlay.
    #[test]
    fn an_overlay_spanning_the_box_clips_it_vertically() {
        let menu = rect_overlay(300.0, 253.0, 430.0, 120.0);
        let b = occlude_against(WIN, 320.0, 400.0, 240.0, 257.0, &[menu]).expect("straddles the top");
        assert_eq!(b[3], 253.0, "clipped to the strip above the menu");
        assert_eq!(b[2], WIN[2], "and not trimmed horizontally");
    }

    /// Of the two sides an overlay leaves, the cut keeps the wider one — the most
    /// of the box that a single clip rect can still express.
    #[test]
    fn the_cut_keeps_the_wider_side() {
        // Overlay near the box's right end: the long run before it survives.
        let right = rect_overlay(380.0, 0.0, 200.0, 700.0);
        let b = occlude_against(WIN, 40.0, 420.0, 288.0, 305.0, &[right]).unwrap();
        assert_eq!(b[2], 380.0);
        assert_eq!(b[0], WIN[0]);

        // Overlay near the left end: the run after it survives instead.
        let left = rect_overlay(0.0, 0.0, 80.0, 700.0);
        let b = occlude_against(WIN, 40.0, 420.0, 288.0, 305.0, &[left]).unwrap();
        assert_eq!(b[0], 80.0);
        assert_eq!(b[2], WIN[2]);
    }

    #[test]
    fn the_breadcrumb_menu_offers_the_other_views() {
        let rows = breadcrumb_menu_rows("/home/me".to_string(), Page::Network);
        let labels: Vec<&str> = rows.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, ["Copy Path", "-", "List", "Space"]);
        assert!(matches!(&rows[0].1, Some(Message::CopyPath(p)) if p == "/home/me"));
        assert!(rows[1].1.is_none(), "the separator sends nothing");
        assert!(matches!(rows[2].1, Some(Message::SwitchPage(Page::Browse))));
        assert!(matches!(rows[3].1, Some(Message::SwitchPage(Page::Space))));
        for page in Page::ALL {
            let rows = breadcrumb_menu_rows(String::new(), page);
            assert!(
                !rows.iter().any(|(_, m)| matches!(m, Some(Message::SwitchPage(p)) if *p == page)),
                "{page:?} offers itself"
            );
        }
    }

    #[test]
    fn a_space_tile_menu_copies_its_path() {
        let tile = |name: &str, is_dir, depth, rest| pages::space::Tile {
            path: std::path::PathBuf::from("/root").join(name),
            name: name.to_string(),
            size: 1,
            is_dir,
            depth,
            rect: (0.0, 0.0, 10.0, 10.0),
            aggregate: None,
            rest,
        };
        let labels = |t| space_tile_menu(&t).into_iter().map(|(l, _)| l).collect::<Vec<_>>();
        assert_eq!(labels(tile("a.txt", false, 2, 0)), ["[File] a.txt", "Open", "Open with...", "Copy Path"]);
        assert_eq!(labels(tile("sub", true, 1, 0)), ["[Directory] sub", "Open", "Copy Path"]);
        assert_eq!(labels(tile("here", true, 0, 0)), ["[Directory] here", "Copy Path"]);
        assert_eq!(labels(tile("3 smaller items", true, 2, 3)), ["3 smaller items", "Open Folder", "Copy Folder Path"]);

        let menu = space_tile_menu(&tile("a.txt", false, 2, 0));
        assert!(menu[0].1.is_none(), "the header does nothing");
        assert!(matches!(&menu.last().unwrap().1, Some(Message::CopyPath(p)) if p == "/root/a.txt"));
    }

    #[test]
    fn a_graph_node_menu_follows_the_node_order() {
        let entry = |name: &str, is_dir| pages::browse::DirEntry {
            name: name.to_string(),
            path: std::path::PathBuf::from("/home/me").join(name),
            is_dir,
            size: 0,
            permissions: 0,
            modified: String::new(),
            origin: None,
        };
        let entries = [entry("docs", true), entry("a.txt", false)];
        let dir = std::path::Path::new("/home/me");
        let labels = |idx| graph_node_menu(idx, dir, &entries).map(|m| m.into_iter().map(|(l, _)| l).collect::<Vec<_>>());
        assert_eq!(labels(0).unwrap(), ["[Directory] home", "Open", "Copy Path"]);
        assert_eq!(labels(1).unwrap(), ["[Directory] me", "Copy Path"]);
        assert_eq!(labels(2).unwrap(), ["[Directory] docs", "Open", "Copy Path"]);
        assert_eq!(labels(3).unwrap(), ["[File] a.txt", "Open", "Open with...", "Copy Path"]);
        assert!(labels(4).is_none());

        let menu = graph_node_menu(3, dir, &entries).unwrap();
        assert!(matches!(&menu.last().unwrap().1, Some(Message::CopyPath(p)) if p == "/home/me/a.txt"));
        // At the root there is no parent node: node 0 is the root itself.
        let root = graph_node_menu(0, std::path::Path::new("/"), &[]).unwrap();
        assert_eq!(root[0].0, "[Directory] /");
        assert_eq!(root.len(), 2);
    }
}
