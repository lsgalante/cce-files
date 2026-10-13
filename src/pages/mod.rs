pub mod browse;
pub mod preview;
pub mod network;
pub mod space;

use cce_ui::scene::layout::Rect;
use cce_ui::scene::paint::PaintCtx;
use cce_ui::widget::Handle;

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

/// Lay out and paint a page's breadcrumb: the pane's full width along its top
/// edge, the same on every page, so switching views leaves it where it was.
/// Returns the top of the content under it, one plate gap below.
///
/// The widget paints itself — its run's flush plate, the slanted seams, the
/// hover tint, the labels. Until 2026-10-12 it went through the flat-host
/// bridge, which dropped its relief, so this carved the run's plate and the
/// seams again app-side (`breadcrumb_relief`); the bridge's own copy of the
/// plate still came through, and the run wore two.
///
/// Network and Space used to place theirs 4px in, 6px down and 16px narrower
/// than Browse, an inset left over from before the pane rect carried it, and
/// started their content 28px and 34px down against Browse's 24 + gap.
pub fn breadcrumb_header(
    pc: &mut PaintCtx,
    breadcrumb: Handle<cce_ui::widget::Adapted<cce_ui::widget::Breadcrumb>>,
    cx: f32,
    cy: f32,
    cw: f32,
    ctx: &mut cce_ui::context::UiContext,
) -> f32 {
    let h = breadcrumb_h();
    widget(pc, breadcrumb, cx, cy, cw, h, ctx);
    cy + h + cce_ui::layout::root_plate_gap()
}

/// Place the widget `h` in the block `(x, y, w, bh)` and paint it where it
/// stands — `compose::place_h`, then the paint walk.
pub fn widget<T: cce_ui::widget::WidgetHost + 'static>(
    pc: &mut PaintCtx,
    h: Handle<T>,
    x: f32,
    y: f32,
    w: f32,
    bh: f32,
    ctx: &mut cce_ui::context::UiContext,
) {
    cce_ui::compose::place_h(h, x, y, w, bh, ctx);
    if let Some(widget) = ctx.get(h) {
        cce_ui::widget::painter::paint_root_into(ctx, widget, pc);
    }
}

/// `(x, y, w, h)` as a [`Rect`].
pub fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect { x, y, width: w, height: h }
}

/// A square-cornered fill.
pub fn fill(pc: &mut PaintCtx, color: [f32; 4], x: f32, y: f32, w: f32, h: f32) {
    pc.quad(rect(x, y, w, h), color);
}

/// A fill rounded at `radius` on `corners` (top-left, top-right,
/// bottom-right, bottom-left) — square under a tenth of a pixel.
pub fn fill_rounded(pc: &mut PaintCtx, color: [f32; 4], x: f32, y: f32, w: f32, h: f32, radius: f32, corners: (bool, bool, bool, bool)) {
    if radius > 0.1 {
        pc.rounded_rect(rect(x, y, w, h), radius, corners, color);
    } else {
        pc.quad(rect(x, y, w, h), color);
    }
}

/// A label at `(x, y)` in `color` — its alpha is not drawn — in `font` (the
/// default face when `None`), cut to `bounds` `[l, t, r, b]` when given.
pub fn text(pc: &mut PaintCtx, content: &str, x: f32, y: f32, size: f32, color: [f32; 4], font: Option<&str>, bounds: Option<[f32; 4]>) {
    let rgb = [(color[0] * 255.0) as u8, (color[1] * 255.0) as u8, (color[2] * 255.0) as u8];
    pc.text_with(content.to_string(), x, y, size, rgb, font.map(str::to_string), bounds);
}

/// A cce-icons glyph (`folder`, `file-image`, …) at `(x, y, w, h)`, tinted
/// `color` as a label is — the one way this crate draws a symbol — and cut,
/// not squashed, by `clip` `[l, t, r, b]`. Nothing when the clip misses it.
pub fn icon(pc: &mut PaintCtx, name: &str, x: f32, y: f32, w: f32, h: f32, color: [f32; 4], clip: Option<[f32; 4]>) {
    let Some([l, t, r, b]) = clip else {
        pc.icon(name, rect(x, y, w, h), color);
        return;
    };
    if r <= l || b <= t || l >= x + w || r <= x || t >= y + h || b <= y {
        return;
    }
    pc.clip(rect(l, t, r - l, b - t), |pc| {
        pc.icon(name, rect(x, y, w, h), color);
    });
}

/// Wall depth for a carve whose corners are rounded at `radius`, capped so
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

/// The wall of a recessed well carved round `(x, y, w, h)` — the list's, the
/// preview's two, the treemap's — emitted after what it holds, so it lands
/// over its floor. With pointer `focused` the accent ring replaces its
/// lighting (the tinted carve: the DE's one focus language, the same
/// treatment as a focused plate's ring). Nothing when the DE's
/// control_relief styling is off.
pub fn well(pc: &mut PaintCtx, x: f32, y: f32, w: f32, h: f32, radius: f32, focused: bool) {
    if !cce_ui::layout::control_relief() {
        return;
    }
    let depth = carve_depth(h, radius);
    let radii = (radius, radius, radius, radius);
    if focused {
        let hc = cce_ui::color::highlight_primary_color();
        pc.recess_tinted(rect(x, y, w, h), radii, depth, [hc[0], hc[1], hc[2]]);
    } else {
        pc.recess(rect(x, y, w, h), radii, depth);
    }
}

/// A raised plateau's rim round `(x, y, w, h)`, edges only — nothing when the
/// DE's control_relief styling is off.
pub fn raised(pc: &mut PaintCtx, x: f32, y: f32, w: f32, h: f32, radius: f32) {
    if cce_ui::layout::control_relief() {
        pc.boss(rect(x, y, w, h), (radius, radius, radius, radius), carve_depth(h, radius));
    }
}
