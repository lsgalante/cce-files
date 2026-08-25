//! The detached preview window — cce-files run as `--preview-window <sync-file>`
//! (cce-ui RFC 7c: the first non-designer detach).
//!
//! Process model (app policy, mirroring the designer's): the parent writes the
//! sync file BEFORE spawning this process and rewrites it on every selection
//! change; this window polls it. Reattach is EXIT — the parent `try_wait`s its
//! child from tick and takes the pane back when it goes, so closing this
//! window by any means (corner menu, compositor close, crash) reattaches. The
//! same rule runs in reverse: this process exits when the sync file disappears
//! (the parent reattached or quit) or the parent pid is gone (it crashed, or a
//! session restore respawned us without one) — a preview with no parent is an
//! orphan, not a window worth keeping.
//!
//! Sync-file format, one field per line: parent pid, then the selected path
//! (absent while nothing is selected).

use crate::preview_pane::PreviewPane;
use cce_ui::engine::{Application, LogicalPosition, LogicalSize, WindowSettings};
use cce_ui::widget::{ElementState, KeyEvent, MouseButton, MouseScrollDelta};
use wayland_client::QueueHandle;
use cce_ui::widget::plate_dock as dock;

/// Poll cadence for the sync file. Selection changes are user-paced; 100ms is
/// invisible against the parent's own preview-load latency.
const POLL_S: f32 = 0.1;

/// One menu row's height — matches the toolkit context menu's row pitch.
const MENU_ROW_H: f32 = 24.0;
const MENU_W: f32 = 120.0;

pub struct PreviewWindowApp {
    preview: PreviewPane,
    fs_service: crate::services::fs::FsService,
    sync_path: std::path::PathBuf,
    parent_pid: Option<u32>,
    shown_path: Option<std::path::PathBuf>,
    poll_accum: f32,
    width: u32,
    height: u32,
    cursor: (f32, f32),
    /// The Reattach menu (drawn by this app — the toolkit context menu
    /// dispatches through a widget tree this window does not have): the
    /// top-left of the open menu, `None` while closed.
    menu_at: Option<(f32, f32)>,
    needs_rebuild: bool,
}

impl PreviewWindowApp {
    /// The rect the pane fills and the corner control anchors to.
    fn pane_rect(&self) -> (f32, f32, f32, f32) {
        let pad = cce_ui::layout::root_plate_padding();
        (
            pad,
            pad,
            (self.width as f32 - 2.0 * pad).max(1.0),
            (self.height as f32 - 2.0 * pad).max(1.0),
        )
    }

    fn dock_state() -> dock::PlateDockState {
        dock::PlateDockState { collapsed: false, detached: true }
    }

    /// Re-read the sync file; exit when it (or the parent) is gone.
    fn poll_sync(&mut self, needs_rebuild: &mut bool) {
        let content = match std::fs::read_to_string(&self.sync_path) {
            Ok(c) => c,
            // The parent reattached (it unlinks on reattach and on its own
            // exit path) or never existed (stale session restore).
            Err(_) => std::process::exit(0),
        };
        let mut lines = content.lines();
        if self.parent_pid.is_none() {
            self.parent_pid = lines.next().and_then(|l| l.trim().parse().ok());
        } else {
            lines.next();
        }
        if let Some(pid) = self.parent_pid {
            if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
                std::process::exit(0);
            }
        }
        let path = lines.next().map(str::trim).filter(|l| !l.is_empty());
        let path = path.map(std::path::PathBuf::from);
        if path != self.shown_path {
            self.shown_path = path.clone();
            match path {
                Some(p) => self
                    .fs_service
                    .send(crate::services::fs::FsRequest::ReadPreview(p)),
                None => crate::pages::preview::update(
                    &mut self.preview,
                    crate::pages::preview::PreviewMessage::Clear,
                ),
            }
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }
    }
}

impl Application for PreviewWindowApp {
    type Message = crate::Message;

