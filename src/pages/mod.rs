pub mod browse;
pub mod preview;
pub mod network;
pub mod space;

use cce_ui::widget::Owned;
use cce_ui::layout::RenderTarget;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Browse,
    Network,
    Space,
}

impl Page {
    pub const ALL: [Page; 3] = [
        Page::Browse,
        Page::Network,
        Page::Space,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Page::Browse => "Browse",
            Page::Network => "Network",
            Page::Space => "Space",
        }
    }

    /// The page's row in the breadcrumb's view menu. These name the
    /// visualization rather than the page, so they are not [`Self::label`].
    pub fn view_label(self) -> &'static str {
        match self {
            Page::Browse => "List",
            Page::Network => "Graph",
            Page::Space => "Space",
        }
    }
}

/// The breadcrumb's height on every page. Its segments are button plates, so
/// it is one button tall: the toolkit's control height (as the widget's own
/// `intrinsic_size` says), not a number of this crate's.
pub fn breadcrumb_h() -> f32 {
    cce_ui::layout::button_height()
}

/// Lay out, render and carve a page's breadcrumb: the pane's full width along
/// its top edge, the same on every page, so switching views leaves it where it
/// was. Returns the top of the content under it, one plate gap below.
///
/// Network and Space used to place theirs 4px in, 6px down and 16px narrower
/// than Browse, an inset left over from before the pane rect carried it, and
/// started their content 28px and 34px down against Browse's 24 + gap.
pub fn breadcrumb_header(
    pc: &mut PageContent,
    breadcrumb: &mut Owned<cce_ui::widget::Adapted<cce_ui::widget::Breadcrumb>>,
    cx: f32,
    cy: f32,
    cw: f32,
    ctx: &mut cce_ui::context::UiContext,
) -> f32 {
    let h = breadcrumb_h();
    cce_ui::layout::render_widget(pc, breadcrumb, cx, cy, cw, h, ctx);
    let rect = cce_ui::scene::layout::Rect { x: cx, y: cy, width: cw, height: h };
    breadcrumb_relief(pc, breadcrumb, rect);
    cy + h + cce_ui::layout::root_plate_gap()
}

/// Mirror the breadcrumb's relief into a flat-path [`PageContent`]: the
/// full-width recessed well, the ONE raised plate the segment run shares, and a
/// slanted seam engraved at each boundary between two segments.
///
/// `render_widget` drops the relief prims `Breadcrumb::paint` emits, so every
/// page that shows a breadcrumb has to carve it app-side — this is that carve,
/// in one place, since all three pages want it identically.
///
/// **Audited against `Breadcrumb::paint` and deliberately left page-side** —
/// it does NOT need the treatment the view dropdown's ring needed (carved into
/// `window_pc` after the pages had laid it out; the dropdown is gone since
/// 2026-10-06, its List/Graph/Space rows moved into this breadcrumb's context
/// menu), for two reasons that are easy to assume away:
///
/// - *The depths already agree.* Each `relief_*`/`groove` helper derives depth
///   from the height it is handed, and all three here are handed what the
///   widget uses: the well from `rect.height`, the plate from `rh`, and the
///   seams from the run as host — the widget engraves the seams at the RUN's
///   depth, not the well's. Nothing is pre-expanded, so nothing drifts the way
///   the dropdown's ring did.
/// - *The rect is fresh.* Every caller builds it as a literal on the line after
///   laying the breadcrumb out, rather than reading it back off the widget, so
///   there is no previous-frame rect to pick up.
///
/// Nor does the `window_pc`-vs-`pc` split matter here, though it did for the
/// dropdown's ring. `display_list` emits ALL of a `PageContent`'s rects
/// before ALL of its reliefs, so the call order within `pc` is irrelevant; only
/// a different PageContent could reorder these. The only quad the breadcrumb
/// puts under the carve is the hover tint, and that is inset to the seam's
/// furthest lean by construction, so it does not overlap the grooves. Moving
/// this carve to `window_pc` would buy nothing.
pub fn breadcrumb_relief(
    pc: &mut PageContent,
    breadcrumb: &Owned<cce_ui::widget::Adapted<cce_ui::widget::Breadcrumb>>,
    rect: cce_ui::scene::layout::Rect,
) {
    let r = cce_ui::layout::dropdown_corner_radius();
    let Some(run) = breadcrumb.run_box(rect) else { return };
    let (rx, ry, rw, rh) = run;
    // The dropdown's flush inset plate on the segment run (mirroring
    // Breadcrumb::paint's relief branch) — the full-rect recessed well and
    // the raised run inside it are gone with the restyle.
    //
    // Face AND ring, through the `inset_plate` bridge, because the face is
    // the dropdown's configured fill: `render_widget` offers that fill for a
    // Dropdown through a per-type hook (layout.rs) but has no Breadcrumb arm,
    // so a run carved here with no face would keep showing the window plate
    // while the DE's dropdowns went opaque. It reads
    // `dropdown_background_color` too, so they match under any config —
    // transparent leaves the plate as the face.
    let radius = r.min(rh * 0.5);
    let depth = cce_ui::layout::bevel_width().min(rh * 0.2);
    let raw = cce_ui::color::dropdown_background_color();
    let face = if raw[3] > 0.001 {
        let mut c = raw;
        c[3] = 1.0;
        c
    } else {
        [0.0; 4]
    };
    pc.inset_plate(face, rx, ry, rw, rh, radius, depth);
    for (a, b) in breadcrumb.seams(rect) {
        pc.groove(a, b, cce_ui::widget::Breadcrumb::SEAM_WIDTH, run);
    }
}

