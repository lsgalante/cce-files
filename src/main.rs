use wayland_client::QueueHandle;
use cce_ui::cosmic_text::FontSystem;

use cce_ui::engine::{Application, LogicalPosition, LogicalSize, WindowSettings};
use cce_ui::widget::{MouseButton, ElementState, MouseScrollDelta, KeyEvent, WidgetHost, PageSelector, Paginator, MenuController};
use cce_ui::widget::{GraphController, PathController};

use notify::{Watcher, RecommendedWatcher, RecursiveMode, Config};

use cce_files::{Message, pages, services};
use cce_files::pages::Page;
use cce_files::pages::browse::is_project_dir;

// ── Layout constants ────────────────────────────────────────────────

const ROW_H: f32 = 24.0;        // context-menu / breadcrumb row height
const DIALOG_W: f32 = 400.0;
const DIALOG_H: f32 = 160.0;
const MENU_MIN_W: f32 = 120.0;

/// Context-menu width/height for a given set of options.
///
/// The width comes from the SHAPED widest label in the menu font — the same
/// face [`cce_ui::widget::context_menu`] uses for the toolkit's own menus.
/// The old `bytes * 7.5` estimate belonged to neither face, so the plate was
/// sized for a font nothing here draws in.
fn context_menu_size(options: &[(String, Option<Message>)]) -> (f32, f32) {
    let (family, size) = cce_ui::widget::context_menu::label_font();
    let widest = options
        .iter()
        .map(|(s, _)| cce_ui::widget::display::measure_text_width(s, &family, size))
        .fold(0.0f32, f32::max);
    let w = (widest + 24.0).max(MENU_MIN_W);
    let h = options.len() as f32 * ROW_H;
    (w, h)
}

/// Computed rects for the "Open with…" modal, so layout and hit-testing agree.
struct OpenWithRects {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    tb: (f32, f32, f32, f32),
    cancel: (f32, f32, f32, f32),
    open: (f32, f32, f32, f32),
}

fn open_with_rects(win_w: f32, win_h: f32) -> OpenWithRects {
    let x = (win_w - DIALOG_W) / 2.0;
    let y = (win_h - DIALOG_H) / 2.0;
    let tb_h = cce_ui::layout::textbox_height();
    let btn_h = cce_ui::layout::button_height();
    OpenWithRects {
        x,
        y,
        w: DIALOG_W,
        h: DIALOG_H,
        tb: (x + 20.0, y + 60.0, DIALOG_W - 40.0, tb_h),
        cancel: (x + DIALOG_W - 180.0, y + DIALOG_H - btn_h - 16.0, 70.0, btn_h),
        open: (x + DIALOG_W - 100.0, y + DIALOG_H - btn_h - 16.0, 80.0, btn_h),
    }
}