    fn new(
        _qh: &QueueHandle<cce_ui::engine::EngineState<Self>>,
        sender: calloop::channel::Sender<Self::Message>,
    ) -> Self {
        let args: Vec<String> = std::env::args().collect();
        let sync_path = args
            .iter()
            .position(|a| a == "--preview-window")
            .and_then(|i| args.get(i + 1))
            .map(std::path::PathBuf::from)
            .unwrap_or_default();
        cce_ui::scale::set_scale_factor(1.0);
        cce_ui::scale::set_app_id("cce-files-preview".to_string());
        let mut app = Self {
            preview: PreviewPane::default(),
            fs_service: crate::services::fs::FsService::new(sender),
            sync_path,
            parent_pid: None,
            shown_path: None,
            poll_accum: 0.0,
            width: 460,
            height: 680,
            cursor: (0.0, 0.0),
            menu_at: None,
            needs_rebuild: true,
        };
        // First read now, not a poll tick later: the parent wrote the file
        // before spawning, so the pane has content on its first frame.
        let mut rb = false;
        app.poll_sync(&mut rb);
        app
    }

    fn settings(&self) -> WindowSettings {
        WindowSettings {
            title: "Preview".to_string(),
            // The cce- prefix keys the compositor's decorated-window treatment
            // (silhouette clip, blur-behind, shadow), same as the parent.
            app_id: "cce-files-preview".to_string(),
            width: 460,
            height: 680,
            fullscreen: false,
            min_size: Some((280, 320)),
        }
    }

    fn update(&mut self, msg: Self::Message, needs_rebuild: &mut bool, _exit: &mut bool) {
        if let crate::Message::Preview(m) = msg {
            crate::pages::preview::update(&mut self.preview, m);
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }
    }

    fn tick(&mut self, dt: f32, needs_rebuild: &mut bool) {
        self.poll_accum += dt;
        if self.poll_accum >= POLL_S {
            self.poll_accum = 0.0;
            self.poll_sync(needs_rebuild);
        }
    }

    fn handle_pointer_move(&mut self, pos: LogicalPosition, needs_rebuild: &mut bool) {
        let (px, py) = (pos.x as f32, pos.y as f32);
        // Redraw across the dot's emphasis flips, cheaply: only when hover
        // over the control changes, not on every motion.
        let before = dock::corner_center(self.pane_rect(), false)
            .map(|c| dock::corner_hit(c, self.cursor.0, self.cursor.1));
        let after = dock::corner_center(self.pane_rect(), false)
            .map(|c| dock::corner_hit(c, px, py));
        self.cursor = (px, py);
        if before != after {
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }
    }