pub const RELIEF_RECESSED: u8 = 0;
pub const RELIEF_RAISED: u8 = 1;
pub const RELIEF_INSET: u8 = 2;
/// A recessed well with pointer focus: renders as the tinted carve — the
/// wrapped accent glint REPLACING the relief lighting (the DE's one focus
/// language, same treatment as a focused plate's ring).
pub const RELIEF_RECESSED_FOCUS: u8 = 3;
/// [`RELIEF_INSET`] with keyboard focus: the flush plate's rim lit in the
/// highlight — the ring a focused control plate wears (`ControlPlate::with_tint`).
pub const RELIEF_INSET_FOCUS: u8 = 4;

/// A stroked shape a widget draws: a graph's wire (a line, or an arc at a
/// rounded bend) or its port (a disc). Coordinates as `PaintCtx` takes them;
/// an arc's `radius` is its OUTER edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Stroke {
    Line { x1: f32, y1: f32, x2: f32, y2: f32, thickness: f32, cap: cce_ui::scene::paint::Cap },
    Arc { cx: f32, cy: f32, radius: f32, thickness: f32, start: f32, end: f32 },
    Disc { cx: f32, cy: f32, radius: f32 },
}

impl Stroke {
    /// The shape's bounding box `(x, y, w, h)` — what culls it.
    pub fn bounds(&self) -> (f32, f32, f32, f32) {
        match *self {
            Stroke::Line { x1, y1, x2, y2, thickness, .. } => {
                let h = thickness * 0.5;
                (x1.min(x2) - h, y1.min(y2) - h, (x1 - x2).abs() + thickness, (y1 - y2).abs() + thickness)
            }
            Stroke::Arc { cx, cy, radius, .. } | Stroke::Disc { cx, cy, radius } => {
                (cx - radius, cy - radius, 2.0 * radius, 2.0 * radius)
            }
        }
    }
}

pub struct PageContent {
    pub rects: Vec<([f32; 4], f32, f32, f32, f32, f32, (bool, bool, bool, bool))>,
    pub texts: Vec<(String, f32, f32, f32, [f32; 4], Option<String>, Option<[f32; 4]>)>,
    pub buttons: Vec<(Owned<cce_ui::widget::Adapted<cce_ui::widget::Button>>, crate::Message)>,
    /// Relief steps for the control_relief styling — (x, y, w, h, radius, depth,
    /// kind: [`RELIEF_RECESSED`]/[`RELIEF_RAISED`]/[`RELIEF_INSET`]). The flat rects
    /// own the faces; these are the edges-only walls emitted over them (the
    /// ParametersBg::reliefs idiom for flat-view hosts).
    pub reliefs: Vec<(f32, f32, f32, f32, f32, f32, u8)>,
    /// Engraved lines over the flat rects — (ax, ay, bx, by, width, depth) plus
    /// the (x, y, w, h) of the surface being engraved, which the shading fades
    /// out against. Unlike [`reliefs`] these are not axis-aligned: this is the
    /// breadcrumb's slanted segment seams.
    ///
    /// [`reliefs`]: PageContent::reliefs
    pub grooves: Vec<(f32, f32, f32, f32, f32, f32, f32, f32, f32, f32)>,
    /// GPU-textured quads — (image id from `cce_ui::vk::upload_rgba`, x, y, w, h,
    /// alpha). Drawn after the part's rects, so a fill emitted earlier is the floor
    /// beneath the image and overlay parts still cover it.
    pub images: Vec<(u32, f32, f32, f32, f32, f32)>,
    /// cce-icons glyphs — (name, x, y, w, h, colour, clip `[l, t, r, b]`).
    /// The colour is a text colour (raw sRGB, alpha = opacity), so a glyph and
    /// the label beside it match; drawn through `PaintCtx::icon` in the part's
    /// order, after its images. The clip cuts a glyph scrolled half under an
    /// edge rather than squashing it, as text bounds cut a label.
    pub icons: Vec<(String, f32, f32, f32, f32, [f32; 4], Option<[f32; 4]>)>,
    /// Strokes, arcs and discs — (shape, colour, clip `[l, t, r, b]`): a
    /// widget's wires and ports as `render_widget` hands them over
    /// (`RenderTarget::line` / `arc` / `circle`). The graph page's wires.
    pub strokes: Vec<(Stroke, [f32; 4], Option<[f32; 4]>)>,
    /// Lit plates — (color, x, y, w, h, radius, depth). Unlike [`reliefs`], a plate
    /// owns its FILL as well as its edge: one primitive carrying a rounded face and
    /// the rolled, lit perimeter, shaded in a single lighting evaluation. That is
    /// what the pane plates and the preview stub wear, so a floating surface
    /// (the context menu) reads as the same material rather than as a flat chip
    /// inside a drawn frame.
    ///
    /// [`reliefs`]: PageContent::reliefs
    pub plates: Vec<([f32; 4], f32, f32, f32, f32, f32, f32)>,
}

