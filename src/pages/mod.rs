pub mod browse;
pub mod preview;
pub mod network;
pub mod space;

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

    pub fn icon(self) -> &'static str {
        match self {
            Page::Browse => "📁",
            Page::Network => "🌐",
            Page::Space => "▦",
        }
    }
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
/// it does NOT need [`dropdown_relief`]'s treatment, for two reasons that are
/// easy to assume away:
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
/// Nor does the `window_pc`-vs-`pc` split matter here, though `dropdown_relief`
/// warns loudly about it. `display_list` emits ALL of a `PageContent`'s rects
/// before ALL of its reliefs, so the call order within `pc` is irrelevant; only
/// a different PageContent could reorder these. The only quad the breadcrumb
/// puts under the carve is the hover tint, and that is inset to the seam's
/// furthest lean by construction, so it does not overlap the grooves. Moving
/// this carve to `window_pc` would buy nothing.
pub fn breadcrumb_relief(
    pc: &mut PageContent,
    breadcrumb: &cce_ui::widget::Adapted<cce_ui::widget::Breadcrumb>,
    rect: cce_ui::scene::layout::Rect,
) {
    let r = cce_ui::layout::dropdown_corner_radius();
    let Some(run) = breadcrumb.run_box(rect) else { return };
    let (rx, ry, rw, rh) = run;
    // The dropdown's flush inset plate on the segment run (mirroring
    // Breadcrumb::paint's relief branch) — the full-rect recessed well and
    // the raised run inside it are gone with the restyle.
    pc.relief_inset(rx, ry, rw, rh, r.min(rh * 0.5));
    for (a, b) in breadcrumb.seams(rect) {
        pc.groove(a, b, cce_ui::widget::Breadcrumb::SEAM_WIDTH, run);
    }
}

/// Mirror the view dropdown's flush inset plate into a flat-path
/// [`PageContent`]: the groove ring sunk around the control, and the control's
/// own edge rolling back up out of it — face level with the window plate, so
/// the seam is the only thing saying it is a separate part.
///
/// `Dropdown::paint` emits this as one `ctx.inset_plate`; `render_widget` keeps
/// only quads and text, so the carve is app-side — the same story as
/// [`breadcrumb_relief`], which is the well-and-plate this pairs with.
///
/// It reaches the SAME `inset_plate` call the widget makes, via
/// [`PageContent::relief_inset`] → `WidgetFx::Inset`. It used to hand-roll the
/// pair `inset_plate` expands to (`relief_recessed` over an expanded rect, then
/// `relief_raised`) — which got the ring's depth wrong, because
/// `relief_recessed` derives depth from the height it is HANDED, and that was
/// the already-expanded one: a 5.76px wall against a 4.8px lip, so the
/// descending wall over-ran the lip instead of meeting it in the tight V-groove
/// with no flat floor that `inset_plate` documents.
///
/// **Carve it into `window_pc`, AFTER the page's `view()` has run.** Two
/// constraints pin it there, and they pull in opposite directions:
///
/// - *After the pages* — because the pages are what lay the dropdown out. Read
///   `view_dropdown.rect()` before they run and you get the rect they assigned
///   on the PREVIOUS frame, so the ring trails the control by a frame through a
///   resize (and on the very first frame it carves a 0×0 rect).
/// - *Into `window_pc`, not the page's own `pc`* — because these are overlay
///   carves, shaded against whatever is already beneath them. Emitting them
///   page-side puts them after the dropdown's own background quad instead of
///   before it, which visibly thins the lit top rim. Same rect, different
///   material. (Verified by pixel-diffing the two orders; `CCE_PLATE_DEBUG=1`
///   shows both as overlay fallback, so this is compositing order, not
///   plate grouping.)
pub fn dropdown_relief(pc: &mut PageContent, rect: cce_ui::scene::layout::Rect) {
    pc.relief_inset(
        rect.x,
        rect.y,
        rect.width,
        rect.height,
        cce_ui::layout::dropdown_corner_radius(),
    );
}

pub const RELIEF_RECESSED: u8 = 0;
pub const RELIEF_RAISED: u8 = 1;
pub const RELIEF_INSET: u8 = 2;

pub struct PageContent {
    pub rects: Vec<([f32; 4], f32, f32, f32, f32, f32, (bool, bool, bool, bool))>,
    pub texts: Vec<(String, f32, f32, f32, [f32; 4], Option<String>, Option<[f32; 4]>)>,
    pub buttons: Vec<(cce_ui::widget::Adapted<cce_ui::widget::Button>, crate::Message)>,
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
        let PageContent { rects, texts, buttons, reliefs, grooves, images } = other;
        self.rects.extend(rects);
        self.texts.extend(texts);
        self.buttons.extend(buttons);
        self.reliefs.extend(reliefs);
        self.grooves.extend(grooves);
        self.images.extend(images);
    }

    /// A GPU-textured quad (id from `cce_ui::vk::upload_rgba`).
    pub fn image(&mut self, id: u32, x: f32, y: f32, w: f32, h: f32, alpha: f32) {
        self.images.push((id, x, y, w, h, alpha));
    }

    pub fn rect(&mut self, color: [f32; 4], x: f32, y: f32, w: f32, h: f32) {
        self.rects.push((color, x, y, w, h, 0.0, (true, true, true, true)));
    }

    /// A recessed well carved over the control at (x, y, w, h) — no-op when the
    /// DE's control_relief styling is off.
    pub fn relief_recessed(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32) {
        if cce_ui::layout::control_relief() {
            let depth = cce_ui::layout::bevel_width().min(h * 0.2);
            self.reliefs.push((x, y, w, h, radius, depth, RELIEF_RECESSED));
        }
    }

    /// A raised plateau over the control at (x, y, w, h) — no-op when the DE's
    /// control_relief styling is off.
    pub fn relief_raised(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32) {
        if cce_ui::layout::control_relief() {
            let depth = cce_ui::layout::bevel_width().min(h * 0.2);
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
            let depth = cce_ui::layout::bevel_width().min(h * 0.2);
            self.reliefs.push((x, y, w, h, radius, depth, RELIEF_INSET));
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

    /// The flat-path bridge for a widget's flush inset plate — the Dropdown's
    /// OPEN popover, which is its trigger surface grown over the unified box.
    /// Without this override the default degrades it to a plain rounded fill,
    /// so the menu lost the groove ring and lip the closed trigger has (the
    /// `dropdown_relief` carve) the moment it expanded.
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
    /// and thicken the ring as the menu grows — the same mistake
    /// [`dropdown_relief`] documents. A ring that swells during the open
    /// animation is exactly the artifact this override exists to avoid.
    fn inset_plate(&mut self, color: [f32; 4], x: f32, y: f32, w: f32, h: f32, radius: f32, depth: f32) {
        self.rects.push((color, x, y, w, h, radius, (true, true, true, true)));
        if cce_ui::layout::control_relief() {
            self.reliefs.push((x, y, w, h, radius, depth, RELIEF_INSET));
        }
    }
}