    fn handle_mouse_input(
        &mut self,
        button: MouseButton,
        state: ElementState,
        pos: LogicalPosition,
        needs_rebuild: &mut bool,
    ) -> Option<Self::Message> {
        if button != MouseButton::Left || state != ElementState::Released {
            return None;
        }
        let (px, py) = (pos.x as f32, pos.y as f32);
        if let Some((mx, my)) = self.menu_at {
            // One row: Reattach — which for the detached window IS exit; the
            // parent reaps the child and takes the pane back (see module doc).
            if px >= mx && px <= mx + MENU_W && py >= my && py <= my + MENU_ROW_H {
                std::process::exit(0);
            }
            self.menu_at = None;
            *needs_rebuild = true;
            self.needs_rebuild = true;
            return None;
        }
        if let Some(c) = dock::corner_center(self.pane_rect(), false) {
            if dock::corner_hit(c, px, py) {
                self.menu_at = Some((c.0 - dock::CORNER_R, c.1 + dock::CORNER_R));
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
        }
        None
    }

    fn handle_mouse_wheel(
        &mut self,
        delta: &MouseScrollDelta,
        pos: LogicalPosition,
        needs_rebuild: &mut bool,
    ) {
        if let Some(changed) = self.preview.wheel(delta, pos.x as f32, pos.y as f32) {
            if changed {
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
        }
    }

    fn handle_key_input(&mut self, _event: &KeyEvent, _needs_rebuild: &mut bool) -> Option<Self::Message> {
        None
    }

    fn display_list(&mut self, size: LogicalSize, scale: f64) -> Option<cce_ui::scene::paint::DisplayList> {
        use cce_ui::scene::layout::Rect;
        self.width = size.width as u32;
        self.height = size.height as u32;
        cce_ui::scale::set_scale_factor(scale as f32);

        let mut pc = cce_ui::scene::paint::PaintCtx::new();

        // The root plate: a detached pane's plate IS the window's base surface
        // (PlateSpec::detached's role flip) — all four corners on the
        // silhouette, fill mirroring the parent's root plate.
        let mut plate = cce_ui::color::page_low_color();
        if plate[3] > 0.001 {
            plate[3] = cce_ui::color::root_plate_opacity();
        }
        pc.plate_spec(&cce_ui::scene::paint::PlateSpec {
            rect: Rect { x: 0.0, y: 0.0, width: size.width as f32, height: size.height as f32 },
            color: plate,
            blur: false,
            window_corners: (true, true, true, true),
            depth: cce_ui::layout::bevel_width(),
        });

        // The pane, through its own PageContent, bridged onto the PaintCtx via
        // the RenderTarget methods (PaintCtx implements the trait) — the small
        // flat subset of the parent's page bridge, with no viewport clipping
        // because this window IS the pane.
        let (rx, ry, rw, rh) = self.pane_rect();
        self.preview.set_rect(rx, ry, rw, rh);
        let mut content = crate::pages::PageContent::new();
        self.preview.push_prims(&mut content);
        {
            use cce_ui::layout::RenderTarget;
            let crate::pages::PageContent { rects, texts, buttons: _, reliefs, grooves, images } = content;
            for (c, x, y, w, h, r, corners) in rects {
                pc.rect_with_radius_corners(c, x, y, w, h, r, corners);
            }
            for (x, y, w, h, radius, depth, kind) in reliefs {
                let rect = Rect { x, y, width: w, height: h };
                let radii = (radius, radius, radius, radius);
                match kind {
                    crate::pages::RELIEF_RAISED => pc.boss(rect, radii, depth),
                    _ => pc.recess(rect, radii, depth),
                }
            }
            for (ax, ay, bx, by, w, d, hx, hy, hw, hh) in grooves {
                pc.groove((ax, ay), (bx, by), w, d, Rect { x: hx, y: hy, width: hw, height: hh });
            }
            for (id, x, y, w, h, alpha) in images {
                pc.image(id, Rect { x, y, width: w, height: h }, alpha);
            }
            for (text, sz, x, y, col, font, bounds) in texts {
                match font {
                    Some(f) => pc.text_with_font_and_bounds(&text, x, y, sz, col, &f, bounds),
                    None => pc.text_with_bounds(&text, x, y, sz, col, bounds),
                }
            }
        }

        // The corner control, over everything the pane drew.
        if let Some(c) = dock::corner_center((rx, ry, rw, rh), false) {
            let emphasized = dock::corner_hit(c, self.cursor.0, self.cursor.1) || self.menu_at.is_some();
            dock::draw_corner_dot(&mut pc, c, emphasized);
        }

        // The one-row Reattach menu.
        if let Some((mx, my)) = self.menu_at {
            let rows = dock::standard_menu(Self::dock_state(), false);
            pc.quad(Rect { x: mx, y: my, width: MENU_W, height: MENU_ROW_H * rows.len() as f32 },
                cce_ui::color::popover_bg_color());
            for (i, (label, _)) in rows.iter().enumerate() {
                let hy = my + i as f32 * MENU_ROW_H;
                if self.cursor.0 >= mx && self.cursor.0 <= mx + MENU_W
                    && self.cursor.1 >= hy && self.cursor.1 <= hy + MENU_ROW_H
                {
                    pc.quad(Rect { x: mx, y: hy, width: MENU_W, height: MENU_ROW_H },
                        [0.20, 0.40, 0.65, 0.6]);
                }
                pc.text(label, mx + 8.0, hy + 6.0, 12.0, [204, 204, 217]);
            }
        }

        Some(pc.finish())
    }

    fn display_list_text(&self) -> bool {
        true
    }
}