impl PageContent {
    pub fn new() -> Self {
        Self {
            rects: Vec::new(),
            texts: Vec::new(),
            buttons: Vec::new(),
            reliefs: Vec::new(),
            grooves: Vec::new(),
            images: Vec::new(),
            icons: Vec::new(),
            strokes: Vec::new(),
            plates: Vec::new(),
        }
    }

    /// Move every part of `other` into this content.
    ///
    /// Field-complete by construction: this replaces four hand-listed runs of
    /// `self.x.extend(other.x)` in `rebuild_layout`, which silently dropped any
    /// vec nobody remembered to add a line for — `grooves` was invisible for
    /// exactly that reason. The destructuring below turns a new field into a
    /// compile error instead of a missing mark on screen.
    pub fn absorb(&mut self, other: PageContent) {
        let PageContent { rects, texts, buttons, reliefs, grooves, images, icons, strokes, plates } = other;
        self.rects.extend(rects);
        self.texts.extend(texts);
        self.buttons.extend(buttons);
        self.reliefs.extend(reliefs);
        self.grooves.extend(grooves);
        self.images.extend(images);
        self.icons.extend(icons);
        self.strokes.extend(strokes);
        self.plates.extend(plates);
    }

    /// This content cut to `[l, t, r, b]`: fills intersected with it (a
    /// fill wholly outside is dropped), and the bounds of every label and
    /// glyph narrowed to it. For a widget that paints past the rect it was
    /// given — the graph, panned — since a flat host has no clip stack.
    /// Strokes are cut by their clip as glyphs are. Parts with no clip of
    /// their own (buttons, reliefs, grooves, images,
    /// plates) pass through as they are.
    pub fn clipped_to(mut self, clip: [f32; 4]) -> Self {
        let [l, t, r, b] = clip;
        self.rects.retain_mut(|(_, x, y, w, h, _, _)| {
            let (x0, y0) = (x.max(l), y.max(t));
            let (x1, y1) = ((*x + *w).min(r), (*y + *h).min(b));
            if x1 <= x0 || y1 <= y0 {
                return false;
            }
            (*x, *y, *w, *h) = (x0, y0, x1 - x0, y1 - y0);
            true
        });
        let narrow = |bounds: Option<[f32; 4]>| -> Option<[f32; 4]> {
            Some(match bounds {
                Some([bl, bt, br, bb]) => [bl.max(l), bt.max(t), br.min(r), bb.min(b)],
                None => clip,
            })
        };
        for t in &mut self.texts {
            t.6 = narrow(t.6);
        }
        for i in &mut self.icons {
            i.6 = narrow(i.6);
        }
        for st in &mut self.strokes {
            st.2 = narrow(st.2);
        }
        self
    }

    /// A GPU-textured quad (id from `cce_ui::vk::upload_rgba`).
    pub fn image(&mut self, id: u32, x: f32, y: f32, w: f32, h: f32, alpha: f32) {
        self.images.push((id, x, y, w, h, alpha));
    }

    /// A cce-icons glyph (`folder`, `file-image`, …) at `(x, y, w, h)`, tinted
    /// `color` as a label is — the one way this crate draws a symbol.
    pub fn icon(&mut self, name: &str, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) {
        self.icons.push((name.to_string(), x, y, w, h, color, None));
    }