/// Clip a vertical span `[y, y+h)` to the viewport `[top, bottom)`.
/// Returns the clipped `(y, h)`, or `None` if fully outside.
fn clip_to_viewport(y: f32, h: f32, top: f32, bottom: f32) -> Option<(f32, f32)> {
    if y >= bottom || y + h <= top {
        return None;
    }
    let mut ny = y;
    let mut nh = h;
    if ny < top {
        let diff = top - ny;
        ny = top;
        nh = (nh - diff).max(0.0);
    }
    if ny + nh > bottom {
        nh = (bottom - ny).max(0.0);
    }
    Some((ny, nh))
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

/// Clip a text/label box against overlay rects so it does not bleed through
/// popovers/menus/dialogs. Returns the adjusted clip bounds, or `None` if the
/// box is fully covered (should be discarded).
fn occlude_against(
    mut bounds: [f32; 4],
    t_min_x: f32,
    t_max_x: f32,
    t_min_y: f32,
    t_max_y: f32,
    overlays: &[&pages::PageContent],
) -> Option<[f32; 4]> {
    for overlay_pc in overlays {
        // Plates occlude exactly as rects do — an overlay's opaque face is its
        // plate now, not a flat fill, and a face missing from this sweep lets the
        // page's text bleed through the surface covering it.
        let faces = overlay_pc
            .rects
            .iter()
            .map(|(_, x, y, w, h, _, _)| (*x, *y, *w, *h))
            .chain(overlay_pc.plates.iter().map(|(_, x, y, w, h, _, _)| (*x, *y, *w, *h)));
        for (ox, oy, ow, oh) in faces {
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
    }
    Some(bounds)
}

/// Height of the chooser-mode bottom action bar (the band carved into the plate).
const SELECT_BAR_H: f32 = 48.0;

// ── State ───────────────────────────────────────────────────────────

/// How a flat quad renders under the SDF-lit plate system. `Flat` is the plain
/// fill; the relief variants carry the roll/carve depth in px.
#[derive(Clone, Copy, PartialEq)]
enum WidgetFx {
    Flat,
    /// A lit Bevel plate: the quad's own fill plus a rolled, lit lip (raised
    /// buttons with an opaque face).
    Bevel(f32),
    /// Edges-only raised plateau over whatever is painted below.
    Boss(f32),
    /// A lit plate: a rounded face in the quad's own color plus the rolled, lit
    /// perimeter, at full size. What the pane plates and the preview stub wear.
    /// Distinct from [`WidgetFx::Bevel`], which insets its fill by the depth, and
    /// from [`WidgetFx::Boss`], which is a rim with no face of its own.
    Plate(f32),
    /// Edges-only carve into whatever is painted below (recessed wells).
    Recess(f32),
    /// [`WidgetFx::Recess`] with pointer focus: the tinted carve — the wrapped
    /// accent glint REPLACING the relief lighting (the DE's one focus
    /// language; the shader drops the diffuse/curvature terms for tinted
    /// carves, so the ring is all that shows).
    RecessFocus(f32),
    /// A flush inset control (buttons): groove ring carved down around the
    /// rect, beveled lip back up inside, face level with the surface.
    Inset(f32),
    /// [`WidgetFx::Inset`] with keyboard focus: the rim lit in the highlight —
    /// the ring a focused control plate wears.
    InsetFocus(f32),
    /// A GPU-textured quad; the id comes from `cce_ui::vk::upload_rgba`
    /// (the preview pane's image). `color` is unused.
    Image { id: u32, alpha: f32 },
    /// A line engraved from (ax, ay) to (bx, by) into the widget's rect, which
    /// is the HOST surface here rather than the mark's own bounds — the shading
    /// fades out across the host's rolled edge. The breadcrumb's slanted seams;
    /// the only quad in this list that is not axis-aligned.
    Groove { ax: f32, ay: f32, bx: f32, by: f32, width: f32, depth: f32 },
}

struct AppWidget {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: [f32; 4],
    radius: f32,
    corners: (bool, bool, bool, bool),
    fx: WidgetFx,
}

#[derive(Clone)]
struct ContextMenu {
    visible: bool,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    options: Vec<(String, Option<Message>)>,
    hovered: Option<usize>,
}

/// App-owned two-pane horizontal split, replacing the dissolved `SplitBox` +
/// `BrowseContainer`/`NetworkContainer` shims (Phase 6y). Those existed to (a) position
/// pane content — but the pages already lay out and render everything from the pane rect,
/// the container copies just coincided (the Phase 0 double-paint) — and (b) own the
/// divider: its quad, hover tint, and proportion drag. This is (b), app-side, with the
/// `SplitBox` two-child horizontal math verbatim.
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
/// (`cce-files` domain → `cce-ui` domain), defaulting to the historical
/// vim-ish keys. Arrow keys, Enter-in-save-mode, and Escape stay fixed.
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
    /// See [`FocusedWell`]; drives the per-well `focused` flags each rebuild.
    focused_well: FocusedWell,

    // Command-line chooser options
    select_mode: bool,
    select_directory: bool,
    save_mode: bool,

    // Rendering resources
    widgets: Vec<AppWidget>,
    // (content, font_size, x, y, color, font, bounds) — occlusion-adjusted text tuples,
    // emitted as display-list Text prims.
    texts: Vec<(String, f32, f32, f32, [f32; 4], Option<String>, Option<[f32; 4]>)>,
    font_system: FontSystem,
    needs_rebuild: bool,
    width: u32,
    height: u32,
    scale_factor: f64,
    page_buttons: Vec<(cce_ui::widget::Adapted<cce_ui::widget::Button>, Message)>,
    hovered_button: Option<usize>,
    cursor_x: f32,
    cursor_y: f32,
    paginator: cce_ui::widget::Adapted<Paginator>,
    view_dropdown: cce_ui::widget::Adapted<cce_ui::widget::Dropdown>,
    just_initialized: bool,
    ui_context: cce_ui::context::UiContext,
    watcher: Option<notify::RecommendedWatcher>,
    fs_service: services::fs::FsService,
    /// Whether a renderer has been handed over yet — the first one is the
    /// process's own, any later one is a replacement after a reconnect. See
    /// `renderer_init`.
    seen_renderer: bool,
    context_menu: ContextMenu,
    open_with_dialog: Option<(std::path::PathBuf, cce_ui::widget::Adapted<cce_ui::widget::TextBox>)>,
    browse_split: SplitPane,
    network_split: SplitPane,
    space_split: SplitPane,
    /// The preview pane's plate-dock state (cce-ui RFC 7c-2): collapsing
    /// narrows the pane column to a title stub and the list keeps the space.
    preview_dock: cce_ui::widget::plate_dock::PlateDockState,
    /// The three splits' fracs as they stood before a collapse, restored on
    /// expand.
    preview_prior_fracs: Option<(f32, f32, f32)>,
    /// Rows of the OPEN plate-dock corner menu (empty = not ours); routed
    /// before the generic context-menu dispatch.
    plate_menu_actions: Vec<cce_ui::widget::plate_dock::PlateDockAction>,
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

    fn rebuild_layout(&mut self) {

        // Well focus → the emitters (each picks the tinted carve when set).
        self.browse.list.focused = self.focused_well == FocusedWell::Content;
        self.space.focused = self.focused_well == FocusedWell::Content;
        self.preview.focused_well = match self.focused_well {
            FocusedWell::PreviewTop => Some(cce_files::preview_pane::PreviewWell::Top),
            FocusedWell::PreviewBottom => Some(cce_files::preview_pane::PreviewWell::Bottom),
            FocusedWell::Content => None,
        };

        self.ui_context.clear_hierarchy();
        self.browse.save_name_box.prepare_text(&mut self.font_system);
        self.browse.search_box.prepare_text(&mut self.font_system);

        let mut widgets = Vec::new();
        let mut texts = Vec::new();

        cce_ui::widget::hover_animation::reset_frame_registration();
        cce_ui::widget::hover_animation::set_cursor_pos(self.cursor_x, self.cursor_y);

        // root plate container DISSOLVED: top-level widgets register parentless below; the
        // window plate tuple is emitted in the legacy aggregate order (after the plain
        // child quads).

        // Clear all widgets' hierarchy links
        self.paginator.clear_children(&mut self.ui_context); self.paginator.set_parent(None, &mut self.ui_context);
        self.view_dropdown.clear_children(&mut self.ui_context); self.view_dropdown.set_parent(None, &mut self.ui_context);


        self.browse.save_name_box.clear_children(&mut self.ui_context); self.browse.save_name_box.set_parent(None, &mut self.ui_context);
        self.browse.breadcrumb.clear_children(&mut self.ui_context); self.browse.breadcrumb.set_parent(None, &mut self.ui_context);
        self.network.breadcrumb.clear_children(&mut self.ui_context); self.network.breadcrumb.set_parent(None, &mut self.ui_context);
        self.network.graph.clear_children(&mut self.ui_context); self.network.graph.set_parent(None, &mut self.ui_context);
        self.space.breadcrumb.clear_children(&mut self.ui_context); self.space.breadcrumb.set_parent(None, &mut self.ui_context);
        if let Some((_, textbox)) = &mut self.open_with_dialog {
            textbox.clear_children(&mut self.ui_context);
            textbox.set_parent(None, &mut self.ui_context);
        }

        let has_sidebar = false;
        let sidebar_w = if has_sidebar { self.paginator.sidebar_w() } else { 0.0 };
        // DE-wide plate rim padding (style.surface.root plate.padding).
        let pad = cce_ui::layout::root_plate_padding();
        let browse_x = if has_sidebar { sidebar_w + pad + 1.0 } else { pad };
        let usable_w = self.width as f32 - sidebar_w - (if has_sidebar { 1.0 } else { 0.0 }) - 2.0 * pad;
        let content_y = pad;

        let select_bar_h = SELECT_BAR_H;
        let content_h = if self.select_mode {
            self.height as f32 - 2.0 * pad - select_bar_h
        } else {
            self.height as f32 - 2.0 * pad
        };

        {
            let self_ptr = self as *mut Self;
            unsafe {
                if has_sidebar {
                    self.ui_context.register_widget((*self_ptr).paginator.base().id(), (*self_ptr).paginator.as_ptr_mut());
                    (*self_ptr).paginator.set_parent(None, &mut self.ui_context);
                }
                self.ui_context.register_widget((*self_ptr).view_dropdown.base().id(), (*self_ptr).view_dropdown.as_ptr_mut());
                (*self_ptr).view_dropdown.set_parent(None, &mut self.ui_context);
            }
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

        if let Some((_, textbox)) = &mut self.open_with_dialog {
            self.ui_context.register_widget(textbox.base().id(), textbox.as_ptr_mut());
            textbox.set_parent(None, &mut self.ui_context);
        }

        // Layout widgets recursively inside the parent space
        let mut dummy_pc = pages::PageContent::new();
        if has_sidebar {
            let page_idx = Page::ALL.iter().position(|&p| p == self.current_page).unwrap_or(0);
            self.paginator.set_selected_page(page_idx);
            cce_ui::layout::render_widget(&mut dummy_pc, &mut self.paginator, 0.0, 0.0, sidebar_w, self.height as f32, &mut self.ui_context);
        }

        // Each top-level widget rendered through the same immediate-mode path the root
        // recursion used, replicating the legacy TUPLE ORDER: plain child quads first,
        // then the dissolved root plate container's plate, then the rounded children (the
        // aggregate emitted all plain quads before the rounded root bg).
        let mut window_pc = pages::PageContent::new();
        {
            let self_ptr = self as *mut Self;
            unsafe {
                let mut plain_pc = pages::PageContent::new();
                // NOT the view dropdown: every page's `view()` already renders
                // it, so a copy here was a second draw of the same widget — at
                // the previous frame's rect, and compositing its label's
                // antialiased edges twice into a faux-bold.
                // The dissolved splitter's paint: its divider quad, then the preview
                // pane (the only pane content the pages don't render themselves). The
                // left pane's container copy is gone — the legacy aggregate painted it
                // UNDER the page's own copy, double-compositing every translucent quad.
                {
                    let split = match self.current_page {
                        Page::Browse => &self.browse_split,
                        Page::Network => &self.network_split,
                        Page::Space => &self.space_split,
                    };
                    // A collapsed preview draws neither divider nor content —
                    // the stub band and its corner control paint in
                    // display_list, over everything (cce-ui RFC 7c-2).
                    if !self.preview_dock.collapsed {
                        if let Some((dx, dy, dw, dh, dc)) = split.divider_quad() {
                            plain_pc.rects.push((dc, dx, dy, dw, dh, 0.0, (true, true, true, true)));
                        }
                        let (px_r, py_r, pw_r, ph_r) = split.right_rect();
                        // The pane clamps its own text bounds to its rect inside
                        // push_prims (the old SplitBox clamp, absorbed).
                        (*self_ptr).preview.set_rect(px_r, py_r, pw_r, ph_r);
                        (*self_ptr).preview.push_prims(&mut plain_pc);
                    }
                }
                if let Some((_, textbox)) = &mut (*self_ptr).open_with_dialog {
                    let (x, y, w, h) = textbox.rect();
                    cce_ui::layout::render_widget(&mut plain_pc, textbox, x, y, w, h, &mut self.ui_context);
                }

                // Legacy aggregate order: plain child quads, then rounded child quads.
                // The dissolved root plate that used to sit between them is now a
                // Prim::Plate emitted FIRST in display_list — the lit window slab the
                // rest of the frame sits on (and the surface the band carves CSG into).
                let mut plain_pc = plain_pc;
                let (plain, rounded): (Vec<_>, Vec<_>) =
                    std::mem::take(&mut plain_pc.rects).into_iter().partition(|r| r.5 <= 0.1);
                window_pc.rects.extend(plain);
                window_pc.rects.extend(rounded);
                window_pc.absorb(plain_pc);

                // The view dropdown's flush inset plate is carved below, once
                // the pages have laid the dropdown out — carving it here would
                // read the previous frame's rect (`pages::dropdown_relief`).
            }
        }

        // 3. Draw Page custom/static content (drawn to pc)
        let mut pc = pages::PageContent::new();
        match self.current_page {
            Page::Browse => {
                let (bx, by, bw, bh) = self.browse_split.left_rect();
                let browse_pc = pages::browse::view(&mut self.browse, &mut self.view_dropdown, bx, by, bw, bh, self.select_mode, &mut self.ui_context);

                pc.absorb(browse_pc);
            }
            Page::Network => {
                let (nx, ny, nw, nh) = self.network_split.left_rect();
                let network_pc = pages::network::view(&mut self.network, &self.browse, &mut self.view_dropdown, nx, ny, nw, nh, &mut self.ui_context);

                pc.absorb(network_pc);
            }
            Page::Space => {
                let (sx, sy, sw, sh) = self.space_split.left_rect();
                let space_pc = pages::space::view(&mut self.space, &self.browse, &mut self.view_dropdown, sx, sy, sw, sh, &mut self.ui_context);

                pc.absorb(space_pc);
            }
        }

        // The view dropdown's flush inset plate (control_relief) lives in its
        // modern paint(); the flat view loses it, so carve it here — from the
        // rect the page above just laid the dropdown out at, NOT the one it
        // held when this method started.
        {
            let (dx, dy, dw, dh) = self.view_dropdown.rect();
            pages::dropdown_relief(
                &mut window_pc,
                cce_ui::scene::layout::Rect { x: dx, y: dy, width: dw, height: dh },
            );
        }

        // Draw bottom selection bar if select_mode is enabled. It lives below the
        // content region, so it goes into window_pc: page content (pc) is clipped
        // to the viewport and would swallow the bar entirely.
        if self.select_mode {
            let bar_y = self.height as f32 - select_bar_h - cce_ui::layout::root_plate_padding();
            // Divider line — under control_relief the bar is a band carved into the
            // plate (see display_list), so the flat line is the fallback only.
            if !cce_ui::layout::control_relief() {
                window_pc.rect([0.15, 0.20, 0.16, 1.0], browse_x, bar_y, usable_w, 1.0);
            }

            let btn_h = cce_ui::layout::button_height();
            let btn_y = bar_y + (select_bar_h - btn_h) / 2.0;

            // Cancel and Save/Select: equal widths, one gap, right-aligned to
            // the content region with the bar's own padding — and the
            // toolkit's plain button face, not hand-picked red/green tints.
            let btn_w = 84.0;
            let gap = 10.0;
            // Flush with the panels' right edge — an extra inset here left the
            // buttons hanging short of the column above them.
            let confirm_x = browse_x + usable_w - btn_w;
            let cancel_x = confirm_x - gap - btn_w;
            window_pc.button_plain("Cancel", cancel_x, btn_y, btn_w, btn_h, Message::SelectCancel);
            let button_label = if self.save_mode { "Save" } else { "Select" };
            window_pc.button_plain(button_label, confirm_x, btn_y, btn_w, btn_h, Message::SelectOpen);
        }

        // Gather all popovers
        let mut popover_pc = pages::PageContent::new();
        cce_ui::layout::render_popovers(&mut popover_pc, &mut self.ui_context);

        // Gather context menu overlay if visible
        let mut context_menu_pc = pages::PageContent::new();
        if self.context_menu.visible {
            let cx = self.context_menu.x;
            let cy = self.context_menu.y;
            let cw = self.context_menu.w;
            let ch = self.context_menu.h;

            // The menu is a lit plate of the same material as the surface it
            // opens on: it takes the HOST plate's face rather than a popover
            // color of its own, and floats as frosted glass — the configured
            // translucency plus the blur-behind sentinel (negative alpha), which
            // is the breadcrumb's raised-run treatment. The frost is not
            // decoration at this alpha: it is what keeps the labels readable
            // over live content instead of over a legible-by-luck backdrop.
            //
            // A fully transparent host fill degrades to the edges-only boss,
            // exactly as the raised run does — with no face to tint, a plate
            // would paint a hole.
            let menu_r = cce_ui::layout::plate_corner_radius();
            let face = cce_ui::color::page_low_color();
            if face[3] > 0.001 {
                let mut frosted = face;
                frosted[3] = -frosted[3];
                context_menu_pc.plate(frosted, cx, cy, cw, ch, menu_r);
            } else {
                context_menu_pc.relief_raised(cx, cy, cw, ch, menu_r);
            }
            // Hover highlight, inset off the roll so it sits on the face rather
            // than climbing the lit edge. The last row is the only one that meets
            // a rounded corner (row 0 is the header and never highlights), so it
            // carries the plate's radius on the bottom two.
            if let Some(h_idx) = self.context_menu.hovered {
                let iy = cy + h_idx as f32 * ROW_H;
                let inset = (cce_ui::layout::bevel_width().min(ch * 0.2) * 0.5).max(2.0);
                let last = h_idx + 1 == self.context_menu.options.len();
                context_menu_pc.rect_rounded(
                    [0.20, 0.40, 0.65, 0.6],
                    cx + inset,
                    iy + 2.0,
                    cw - 2.0 * inset,
                    ROW_H - 4.0,
                    (menu_r - inset).max(0.0),
                    (false, false, last, last),
                );
            }
            // Text options, in the DE's menu font — family and size. Drawn
            // with a hardcoded 12.0 and no family, the row menu read in the
            // default sans while the list it opened over wore the configured
            // face.
            let (menu_family, menu_size) = cce_ui::widget::context_menu::label_font();
            for (idx, (opt, _)) in self.context_menu.options.iter().enumerate() {
                let iy = cy + idx as f32 * ROW_H + (ROW_H - menu_size) / 2.0;
                // The toolkit's semantic text colors, not the hand-mixed greys
                // that came with the near-black popover face: the header's old
                // 0.44 grey was a step above black and all but vanishes on the
                // plate's own mid-slate.
                let text_color = if idx == 0 {
                    cce_ui::color::TEXT_DIM
                } else if self.context_menu.hovered == Some(idx) {
                    cce_ui::color::TEXT_HEADER
                } else {
                    cce_ui::color::TEXT_FG
                };
                context_menu_pc.text_with_font(opt, cx + 8.0, iy, menu_size, text_color, &menu_family);
            }
        }

        // Gather open-with dialog backdrop & dialog panel if active (open_with_dialog uses textbox rendering manually but we can gather its other quads/texts)
        let mut dialog_pc = pages::PageContent::new();
        if let Some((_path, textbox)) = &mut self.open_with_dialog {
            let r = open_with_rects(self.width as f32, self.height as f32);
            let (dialog_x, dialog_y, dialog_w, dialog_h) = (r.x, r.y, r.w, r.h);
            let (tb_x, tb_y, tb_w, tb_h) = r.tb;
            let (btn_cancel_x, btn_cancel_y, btn_cancel_w, btn_cancel_h) = r.cancel;
            let (btn_open_x, btn_open_y, btn_open_w, btn_open_h) = r.open;

            // Semi-transparent backdrop overlay
            dialog_pc.rect([0.02, 0.02, 0.03, 0.6], 0.0, 0.0, self.width as f32, self.height as f32);
            // Dialog panel border
            dialog_pc.rect([0.22, 0.22, 0.28, 1.0], dialog_x, dialog_y, dialog_w, dialog_h);
            // Dialog panel background
            dialog_pc.rect([0.08, 0.08, 0.12, 1.0], dialog_x + 1.0, dialog_y + 1.0, dialog_w - 2.0, dialog_h - 2.0);

            // Title text
            dialog_pc.text("Open with...", dialog_x + 20.0, dialog_y + 20.0, 14.0, [1.0, 1.0, 1.0, 1.0]);
            // Description
            dialog_pc.text("Enter command:", dialog_x + 20.0, dialog_y + 42.0, 11.0, [0.54, 0.54, 0.58, 1.0]);

            // Set textbox position dynamically using configured textbox height
            textbox.set_rect(tb_x, tb_y, tb_w, tb_h);
            // Raised dialog plate + recessed command well (control_relief styling).
            dialog_pc.relief_raised(dialog_x, dialog_y, dialog_w, dialog_h, 0.0);
            dialog_pc.relief_recessed(tb_x, tb_y, tb_w, tb_h, cce_ui::layout::textbox_corner_radius());

            // Plain toolkit faces, like the chooser footer: the themed Button
            // owns its own hover state, so the hand-rolled cursor tracking and
            // the red/green tints go together.
            dialog_pc.button_plain("Cancel", btn_cancel_x, btn_cancel_y, btn_cancel_w, btn_cancel_h, Message::OpenWithCancel);
            dialog_pc.button_plain("Open", btn_open_x, btn_open_y, btn_open_w, btn_open_h, Message::OpenWithSubmit);
        }

        // Translate everything into widgets and text_items!
        // We collect from: window_pc, pc, popover_pc, context_menu_pc, dialog_pc
        let mut page_buttons = Vec::new();

        for (part_idx, pc_part) in [&window_pc, &pc, &popover_pc, &context_menu_pc, &dialog_pc].into_iter().enumerate() {
            let is_page_content = part_idx == 1;

            // Plates first: a plate owns a FACE, so it is the floor of its part —
            // everything else the part emits (a hover fill, an engraved seam, the
            // labels) is meant to land on top of it. Emitted after the rects it
            // would paint straight over them, which is where the context menu's
            // hover row went the first time this ran.
            for (c, px, py, pw, ph, pr, pd) in &pc_part.plates {
                let (mut wy, mut wh) = (*py, *ph);
                if is_page_content {
                    match clip_to_viewport(wy, wh, content_y, content_y + content_h) {
                        Some((cy, ch)) => { wy = cy; wh = ch; }
                        None => continue,
                    }
                }
                widgets.push(AppWidget {
                    x: *px,
                    y: wy,
                    w: *pw,
                    h: wh,
                    color: *c,
                    radius: *pr,
                    corners: (true, true, true, true),
                    fx: WidgetFx::Plate(*pd),
                });
            }

            for (c, x, y, w, h, r, corners) in &pc_part.rects {
                let wx = *x;
                let mut wy = *y;
                let ww = *w;
                let mut wh = *h;

                if is_page_content {
                    match clip_to_viewport(wy, wh, content_y, content_y + content_h) {
                        Some((cy, ch)) => { wy = cy; wh = ch; }
                        None => continue,
                    }
                }

                widgets.push(AppWidget {
                    x: wx,
                    y: wy,
                    w: ww,
                    h: wh,
                    color: *c,
                    radius: *r,
                    corners: *corners,
                    fx: WidgetFx::Flat,
                });
            }
            for (id, ix, iy, iw, ih, alpha) in &pc_part.images {
                let (mut wy, mut wh) = (*iy, *ih);
                if is_page_content {
                    match clip_to_viewport(wy, wh, content_y, content_y + content_h) {
                        Some((cy, ch)) => { wy = cy; wh = ch; }
                        None => continue,
                    }
                }
                widgets.push(AppWidget {
                    x: *ix,
                    y: wy,
                    w: *iw,
                    h: wh,
                    color: [0.0; 4],
                    radius: 0.0,
                    corners: (true, true, true, true),
                    fx: WidgetFx::Image { id: *id, alpha: *alpha },
                });
            }
            for (rx, ry, rw, rh, rr, rd, kind) in &pc_part.reliefs {
                let (mut wy, mut wh) = (*ry, *rh);
                if is_page_content {
                    match clip_to_viewport(wy, wh, content_y, content_y + content_h) {
                        Some((cy, ch)) => { wy = cy; wh = ch; }
                        None => continue,
                    }
                }
                widgets.push(AppWidget {
                    x: *rx,
                    y: wy,
                    w: *rw,
                    h: wh,
                    color: [0.0; 4],
                    radius: *rr,
                    corners: (true, true, true, true),
                    fx: match *kind {
                        pages::RELIEF_RAISED => WidgetFx::Boss(*rd),
                        pages::RELIEF_INSET => WidgetFx::Inset(*rd),
                        pages::RELIEF_INSET_FOCUS => WidgetFx::InsetFocus(*rd),
                        pages::RELIEF_RECESSED_FOCUS => WidgetFx::RecessFocus(*rd),
                        _ => WidgetFx::Recess(*rd),
                    },
                });
            }
            for (ax, ay, bx, by, gw, gd, hx, hy, hw, hh) in &pc_part.grooves {
                // Clipped by the HOST rect, not the seam's own span: a groove
                // whose host is scrolled out has nothing left to engrave.
                let (mut wy, mut wh) = (*hy, *hh);
                if is_page_content {
                    match clip_to_viewport(wy, wh, content_y, content_y + content_h) {
                        Some((cy, ch)) => { wy = cy; wh = ch; }
                        None => continue,
                    }
                }
                widgets.push(AppWidget {
                    x: *hx,
                    y: wy,
                    w: *hw,
                    h: wh,
                    color: [0.0; 4],
                    radius: 0.0,
                    corners: (true, true, true, true),
                    fx: WidgetFx::Groove { ax: *ax, ay: *ay, bx: *bx, by: *by, width: *gw, depth: *gd },
                });
            }
            for (btn, action) in &pc_part.buttons {
                let base = btn.base();
                let bg = btn.bg.unwrap_or([0.16, 0.16, 0.24, 1.0]);
                let hover_bg = btn.hover_bg.unwrap_or([0.25, 0.30, 0.26, 1.0]);
                let label = base.label.as_deref().unwrap_or("");
                // The label's font is the configured button font, resolved the
                // same way `Button::paint` resolves it — family AND size. A
                // hardcoded 12.0 here rendered every plain button's label a
                // size or two off whatever the rest of the DE's buttons wear.
                let (label_family, label_size) = {
                    let (family, size) = cce_ui::layout::parse_font_string(&cce_ui::layout::button_font());
                    (family, size.unwrap_or(12.0))
                };
                let label_color = btn.label_color.unwrap_or([0.83, 0.83, 0.83, 1.0]);

                let wx = base.x;
                let mut wy = base.y;
                let ww = base.w;
                let mut wh = base.h;

                if is_page_content {
                    match clip_to_viewport(wy, wh, content_y, content_y + content_h) {
                        Some((cy, ch)) => { wy = cy; wh = ch; }
                        None => continue,
                    }
                }

                let hovering = self.cursor_x >= wx && self.cursor_x <= wx + ww
                    && self.cursor_y >= wy && self.cursor_y <= wy + wh;
                let col = if hovering { hover_bg } else { bg };

                // Flush inset button (control_relief): groove ring down,
                // beveled lip back up, face level with the surface —
                // mirroring Button::paint's relief branch.
                let fx = if cce_ui::layout::control_relief() {
                    WidgetFx::Inset(cce_ui::layout::bevel_width().min(wh * 0.2))
                } else {
                    WidgetFx::Flat
                };
                widgets.push(AppWidget {
                    x: wx,
                    y: wy,
                    w: ww,
                    h: wh,
                    color: col,
                    radius: 4.0, // standard button radius
                    corners: (true, true, true, true),
                    fx,
                });

                // Centring needs the SHAPED width, not `chars * size * 0.65`:
                // that estimate runs wide on a proportional face (and narrow on
                // a large mono one), so a centred label sat visibly off to one
                // side of its own plate — the chooser's Cancel/Select were ~6px
                // left of centre. Measure the run, as `Button::label_width`
                // does, and let `align_text_y` place it vertically, the
                // toolkit's convention for every other text-bearing control.
                let text_w = cce_ui::widget::display::measure_text_width(label, &label_family, label_size);
                let text_x = if btn.justify == cce_ui::widget::Justification::Left {
                    base.x + 8.0
                } else {
                    base.x + (base.w - text_w) / 2.0
                };
                let text_y = cce_ui::layout::align_text_y(base.y, base.h, label_size, 0.0);

                let start_bounds = if is_page_content {
                    [0.0, content_y, self.width as f32, content_y + content_h]
                } else {
                    [0.0, 0.0, self.width as f32, self.height as f32]
                };
                let text_h = label_size * 1.4;
                let occluded = if part_idx < 2 {
                    occlude_against(
                        start_bounds,
                        text_x,
                        text_x + text_w,
                        text_y,
                        text_y + text_h,
                        &[&popover_pc, &context_menu_pc, &dialog_pc],
                    )
                } else {
                    Some(start_bounds)
                };
                let final_button_bounds = match occluded {
                    Some(b) => Some(b),
                    None => {
                        page_buttons.push((btn.clone(), action.clone()));
                        continue;
                    }
                };

                // The family goes with it: measuring in one face and rendering
                // in another is how the centring drifted in the first place.
                texts.push((label.to_string(), label_size, text_x, text_y, label_color, Some(label_family), final_button_bounds));

                page_buttons.push((btn.clone(), action.clone()));
            }
            for (text, size, x, y, col, font, bounds) in &pc_part.texts {
                let clamped_bounds = if is_page_content {
                    let viewport_top = content_y;
                    let viewport_bottom = content_y + content_h;
                    match bounds {
                        Some(b) => Some([
                            b[0],
                            b[1].max(viewport_top),
                            b[2],
                            b[3].min(viewport_bottom),
                        ]),
                        None => Some([
                            0.0,
                            viewport_top,
                            self.width as f32,
                            viewport_bottom,
                        ]),
                    }
                } else {
                    *bounds
                };

                let start_bounds = clamped_bounds.unwrap_or([0.0, 0.0, self.width as f32, self.height as f32]);
                let text_w = text.chars().count() as f32 * size * 0.65;
                let text_h = *size * 1.4;
                let occluded = if part_idx < 2 {
                    occlude_against(
                        start_bounds,
                        *x,
                        *x + text_w,
                        *y,
                        *y + text_h,
                        &[&popover_pc, &context_menu_pc, &dialog_pc],
                    )
                } else {
                    Some(start_bounds)
                };
                let final_bounds = match occluded {
                    Some(b) => Some(b),
                    None => continue,
                };

                texts.push((text.clone(), *size, *x, *y, *col, font.clone(), final_bounds));
            }
        }

        self.widgets = widgets;
        self.texts = texts;
        self.page_buttons = page_buttons;
        self.ui_context.clear_dirty();
        self.needs_rebuild = false;
    }
}

// ── Application Trait Implementation ────────────────────────────────

impl Application for FilesystemApp {
    type Message = Message;

    fn ui_context(&self) -> Option<&cce_ui::context::UiContext> {
        Some(&self.ui_context)
    }

    // The engine ticks the exposed context each loop — this is what drives the
    // dropdown expand/contract animation frames.
    fn ui_context_mut(&mut self) -> Option<&mut cce_ui::context::UiContext> {
        Some(&mut self.ui_context)
    }

    fn is_movable_root_plate_at(&self, px: f32, py: f32) -> bool {
        // 1. If dialog is open, do not drag
        if self.open_with_dialog.is_some() {
            return false;
        }
        // 2. If context menu is visible, do not drag
        if self.context_menu.visible {
            return false;
        }
        // 3. If in the sidebar area (when sidebar is active), do not drag
        if false {
            let sidebar_w = self.paginator.sidebar_w();
            if px <= sidebar_w {
                return false;
            }
        }
        // 4. If over any page button, do not drag
        for (btn, _) in &self.page_buttons {
            let base = btn.base();
            if px >= base.x && px <= base.x + base.w && py >= base.y && py <= base.y + base.h {
                return false;
            }
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

    fn new(_qh: &QueueHandle<cce_ui::engine::EngineState<Self>>, sender: calloop::channel::Sender<Self::Message>) -> Self {
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

        let browse = pages::browse::BrowseState::default();
        let _current_dir = browse.current_dir.clone();

        let pages_names = Page::ALL.iter().map(|p| p.label().to_string()).collect::<Vec<_>>();
        let paginator = cce_ui::widget::Paginator::new(pages_names);
        // These name the visualization rather than the page, so they are not
        // Page::label(). Order MUST track Page::ALL — the selected index is
        // indexed straight into it when the dropdown changes.
        let view_dropdown = cce_ui::widget::Dropdown::new(
            vec!["List".to_string(), "Graph".to_string(), "Space".to_string()],
            0,
        ).with_font_family(&cce_ui::layout::list_font_parsed().0);

        let fs_service = services::fs::FsService::new(sender.clone());
        let initial_w = if select_mode { 900 } else { 1200 };
        let initial_h = if select_mode { 500 } else { 720 };
        let app = Self {
            current_page: Page::Browse,
            browse,
            network: pages::network::NetworkState::default(),
            space: pages::space::SpaceState::default(),
            preview: Default::default(),
            focused_well: FocusedWell::Content,
            preview_dock: Default::default(),
            preview_prior_fracs: None,
            plate_menu_actions: Vec::new(),
            select_mode,
            select_directory,
            save_mode,
            widgets: Vec::new(),
            texts: Vec::new(),
            font_system: cce_ui::create_font_system(),
            needs_rebuild: true,
            width: initial_w,
            height: initial_h,
            scale_factor: 1.0,
            page_buttons: Vec::new(),
            hovered_button: None,
            cursor_x: 0.0,
            cursor_y: 0.0,
            paginator,
            view_dropdown,
            just_initialized: true,
            ui_context: cce_ui::context::UiContext::new(),
            watcher: None,
            fs_service,
            seen_renderer: false,
            context_menu: ContextMenu {
                visible: false,
                x: 0.0,
                y: 0.0,
                w: 120.0,
                h: 0.0,
                options: Vec::new(),
                hovered: None,
            },
            open_with_dialog: None,
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
                self.paginator.set_selected_page(page_idx);
                self.view_dropdown.selected = page_idx;
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

                if let Some(req) = pages::browse::update(&mut self.browse, msg) {
                    self.fs_service.send(req);
                }

                if let Some(path) = is_directory_loaded {
                    self.start_watching(path);
                    // Navigating re-scans the new subtree when Space is up.
                    self.ensure_space_scan();
                }

                // If NavigateTo or SelectEntry happened, update Preview path
                let selected_path = self.browse.selected_path();
                if let Some(path) = selected_path {
                    self.fs_service.send(services::fs::FsRequest::ReadPreview(path));
                } else {
                    pages::preview::update(&mut self.preview, pages::preview::PreviewMessage::Clear);
                }

                if let Some(idx) = self.browse.selected {
                    if let Some(entry) = self.browse.entries.get(idx) {
                        if self.select_mode {
                            self.browse.save_name_box.text = entry.name.clone();
                            if self.browse.save_name_box.editing {
                                self.browse.save_name_box.edit_buffer = entry.name.clone();
                            }
                        }
                    }
                } else {
                    if self.select_mode {
                        self.browse.save_name_box.text.clear();
                        if self.browse.save_name_box.editing {
                            self.browse.save_name_box.edit_buffer.clear();
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
                    let filename = if self.browse.save_name_box.editing {
                        self.browse.save_name_box.edit_buffer.trim()
                    } else {
                        self.browse.save_name_box.text.trim()
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
                self.ui_context.set_focused(&mut tb);
                self.open_with_dialog = Some((path, tb));
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
            Message::OpenWithSubmit => {
                if let Some((path, textbox)) = self.open_with_dialog.take() {
                    let cmd_str = if textbox.editing {
                        textbox.edit_buffer.trim().to_string()
                    } else {
                        textbox.text.trim().to_string()
                    };
                    if !cmd_str.is_empty() {
                        crate::services::fs::spawn_command_for_path(&cmd_str, &path);
                    }
                }
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
            Message::OpenWithCancel => {
                self.open_with_dialog = None;
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
            Message::CopyPath(path) => {
                cce_ui::widget::clipboard::copy_to_clipboard(&path);
            }
        }
    }

    fn tick(&mut self, dt: f32, needs_rebuild: &mut bool) {
        // Pump the widget tick walk (the cce-data-editor pattern): animating
        // widgets — the view dropdown's expand/contract menu — register as
        // tick receivers and report changed until their transition lands;
        // without this the close animation freezes at fully open.
        if self.ui_context.tick(dt) {
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }

        if self.just_initialized {
            self.just_initialized = false;
            if self.save_mode {
                self.browse.save_name_box.focus();
                self.ui_context.set_focused(&mut self.browse.save_name_box);
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
        }

        if self.paginator.tick(dt, &mut self.ui_context) {
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
        if let Some(path) = self.preview.path.clone() {
            if had_texture {
                log::info!(
                    "[preview] renderer replaced; re-reading the preview of {}",
                    path.display()
                );
                self.fs_service.send(services::fs::FsRequest::ReadPreview(path));
            }
        }
        self.needs_rebuild = true;
    }

    fn display_list(&mut self, size: LogicalSize, scale: f64) -> Option<cce_ui::scene::paint::DisplayList> {
        // Phase 6 single paint path: the whole frame — geometry and text — is this one list.
        // rebuild_layout flattens every source (browse/network page, popovers, context menu,
        // dialogs) into self.widgets/self.texts, with popover/dialog occlusion already folded
        // into each text's bounds.
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
                let band_h = SELECT_BAR_H + cce_ui::layout::root_plate_padding();
                pc.recess_edges(
                    Rect { x: 0.0, y: fh - band_h, width: fw, height: band_h },
                    (0.0, 0.0, 0.0, 0.0),
                    wall,
                    (true, false, false, false),
                );
            }
        }

        for w in &self.widgets {
            let rect = Rect { x: w.x, y: w.y, width: w.w, height: w.h };
            let radii = (w.radius, w.radius, w.radius, w.radius);
            match w.fx {
                WidgetFx::Bevel(depth) => pc.bevel(rect, radii, &cce_ui::scene::Material::from_fill(w.color), depth),
                WidgetFx::Plate(depth) => pc.plate(rect, radii, &cce_ui::scene::Material::from_fill(w.color), depth),
                WidgetFx::Boss(depth) => pc.boss(rect, radii, depth),
                WidgetFx::Recess(depth) => pc.recess(rect, radii, depth),
                WidgetFx::RecessFocus(depth) => {
                    let hc = cce_ui::color::highlight_primary_color();
                    pc.recess_tinted(rect, radii, depth, [hc[0], hc[1], hc[2]]);
                }
                WidgetFx::Inset(depth) => pc.inset_plate(rect, radii, cce_ui::scene::Material::face(w.color).as_ref(), depth),
                WidgetFx::InsetFocus(depth) => {
                    pc.inset_plate_tinted(rect, radii, cce_ui::scene::Material::face(w.color).as_ref(), depth, cce_ui::widget::ControlPlate::focus_tint())
                }
                WidgetFx::Image { id, alpha } => pc.image(id, rect, alpha),
                WidgetFx::Groove { ax, ay, bx, by, width, depth } => {
                    pc.groove((ax, ay), (bx, by), width, depth, rect)
                }
                WidgetFx::Flat => {
                    if w.radius > 0.1 {
                        pc.rounded_rect(rect, w.radius, w.corners, w.color);
                    } else {
                        pc.quad(rect, w.color);
                    }
                }
            }
        }
        for (text, font_size, x, y, col, font, bounds) in &self.texts {
            pc.text_with(
                text.clone(),
                *x,
                *y,
                *font_size,
                [
                    (col[0] * 255.0) as u8,
                    (col[1] * 255.0) as u8,
                    (col[2] * 255.0) as u8,
                ],
                font.clone(),
                *bounds,
            );
        }
        // The toolkit's shared context menu (breadcrumb segments, config-bound
        // controls) draws into the app's display list like every popover. Its
        // labels carry bounds equal to the menu rect — the engine's popover
        // occlusion clamp exempts exactly that, so they render inside the menu
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
                    band.0 + 10.0,
                    band.1 + (band.3 - 13.0) / 2.0,
                    13.0,
                    [0xc8, 0xc8, 0xd4],
                    None,
                    Some([band.0, band.1, band.0 + band.2 - dock::CORNER_INSET - dock::CORNER_R, band.1 + band.3]),
                );
            }
            if let Some(c) = dock::corner_center(band, stubbed) {
                let emphasized = dock::corner_hit(c, self.cursor_x, self.cursor_y)
                    || !self.plate_menu_actions.is_empty();
                dock::draw_corner_dot(&mut pc, c, emphasized);
            }
        }

        // while page text beneath stays clamped.
        // The shared menu paints itself as a lit plate (cce-ui), so the
        // breadcrumb's copy-path menu is the same frosted glass as the row
        // menu above rather than the flat quads it used to stack. Labels come
        // with it: `paint_with_labels` carries the menu font's family, which
        // the hand-rolled `paint` + `text_labels()` loop here did not.
        cce_ui::widget::context_menu::paint_with_labels(&mut pc);
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
        if self.open_with_dialog.is_some() {
            if let Some((_path, textbox)) = &mut self.open_with_dialog {
                let root = textbox.id();
                let _ = self.ui_context.propagate_event(&mv, root);
            }
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

        if self.context_menu.visible {
            let cx = self.context_menu.x;
            let cy = self.context_menu.y;
            let cw = self.context_menu.w;
            let ch = self.context_menu.h;
            let was_hovered = self.context_menu.hovered;
            self.context_menu.hovered = None;
            if pos.x >= cx && pos.x <= cx + cw && pos.y >= cy && pos.y <= cy + ch {
                let idx = ((pos.y - cy) / ROW_H) as usize;
                if idx < self.context_menu.options.len() && idx > 0 {
                    self.context_menu.hovered = Some(idx);
                }
            }
            if self.context_menu.hovered != was_hovered {
                changed = true;
            }
            if changed {
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
        } else if self.current_page == Page::Space {
            if self.space_split.cursor_moved(pos.x, pos.y) {
                changed = true;
            }
        }

        if !self.select_mode {
            // Self-routing composite: handle_event, not propagate — the router's
            // children-first descent would let the embedded strip consume this.
            if self.paginator.handle_event(&mv, &mut self.ui_context) {
                changed = true;
            }
        }

        {
            let root = self.view_dropdown.id();
            if self.ui_context.propagate_event(&mv, root) {
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

        // Repaint only when the hovered page button actually changes. Page-button hover is
        // a binary color decided at rebuild time (see rebuild_layout), so idle pointer moves
        // don't need a redraw. Rebuilding unconditionally re-commits the translucent surface
        // every move, which makes the compositor re-blur the backdrop continuously.
        let hovered_button = self.page_buttons.iter().position(|(btn, _)| {
            let base = btn.base();
            self.cursor_x >= base.x && self.cursor_x <= base.x + base.w
                && self.cursor_y >= base.y && self.cursor_y <= base.y + base.h
        });
        if hovered_button != self.hovered_button {
            self.hovered_button = hovered_button;
            changed = true;
        }

        if changed {
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }
    }
    fn handle_mouse_input(&mut self, button: MouseButton, state: ElementState, pos: LogicalPosition, needs_rebuild: &mut bool) -> Option<Self::Message> {
        if button != MouseButton::Left && button != MouseButton::Right {
            return None;
        }

        // The preview pane's plate-dock menu (cce-ui RFC 7c-2) routes BEFORE
        // the generic context-menu dispatch below: that path targets widget
        // ContextActions, not these rows. Keyed on RELEASE: in this app the
        // routed-widget path upstream consumes left PRESSES before the hook
        // (only releases reliably arrive here).
        if button == MouseButton::Left
            && state == ElementState::Released
            && !self.plate_menu_actions.is_empty()
            && cce_ui::widget::context_menu::is_visible()
        {
            let (px, py) = (pos.x as f32, pos.y as f32);
            let picked = cce_ui::widget::context_menu::row_at(px, py)
                .and_then(|row| self.plate_menu_actions.get(row).copied());
            cce_ui::widget::context_menu::hide();
            self.plate_menu_actions.clear();
            if let Some(action) = picked {
                self.dispatch_preview_dock(action);
            }
            *needs_rebuild = true;
            self.needs_rebuild = true;
            return None;
        }

        // A left RELEASE on the preview pane's corner control opens its menu
        // (the designer also opens on release; presses do not reliably reach
        // this hook — see above). No press arming: the arming protocol
        // disambiguates click from dock-DRAG, and a single pane has nowhere
        // to dock.
        if button == MouseButton::Left && state == ElementState::Released {
            use cce_ui::widget::plate_dock as dock;
            let band = self.preview_dock_band();
            if let Some(c) = dock::corner_center(band, self.preview_dock.stubbed()) {
                if dock::corner_hit(c, pos.x as f32, pos.y as f32) {
                    let rows = dock::standard_menu(self.preview_dock, false);
                    let (labels, actions): (Vec<String>, Vec<_>) = rows.into_iter().unzip();
                    cce_ui::widget::context_menu::show(
                        c.0 - dock::CORNER_R,
                        c.1 + dock::CORNER_R,
                        labels,
                        0,
                        self.paginator.base().id(),
                    );
                    self.plate_menu_actions = actions;
                    *needs_rebuild = true;
                    self.needs_rebuild = true;
                    return None;
                }
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

        if let Some((_path, textbox)) = &mut self.open_with_dialog {
            let r = open_with_rects(self.width as f32, self.height as f32);
            let (dialog_x, dialog_y, dialog_w, dialog_h) = (r.x, r.y, r.w, r.h);
            let (tb_x, tb_y, tb_w, tb_h) = r.tb;
            let (btn_cancel_x, btn_cancel_y, btn_cancel_w, btn_cancel_h) = r.cancel;
            let (btn_open_x, btn_open_y, btn_open_w, btn_open_h) = r.open;

            if state == ElementState::Pressed {
                let clicked_inside = pos.x >= dialog_x && pos.x <= dialog_x + dialog_w && pos.y >= dialog_y && pos.y <= dialog_y + dialog_h;
                if !clicked_inside {
                    textbox.unfocus();
                    self.ui_context.clear_focus();
                    return Some(Message::OpenWithCancel);
                }

                if button == MouseButton::Left {
                    if pos.x >= tb_x && pos.x <= tb_x + tb_w && pos.y >= tb_y && pos.y <= tb_y + tb_h {
                        let ev = cce_ui::widget::Event::MouseButton { button, state, x: pos.x, y: pos.y, local_x: pos.x, local_y: pos.y };
                        let root = textbox.id();
                        if self.ui_context.propagate_event(&ev, root) {
                            self.ui_context.set_focused(textbox);
                            *needs_rebuild = true;
                            self.needs_rebuild = true;
                        }
                    } else if pos.x >= btn_cancel_x && pos.x <= btn_cancel_x + btn_cancel_w && pos.y >= btn_cancel_y && pos.y <= btn_cancel_y + btn_cancel_h {
                        textbox.unfocus();
                        self.ui_context.clear_focus();
                        return Some(Message::OpenWithCancel);
                    } else if pos.x >= btn_open_x && pos.x <= btn_open_x + btn_open_w && pos.y >= btn_open_y && pos.y <= btn_open_y + btn_open_h {
                        textbox.unfocus();
                        self.ui_context.clear_focus();
                        return Some(Message::OpenWithSubmit);
                    } else {
                        textbox.unfocus();
                        self.ui_context.clear_focus();
                        *needs_rebuild = true;
                        self.needs_rebuild = true;
                    }
                }
            } else {
                if button == MouseButton::Left && textbox.editing {
                    let ev = cce_ui::widget::Event::MouseButton { button, state, x: pos.x, y: pos.y, local_x: pos.x, local_y: pos.y };
                    let root = textbox.id();
                    if self.ui_context.propagate_event(&ev, root) {
                        *needs_rebuild = true;
                        self.needs_rebuild = true;
                    }
                }
            }
            return None;
        }

        if self.context_menu.visible {
            if state == ElementState::Pressed {
                let cx = self.context_menu.x;
                let cy = self.context_menu.y;
                let cw = self.context_menu.w;
                let ch = self.context_menu.h;

                let mut clicked_option = None;
                if pos.x >= cx && pos.x <= cx + cw && pos.y >= cy && pos.y <= cy + ch {
                    let idx = ((pos.y - cy) / ROW_H) as usize;
                    if idx < self.context_menu.options.len() && idx > 0 {
                        clicked_option = self.context_menu.options[idx].1.clone();
                    }
                }

                self.context_menu.visible = false;
                *needs_rebuild = true;
                self.needs_rebuild = true;

                if let Some(action) = clicked_option {
                    return Some(action);
                }

                if button == MouseButton::Right {
                    // Fall through to allow right-clicking another item to show a new context menu
                } else {
                    return None;
                }
            } else {
                return None;
            }
        }

        if button == MouseButton::Right && state == ElementState::Pressed {
            let breadcrumb = match self.current_page {
                Page::Browse => &mut self.browse.breadcrumb,
                Page::Network => &mut self.network.breadcrumb,
                Page::Space => &mut self.space.breadcrumb,
            };
            if breadcrumb.hit_test(pos.x, pos.y, &self.ui_context) {
                let ev = cce_ui::widget::Event::MouseButton { button, state, x: pos.x, y: pos.y, local_x: pos.x, local_y: pos.y };
                let root = breadcrumb.id();
                // The breadcrumb opens the toolkit's shared context menu itself
                // (open_context_menu → segment header + Copy Path); the app just
                // routes the event and redraws — no app-side menu duplicate.
                if self.ui_context.propagate_event(&ev, root) {
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

                        if is_trash_dir {
                            options.push(("Restore".to_string(), Some(Message::Browse(pages::browse::BrowseMessage::RestoreEntry(idx)))));
                            options.push(("Delete Permanently".to_string(), Some(Message::Browse(pages::browse::BrowseMessage::DeleteEntryPermanent(idx)))));
                            options.push(("Empty Trash".to_string(), Some(Message::Browse(pages::browse::BrowseMessage::EmptyTrash))));
                        } else {
                            options.push(("Delete".to_string(), Some(Message::Browse(pages::browse::BrowseMessage::DeleteEntry(idx)))));
                            if let Some(trash_files) = services::trash::files_dir() {
                                options.push(("Open Trash".to_string(), Some(Message::Browse(pages::browse::BrowseMessage::NavigateToPath(trash_files)))));
                            }
                        }

                        // Calculate width
                        let (menu_w, menu_h) = context_menu_size(&options);

                        self.context_menu = ContextMenu {
                            visible: true,
                            x: pos.x,
                            y: pos.y,
                            w: menu_w,
                            h: menu_h,
                            options,
                            hovered: None,
                        };
                        *needs_rebuild = true;
                        self.needs_rebuild = true;
                        return None;
                    }
                }
            }
        }

        let mut changed = false;

        log::debug!("MOUSE INPUT: {:?} {:?} pos=({}, {})", button, state, pos.x, pos.y);
        let ev = cce_ui::widget::Event::MouseButton { button, state, x: pos.x, y: pos.y, local_x: pos.x, local_y: pos.y };
        let menu_match = !self.select_mode && {
            // Self-routing composite: handle_event, not propagate (see pointer move).
            self.paginator.handle_event(&ev, &mut self.ui_context)
        };
        if menu_match {
            if let Some((idx, _)) = self.paginator.menu_click() {
                if idx < Page::ALL.len() {
                    self.ui_context.clear_focus();
                    self.current_page = Page::ALL[idx];
                }
            }
            *needs_rebuild = true;
            self.needs_rebuild = true;
            return None;
        }

        if { let root = self.view_dropdown.id(); self.ui_context.propagate_event(&ev, root) } {
            *needs_rebuild = true;
            self.needs_rebuild = true;
            if self.view_dropdown.take_change() {
                // Indexed off Page::ALL rather than hand-mapped: the old
                // `== 0 { Browse } else { Network }` silently sent every
                // entry past the first to Network.
                let new_page = Page::ALL
                    .get(self.view_dropdown.selected)
                    .copied()
                    .unwrap_or(Page::Browse);
                return Some(Message::SwitchPage(new_page));
            }
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
        // dropdown, divider grab) has already returned, so a release reaching
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
            if self.select_mode {
                if { let root = self.browse.save_name_box.id(); self.ui_context.propagate_event(&ev, root) } {
                    if state == ElementState::Pressed {
                        self.ui_context.set_focused(&mut self.browse.save_name_box);
                    }
                    *needs_rebuild = true;
                    self.needs_rebuild = true;
                }
            }
            if self.browse.search_visible
                && button == MouseButton::Left
                && { let root = self.browse.search_box.id(); self.ui_context.propagate_event(&ev, root) }
            {
                self.ui_context.set_focused(&mut self.browse.search_box);
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
            if button == MouseButton::Left && state == ElementState::Pressed {
                if self.browse.breadcrumb.hit_test(pos.x, pos.y, &self.ui_context) {
                    if { let root = self.browse.breadcrumb.id(); self.ui_context.propagate_event(&ev, root) } {
                        if let Some(seg) = self.browse.breadcrumb.path_click() {
                            let target_path = pages::browse::path_to_segment(&self.browse.current_dir, seg);
                            self.fs_service.send(services::fs::FsRequest::ReadDirectory(target_path));
                            *needs_rebuild = true;
                            self.needs_rebuild = true;
                        }
                    }
                }
            }
        } else if self.current_page == Page::Network {
            if button == MouseButton::Left {
                if state == ElementState::Pressed {
                    if self.network.breadcrumb.hit_test(pos.x, pos.y, &self.ui_context) {
                        if { let root = self.network.breadcrumb.id(); self.ui_context.propagate_event(&ev, root) } {
                            if let Some(seg) = self.network.breadcrumb.path_click() {
                                let target_path = pages::browse::path_to_segment(&self.browse.current_dir, seg);
                                self.fs_service.send(services::fs::FsRequest::ReadDirectory(target_path));
                                changed = true;
                            }
                        }
                    } else if {
                        // Routed press: a node grab records the drag target; the router's
                        // DragStart replaces the immediate drag_begin (3px threshold).
                        let root = self.network.graph.id();
                        self.ui_context.propagate_event(&ev, root)
                    } {
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
            if let Some(node_sel) = self.network.graph.selected_node() {
                if node_sel >= offset {
                    let entry_idx = node_sel - offset;
                    if self.browse.selected != Some(entry_idx) {
                        self.browse.selected = Some(entry_idx);
                        if let Some(entry) = self.browse.entries.get(entry_idx) {
                            if self.select_mode {
                                self.browse.save_name_box.text = entry.name.clone();
                                if self.browse.save_name_box.editing {
                                    self.browse.save_name_box.edit_buffer = entry.name.clone();
                                }
                            }
                            self.fs_service.send(services::fs::FsRequest::ReadPreview(entry.path.clone()));
                        }
                        changed = true;
                    }
                } else {
                    if self.browse.selected.is_some() {
                        self.browse.selected = None;
                        pages::preview::update(&mut self.preview, pages::preview::PreviewMessage::Clear);
                        changed = true;
                    }
                }
            } else {
                if self.browse.selected.is_some() {
                    self.browse.selected = None;
                    pages::preview::update(&mut self.preview, pages::preview::PreviewMessage::Clear);
                    changed = true;
                }
            }

            // Sync double click navigation
            if let Some(dbl_idx) = self.network.graph.double_clicked_node() {
                self.network.graph.clear_double_clicked_node();
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
                            self.browse.save_name_box.text = entry.name.clone();
                            if self.browse.save_name_box.editing {
                                self.browse.save_name_box.edit_buffer = entry.name.clone();
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
                if self.space.breadcrumb.hit_test(pos.x, pos.y, &self.ui_context) {
                    if { let root = self.space.breadcrumb.id(); self.ui_context.propagate_event(&ev, root) } {
                        if let Some(seg) = self.space.breadcrumb.path_click() {
                            let target_path = pages::browse::path_to_segment(&self.browse.current_dir, seg);
                            self.fs_service.send(services::fs::FsRequest::ReadDirectory(target_path));
                            changed = true;
                        }
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
                        self.fs_service.send(services::fs::FsRequest::ReadPreview(tile_path));
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
            let clicked_search = self.current_page == Page::Browse && self.browse.search_visible && self.browse.search_box.hit_test(pos.x, pos.y, &self.ui_context);
            let clicked_save_name = self.select_mode && self.current_page == Page::Browse && self.browse.save_name_box.hit_test(pos.x, pos.y, &self.ui_context);
            if !clicked_search {
                self.browse.search_box.unfocus();
            }
            if !clicked_save_name {
                self.browse.save_name_box.unfocus();
            }
            if !clicked_search && !clicked_save_name {
                self.ui_context.clear_focus();
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
        }

        if button == MouseButton::Left && state == ElementState::Released {
            for (btn, action) in &self.page_buttons {
                let base = btn.base();
                if pos.x >= base.x && pos.x <= base.x + base.w && pos.y >= base.y && pos.y <= base.y + base.h {
                    *needs_rebuild = true;
                    self.needs_rebuild = true;
                    return Some(action.clone());
                }
            }
        }

        None
    }

    fn handle_mouse_wheel(&mut self, delta: &MouseScrollDelta, pos: LogicalPosition, needs_rebuild: &mut bool) {
        // Every page shares the preview pane on the right.
        if matches!(self.current_page, Page::Browse | Page::Network | Page::Space) {
            // The pane hit-tests its own laid-out rect and consumes any wheel
            // over its content region, scrolled or not.
            if let Some(changed) = (!self.preview_dock.collapsed)
                .then(|| self.preview.wheel(delta, pos.x as f32, pos.y as f32))
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
            let ev = cce_ui::widget::Event::MouseWheel { delta: *delta, x: pos.x as f32, y: pos.y as f32, local_x: pos.x as f32, local_y: pos.y as f32 };
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

    fn handle_key_input(&mut self, event: &KeyEvent, needs_rebuild: &mut bool) -> Option<Self::Message> {
        if event.state != ElementState::Pressed {
            return None;
        }

        if let Some((_path, textbox)) = &mut self.open_with_dialog {
            if event.logical_key == cce_ui::widget::Key::Named(cce_ui::widget::NamedKey::Enter) {
                textbox.unfocus();
                self.ui_context.clear_focus();
                return Some(Message::OpenWithSubmit);
            }
            if event.logical_key == cce_ui::widget::Key::Named(cce_ui::widget::NamedKey::Escape) {
                textbox.unfocus();
                self.ui_context.clear_focus();
                return Some(Message::OpenWithCancel);
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

        if self.context_menu.visible {
            if event.logical_key == cce_ui::widget::Key::Named(cce_ui::widget::NamedKey::Escape) {
                self.context_menu.visible = false;
                *needs_rebuild = true;
                self.needs_rebuild = true;
                return None;
            }
        }

        // An open view dropdown takes the keyboard — Escape closes it, arrows
        // move the hover, Enter selects — routed to the widget exactly like
        // its mouse events are (see handle_mouse_input). Gated on `open`:
        // this app routes keys per feature rather than to a whole tree, and
        // the dropdown was simply never on the list, which left its own
        // Escape handling unreachable.
        if self.view_dropdown.open {
            let kev = cce_ui::widget::Event::KeyInput(event.clone());
            if { let root = self.view_dropdown.id(); self.ui_context.propagate_event(&kev, root) } {
                *needs_rebuild = true;
                self.needs_rebuild = true;
                if self.view_dropdown.take_change() {
                    let new_page = Page::ALL
                        .get(self.view_dropdown.selected)
                        .copied()
                        .unwrap_or(Page::Browse);
                    return Some(Message::SwitchPage(new_page));
                }
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
                    self.browse.search_box.focus();
                    self.ui_context.set_focused(&mut self.browse.search_box);
                    *needs_rebuild = true;
                    self.needs_rebuild = true;
                    return None;
                }
            } else {
                let close_key = cce_ui::color::list_close_search_key();
                if cce_ui::widget::match_key_shortcut(event, &close_key) {
                    self.browse.search_visible = false;
                    self.browse.search_box.unfocus();
                    self.ui_context.clear_focus();
                    *needs_rebuild = true;
                    self.needs_rebuild = true;
                    return Some(Message::Browse(pages::browse::BrowseMessage::SearchChanged(String::new())));
                }
                if { let kev = cce_ui::widget::Event::KeyInput(event.clone()); let root = self.browse.search_box.id(); self.ui_context.propagate_event(&kev, root) } {
                    *needs_rebuild = true;
                    self.needs_rebuild = true;
                    if self.browse.search_box.take_change() {
                        return Some(Message::Browse(pages::browse::BrowseMessage::SearchChanged(
                            self.browse.search_box.text.clone()
                        )));
                    }
                    return None;
                }
            }
        }

        // If the save_name_box is focused, forward key inputs to it
        if self.select_mode && self.browse.save_name_box.editing {
            if { let kev = cce_ui::widget::Event::KeyInput(event.clone()); let root = self.browse.save_name_box.id(); self.ui_context.propagate_event(&kev, root) } {
                *needs_rebuild = true;
                self.needs_rebuild = true;
                if event.logical_key == cce_ui::widget::Key::Named(cce_ui::widget::NamedKey::Enter) {
                    self.browse.save_name_box.unfocus();
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
                if let Some(idx) = self.browse.selected {
                    if let Some(entry) = self.browse.entries.get(idx) {
                        if entry.is_dir && !is_project_dir(&entry.path) {
                            return Some(Message::Browse(pages::browse::BrowseMessage::NavigateTo(idx)));
                        }
                    }
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

// ── Main ────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    cce_ui::engine::run::<FilesystemApp>();
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIN: [f32; 4] = [0.0, 0.0, 1000.0, 700.0];

    fn rect_overlay(x: f32, y: f32, w: f32, h: f32) -> pages::PageContent {
        let mut pc = pages::PageContent::new();
        pc.rect([0.0, 0.0, 0.0, 1.0], x, y, w, h);
        pc
    }

    fn plate_overlay(x: f32, y: f32, w: f32, h: f32) -> pages::PageContent {
        let mut pc = pages::PageContent::new();
        pc.plates.push(([0.0, 0.0, 0.0, 1.0], x, y, w, h, 8.0, 4.0));
        pc
    }

    /// A box the overlay covers outright is discarded, not clipped.
    #[test]
    fn a_covered_box_is_dropped() {
        let menu = rect_overlay(300.0, 253.0, 430.0, 120.0);
        assert!(occlude_against(WIN, 320.0, 400.0, 300.0, 317.0, &[&menu]).is_none());
    }

    /// An overlay's face is a plate now, so the sweep has to see plates or the
    /// page's text draws straight through the surface covering it.
    #[test]
    fn a_plate_occludes_like_a_rect() {
        let menu = plate_overlay(300.0, 253.0, 430.0, 120.0);
        assert!(occlude_against(WIN, 320.0, 400.0, 300.0, 317.0, &[&menu]).is_none());
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

        let b = occlude_against(WIN, 40.0, 420.0, 288.0, 305.0, &[&menu, &hover])
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
        let b = occlude_against(WIN, 320.0, 400.0, 240.0, 257.0, &[&menu]).expect("straddles the top");
        assert_eq!(b[3], 253.0, "clipped to the strip above the menu");
        assert_eq!(b[2], WIN[2], "and not trimmed horizontally");
    }

    /// Of the two sides an overlay leaves, the cut keeps the wider one — the most
    /// of the box that a single clip rect can still express.
    #[test]
    fn the_cut_keeps_the_wider_side() {
        // Overlay near the box's right end: the long run before it survives.
        let right = rect_overlay(380.0, 0.0, 200.0, 700.0);
        let b = occlude_against(WIN, 40.0, 420.0, 288.0, 305.0, &[&right]).unwrap();
        assert_eq!(b[2], 380.0);
        assert_eq!(b[0], WIN[0]);

        // Overlay near the left end: the run after it survives instead.
        let left = rect_overlay(0.0, 0.0, 80.0, 700.0);
        let b = occlude_against(WIN, 40.0, 420.0, 288.0, 305.0, &[&left]).unwrap();
        assert_eq!(b[0], 80.0);
        assert_eq!(b[2], WIN[2]);
    }
}