    /// [`PageContent::icon`] cut to `bounds` `[l, t, r, b]`.
    pub fn icon_bounded(&mut self, name: &str, x: f32, y: f32, w: f32, h: f32, color: [f32; 4], bounds: Option<[f32; 4]>) {
        self.icons.push((name.to_string(), x, y, w, h, color, bounds));
    }

    pub fn rect(&mut self, color: [f32; 4], x: f32, y: f32, w: f32, h: f32) {
        self.rects.push((color, x, y, w, h, 0.0, (true, true, true, true)));
    }

    /// [`PageContent::rect`] with a corner radius, applied only to `corners`
    /// (top-left, top-right, bottom-right, bottom-left) — for a fill that has to
    /// follow the rounded corner of the plate it sits inside.
    pub fn rect_rounded(
        &mut self,
        color: [f32; 4],
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        corners: (bool, bool, bool, bool),
    ) {
        self.rects.push((color, x, y, w, h, radius, corners));
    }

    /// A lit plate at (x, y, w, h): a rounded face in `color` plus the rolled,
    /// lit perimeter — the treatment the pane plates wear. Unlike the relief
    /// helpers this is NOT gated on `control_relief`: a plate owns the fill, so
    /// skipping it would leave the surface unpainted rather than merely flat.
    pub fn plate(&mut self, color: [f32; 4], x: f32, y: f32, w: f32, h: f32, radius: f32) {
        let depth = cce_ui::layout::bevel_width().min(h * 0.2);
        self.plates.push((color, x, y, w, h, radius, depth));
    }

    /// Wall width for a carve whose corners are rounded at `radius`, capped so
    /// the wall stays crease-free through the corner. A carve's wall straddles
    /// the boundary, reaching `depth / 2` inward — and the inward offsets of a
    /// squircle corner kink into a square crease past the corner's diagonal
    /// curvature radius (`radius / corner_span_factor()`). The plate path
    /// avoids this by widening its corner span instead, which a carve cannot:
    /// its silhouette must stay on the widget's own nominal-radius corner.
    /// Only tall rects ever feel the cap (a control's `h * 0.2` already lands
    /// under it); the list well is the case that motivated it. Zero radius is
    /// exempt — square corners meet in a miter by design.
    fn carve_depth(h: f32, radius: f32) -> f32 {
        let depth = cce_ui::layout::bevel_width().min(h * 0.2);
        if radius > 0.0 {
            depth.min(2.0 * radius / cce_ui::layout::corner_span_factor())
        } else {
            depth
        }
    }

    /// A recessed well carved over the control at (x, y, w, h) — no-op when the
    /// DE's control_relief styling is off.
    pub fn relief_recessed(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32) {
        if cce_ui::layout::control_relief() {
            let depth = Self::carve_depth(h, radius);
            self.reliefs.push((x, y, w, h, radius, depth, RELIEF_RECESSED));
        }
    }

    /// [`Self::relief_recessed`] for the well holding pointer focus: the ring
    /// replaces the lighting (see [`RELIEF_RECESSED_FOCUS`]).
    pub fn relief_recessed_focused(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32) {
        if cce_ui::layout::control_relief() {
            let depth = Self::carve_depth(h, radius);
            self.reliefs.push((x, y, w, h, radius, depth, RELIEF_RECESSED_FOCUS));
        }
    }

    /// A raised plateau over the control at (x, y, w, h) — no-op when the DE's
    /// control_relief styling is off.
    pub fn relief_raised(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32) {
        if cce_ui::layout::control_relief() {
            let depth = Self::carve_depth(h, radius);
            self.reliefs.push((x, y, w, h, radius, depth, RELIEF_RAISED));
        }
    }

    /// A line engraved from `a` to `b` into the surface `host` — no-op when the
    /// DE's control_relief styling is off.
    pub fn groove(&mut self, a: (f32, f32), b: (f32, f32), width: f32, host: (f32, f32, f32, f32)) {
        if cce_ui::layout::control_relief() {
            let depth = cce_ui::layout::bevel_width().min(host.3 * 0.2);
            self.grooves.push((a.0, a.1, b.0, b.1, width, depth, host.0, host.1, host.2, host.3));
        }
    }

    /// A flush inset button plate (groove ring + beveled lip, face level with the
    /// surface — the Button treatment) over the control at (x, y, w, h) — no-op
    /// when the DE's control_relief styling is off.
    pub fn relief_inset(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32) {
        if cce_ui::layout::control_relief() {
            let depth = Self::carve_depth(h, radius);
            self.reliefs.push((x, y, w, h, radius, depth, RELIEF_INSET));
        }
    }

    pub fn text(&mut self, content: &str, x: f32, y: f32, size: f32, color: [f32; 4]) {
        self.texts.push((content.to_string(), size, x, y, color, None, None));
    }

    pub fn text_with_font(&mut self, content: &str, x: f32, y: f32, size: f32, color: [f32; 4], font: &str) {
        self.texts.push((content.to_string(), size, x, y, color, Some(font.to_string()), None));
    }

    /// `text_with_font` with an explicit clip box `[l, t, r, b]` — for text
    /// that scrolls under an edge and must render cut, not culled.
    pub fn text_with_font_bounded(&mut self, content: &str, x: f32, y: f32, size: f32, color: [f32; 4], font: &str, bounds: [f32; 4]) {
        self.texts.push((content.to_string(), size, x, y, color, Some(font.to_string()), Some(bounds)));
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
        self.buttons.push((Owned::new(btn), action));
    }

    /// A button wearing the toolkit's own face — no per-call colors. The
    /// hand-tinted variant above predates the themed Button; chrome buttons
    /// (the chooser's Cancel/Save) should look like every other DE button.
    pub fn button_plain(&mut self, label: &str, x: f32, y: f32, w: f32, h: f32, action: crate::Message) {
        // Colorless chrome: transparent face over the theme's border/relief —
        // the closed-dropdown convention — with a faint neutral hover. The
        // toolkit's Primary face is itself blue-tinted, which is exactly what
        // these buttons are not supposed to be.
        let btn = cce_ui::widget::Button::new(x, y, w, h)
            .with_label(label)
            .with_bg([0.0, 0.0, 0.0, 0.0])
            .with_hover_bg([1.0, 1.0, 1.0, 0.10]);
        self.buttons.push((Owned::new(btn), action));
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
        self.buttons.push((Owned::new(btn), action));
    }
}

impl RenderTarget for PageContent {
    fn icon(&mut self, name: &str, rect: cce_ui::scene::layout::Rect, color: [f32; 4]) {
        PageContent::icon(self, name, rect.x, rect.y, rect.width, rect.height, color);
    }

    fn line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, thickness: f32, color: [f32; 4], cap: cce_ui::scene::paint::Cap) {
        self.strokes.push((Stroke::Line { x1, y1, x2, y2, thickness, cap }, color, None));
    }

    fn arc(&mut self, cx: f32, cy: f32, radius: f32, thickness: f32, start: f32, end: f32, color: [f32; 4]) {
        self.strokes.push((Stroke::Arc { cx, cy, radius, thickness, start, end }, color, None));
    }

    fn circle(&mut self, cx: f32, cy: f32, radius: f32, color: [f32; 4]) {
        self.strokes.push((Stroke::Disc { cx, cy, radius }, color, None));
    }

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

    /// The flat-path bridge for a widget's flush inset plate — the Dropdown's
    /// OPEN popover, which is its trigger surface grown over the unified box.
    /// Without this override the default degrades it to a plain rounded fill,
    /// so the menu lost the groove ring and lip the closed trigger has the
    /// moment it expanded.
    ///
    /// The face and the walls go to different vecs on purpose — `reliefs` are
    /// edges-only, drawn over the faces `rects` own — and `display_list` emits
    /// this part's rects before its reliefs, so one call here lands as fill
    /// then ring, in that order, within whichever part is being collected.
    ///
    /// **The caller's `depth` is used verbatim, NOT re-derived from `h`.** The
    /// widget computes it from the TRIGGER's height; the box handed here is the
    /// trigger plus the revealed menu, several times taller. `relief_inset`
    /// would recompute `bevel_width().min(h * 0.2)` off that expanded height
    /// and thicken the ring as the menu grows. A ring that swells during the open
    /// animation is exactly the artifact this override exists to avoid.
    fn inset_plate(&mut self, color: [f32; 4], x: f32, y: f32, w: f32, h: f32, radius: f32, depth: f32) {
        self.rects.push((color, x, y, w, h, radius, (true, true, true, true)));
        if cce_ui::layout::control_relief() {
            self.reliefs.push((x, y, w, h, radius, depth, RELIEF_INSET));
        }
    }
    /// The focused control plate's ring — same plate, rim lit.
    fn inset_plate_tinted(&mut self, color: [f32; 4], x: f32, y: f32, w: f32, h: f32, radius: f32, depth: f32, _tint: [f32; 3]) {
        self.rects.push((color, x, y, w, h, radius, (true, true, true, true)));
        if cce_ui::layout::control_relief() {
            self.reliefs.push((x, y, w, h, radius, depth, RELIEF_INSET_FOCUS));
        }
    }
}
