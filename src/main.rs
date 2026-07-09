use wayland_client::QueueHandle;
use glyphon::FontSystem;

use cce_ui::engine::{Application, LogicalPosition, LogicalSize, WindowSettings};
use cce_ui::widget::{MouseButton, ElementState, MouseScrollDelta, KeyEvent, TextItem, Element, PageSelector, Paginator, MenuController};
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
const MENU_CHAR_W: f32 = 7.5;   // approximate glyph advance used for menu sizing

/// Context-menu width/height for a given set of options.
fn context_menu_size(options: &[(String, Option<Message>)]) -> (f32, f32) {
    let max_len = options.iter().map(|(s, _)| s.len()).max().unwrap_or(0);
    let w = ((max_len as f32 * MENU_CHAR_W) + 24.0).max(MENU_MIN_W);
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
        for (_, ox, oy, ow, oh, _, _) in &overlay_pc.rects {
            let o_min_x = *ox;
            let o_max_x = *ox + *ow;
            let o_min_y = *oy;
            let o_max_y = *oy + *oh;

            if t_max_x > o_min_x && t_min_x < o_max_x && t_max_y > o_min_y && t_min_y < o_max_y {
                if t_min_x >= o_min_x && t_max_x <= o_max_x && t_min_y >= o_min_y && t_max_y <= o_max_y {
                    return None;
                }
                if o_min_x > t_min_x && o_min_x < t_max_x {
                    bounds[2] = bounds[2].min(o_min_x);
                }
                if o_max_x > t_min_x && o_max_x < t_max_x {
                    bounds[0] = bounds[0].max(o_max_x);
                }
                if o_min_y > t_min_y && o_min_y < t_max_y {
                    bounds[3] = bounds[3].min(o_min_y);
                }
                if o_max_y > t_min_y && o_max_y < t_max_y {
                    bounds[1] = bounds[1].max(o_max_y);
                }
            }
        }
    }
    Some(bounds)
}

// ── State ───────────────────────────────────────────────────────────

struct AppWidget {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: [f32; 4],
    radius: f32,
    corners: (bool, bool, bool, bool),
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

struct BrowseContainer {
    pub base: cce_ui::widget::Widget,
    pub parent: Option<*mut (dyn cce_ui::widget::Element + 'static)>,
    pub breadcrumb: *mut cce_ui::widget::Adapted<cce_ui::widget::Breadcrumb>,
    pub list_box: *mut cce_ui::widget::List,
    pub save_name_box: *mut cce_ui::widget::Adapted<cce_ui::widget::TextBox>,
    pub select_mode: bool,
}

impl cce_ui::widget::Element for BrowseContainer {
    cce_ui::impl_widget_base!(BrowseContainer);

    fn color(&self) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn children(&self, _ctx: &cce_ui::widget::UiContext) -> Vec<*mut (dyn cce_ui::widget::Element + 'static)> {
        let mut list = vec![self.breadcrumb as *mut (dyn cce_ui::widget::Element + 'static), self.list_box as *mut (dyn cce_ui::widget::Element + 'static)];
        if self.select_mode {
            list.push(self.save_name_box as *mut (dyn cce_ui::widget::Element + 'static));
        }
        list
    }

    fn parent(&self, _ctx: &cce_ui::widget::UiContext) -> Option<*mut (dyn cce_ui::widget::Element + 'static)> {
        self.parent
    }

    fn set_parent(&mut self, parent: Option<*mut (dyn cce_ui::widget::Element + 'static)>, ctx: &mut cce_ui::widget::UiContext) {
        self.parent = parent;
        if parent.is_some() {
            let self_ptr = self as *mut Self;
            let self_id = self.base.id();
            unsafe {
                let bc_id = (*self.breadcrumb).base().unwrap().id();
                ctx.register_widget(bc_id, self.breadcrumb as *mut (dyn cce_ui::widget::Element + 'static));
                ctx.link_ids(self_id, bc_id);
                (*self.breadcrumb).set_parent(Some(self_ptr), ctx);

                let lb_id = (*self.list_box).base().unwrap().id();
                ctx.register_widget(lb_id, self.list_box as *mut (dyn cce_ui::widget::Element + 'static));
                ctx.link_ids(self_id, lb_id);
                (*self.list_box).set_parent(Some(self_ptr), ctx);

                if self.select_mode {
                    let sn_id = (*self.save_name_box).base().unwrap().id();
                    ctx.register_widget(sn_id, self.save_name_box as *mut (dyn cce_ui::widget::Element + 'static));
                    ctx.link_ids(self_id, sn_id);
                    (*self.save_name_box).set_parent(Some(self_ptr), ctx);
                }
            }
        }
    }

    fn set_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.base.x = x;
        self.base.y = y;
        self.base.w = w;
        self.base.h = h;

        let gap = 12.0;
        let breadcrumb_h = cce_ui::layout::button_height();
        let textbox_h = 24.0;

        unsafe {
            // pages/browse::view re-renders the breadcrumb and the view dropdown on top of this
            // container's paint. Position the breadcrumb here to EXACTLY match that layout
            // (inset by the page margin, reserving the dropdown's width beside it) so this
            // container's copy sits fully behind the page copy instead of leaking a dark strip
            // behind the dropdown and margins. Keep these constants in sync with pages/browse.rs.
            let page_margin = 12.0;
            let dropdown_w = 120.0;
            let page_breadcrumb_h = 24.0;
            let inner_x = x + page_margin;
            let inner_y = y + page_margin;
            let inner_w = w - 2.0 * page_margin;
            let bc_w = inner_w - dropdown_w - gap;
            (*self.breadcrumb).set_rect(inner_x, inner_y, bc_w, page_breadcrumb_h);
            let list_h = if self.select_mode {
                h - breadcrumb_h - textbox_h - 2.0 * gap
            } else {
                h - breadcrumb_h - gap
            };
            (*self.list_box).set_rect(x, y + breadcrumb_h + gap, w, list_h);

            if self.select_mode {
                (*self.save_name_box).set_rect(x, y + h - textbox_h, w, textbox_h);
                (*self.save_name_box).set_row_rect(x, w);
            }
        }
    }
}

unsafe impl Send for BrowseContainer {}
unsafe impl Sync for BrowseContainer {}

struct NetworkContainer {
    pub base: cce_ui::widget::Widget,
    pub parent: Option<*mut (dyn cce_ui::widget::Element + 'static)>,
    pub breadcrumb: *mut cce_ui::widget::Adapted<cce_ui::widget::Breadcrumb>,
    pub graph: *mut cce_ui::widget::Adapted<cce_ui::widget::Graph>,
}

impl cce_ui::widget::Element for NetworkContainer {
    cce_ui::impl_widget_base!(NetworkContainer);

    fn color(&self) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn children(&self, _ctx: &cce_ui::widget::UiContext) -> Vec<*mut (dyn cce_ui::widget::Element + 'static)> {
        vec![self.breadcrumb as *mut (dyn cce_ui::widget::Element + 'static), self.graph as *mut (dyn cce_ui::widget::Element + 'static)]
    }

    fn parent(&self, _ctx: &cce_ui::widget::UiContext) -> Option<*mut (dyn cce_ui::widget::Element + 'static)> {
        self.parent
    }

    fn set_parent(&mut self, parent: Option<*mut (dyn cce_ui::widget::Element + 'static)>, ctx: &mut cce_ui::widget::UiContext) {
        self.parent = parent;
        if parent.is_some() {
            let self_ptr = self as *mut Self;
            let self_id = self.base.id();
            unsafe {
                let bc_id = (*self.breadcrumb).base().unwrap().id();
                ctx.register_widget(bc_id, self.breadcrumb as *mut (dyn cce_ui::widget::Element + 'static));
                ctx.link_ids(self_id, bc_id);
                (*self.breadcrumb).set_parent(Some(self_ptr), ctx);

                let g_id = (*self.graph).base().unwrap().id();
                ctx.register_widget(g_id, self.graph as *mut (dyn cce_ui::widget::Element + 'static));
                ctx.link_ids(self_id, g_id);
                (*self.graph).set_parent(Some(self_ptr), ctx);
            }
        }
    }

    fn set_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.base.x = x;
        self.base.y = y;
        self.base.w = w;
        self.base.h = h;

        let gap = 12.0;
        let breadcrumb_h = cce_ui::layout::button_height();

        unsafe {
            (*self.breadcrumb).set_rect(x, y, w, breadcrumb_h);
            (*self.graph).set_rect(x, y + breadcrumb_h + gap, w, h - breadcrumb_h - gap);
        }
    }
}

unsafe impl Send for NetworkContainer {}
unsafe impl Sync for NetworkContainer {}

struct FilesystemApp {
    current_page: Page,
    browse: pages::browse::BrowseState,
    network: pages::network::NetworkState,
    preview: cce_ui::widget::Adapted<pages::preview::PreviewState>,

    // Command-line chooser options
    select_mode: bool,
    select_directory: bool,
    save_mode: bool,

    // Rendering resources
    widgets: Vec<AppWidget>,
    text_items: Vec<TextItem>,
    font_system: FontSystem,
    needs_rebuild: bool,
    width: u32,
    height: u32,
    scale_factor: f64,
    page_buttons: Vec<(cce_ui::widget::Adapted<cce_ui::widget::Button>, Message)>,
    cursor_x: f32,
    cursor_y: f32,
    paginator: cce_ui::widget::Adapted<Paginator>,
    view_dropdown: cce_ui::widget::Adapted<cce_ui::widget::Dropdown>,
    just_initialized: bool,
    ui_context: cce_ui::context::UiContext,
    watcher: Option<notify::RecommendedWatcher>,
    fs_service: services::fs::FsService,
    context_menu: ContextMenu,
    open_with_dialog: Option<(std::path::PathBuf, cce_ui::widget::Adapted<cce_ui::widget::TextBox>)>,
    root_window: cce_ui::widget::Backplate,
    browse_splitter: cce_ui::widget::SplitBox,
    network_splitter: cce_ui::widget::SplitBox,
    browse_container: BrowseContainer,
    network_container: NetworkContainer,
    last_click_time: std::time::Instant,
    last_clicked_idx: Option<usize>,
}


// ── Layout Rebuild ──────────────────────────────────────────────────

impl FilesystemApp {
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

    fn rebuild_layout(&mut self) {
        // Refresh the container child pointers on every rebuild. They point into
        // self.browse / self.network, whose addresses change when the app value is
        // first moved out of new(); re-taking them here (where self is at its final
        // address) keeps them valid regardless of return-value optimization.
        self.browse_container.breadcrumb = &mut self.browse.breadcrumb;
        self.browse_container.list_box = &mut self.browse.list_box;
        self.browse_container.save_name_box = &mut self.browse.save_name_box;
        self.network_container.breadcrumb = &mut self.network.breadcrumb;
        self.network_container.graph = &mut self.network.graph;

        self.ui_context.clear_hierarchy();
        self.browse.save_name_box.prepare_text(&mut self.font_system);
        self.browse.list_box.prepare_text(&mut self.font_system);

        let mut widgets = Vec::new();
        let mut text_items = Vec::new();

        cce_ui::widget::hover_animation::reset_frame_registration();
        cce_ui::widget::hover_animation::set_cursor_pos(self.cursor_x, self.cursor_y);

        // Update root window size, background color, opacity
        self.root_window.set_rect(0.0, 0.0, self.width as f32, self.height as f32);

        // Rebuild Element Focus Hierarchy
        self.root_window.clear_children(&mut self.ui_context);

        // Clear all widgets' hierarchy links
        self.paginator.clear_children(&mut self.ui_context); self.paginator.set_parent(None, &mut self.ui_context);
        self.view_dropdown.clear_children(&mut self.ui_context); self.view_dropdown.set_parent(None, &mut self.ui_context);
        self.preview.clear_children(&mut self.ui_context); self.preview.set_parent(None, &mut self.ui_context);

        self.browse_splitter.set_parent(None, &mut self.ui_context);
        self.network_splitter.set_parent(None, &mut self.ui_context);
        self.browse_container.clear_children(&mut self.ui_context); self.browse_container.set_parent(None, &mut self.ui_context);
        self.network_container.clear_children(&mut self.ui_context); self.network_container.set_parent(None, &mut self.ui_context);

        self.browse.save_name_box.clear_children(&mut self.ui_context); self.browse.save_name_box.set_parent(None, &mut self.ui_context);
        self.browse.list_box.clear_children(&mut self.ui_context); self.browse.list_box.set_parent(None, &mut self.ui_context);
        self.browse.breadcrumb.clear_children(&mut self.ui_context); self.browse.breadcrumb.set_parent(None, &mut self.ui_context);
        self.network.breadcrumb.clear_children(&mut self.ui_context); self.network.breadcrumb.set_parent(None, &mut self.ui_context);
        self.network.graph.clear_children(&mut self.ui_context); self.network.graph.set_parent(None, &mut self.ui_context);
        if let Some((_, textbox)) = &mut self.open_with_dialog {
            textbox.clear_children(&mut self.ui_context);
            textbox.set_parent(None, &mut self.ui_context);
        }

        use cce_ui::widget::focus::link_parent_child;
        let has_sidebar = false;
        let sidebar_w = if has_sidebar { self.paginator.sidebar_w() } else { 0.0 };
        let browse_x = if has_sidebar { sidebar_w + 17.0 } else { 16.0 };
        let usable_w = self.width as f32 - sidebar_w - (if has_sidebar { 1.0 } else { 0.0 }) - 32.0;
        let content_y = 16.0;

        let select_bar_h = 48.0;
        let content_h = if self.select_mode {
            self.height as f32 - 32.0 - select_bar_h
        } else {
            self.height as f32 - 32.0
        };

        if has_sidebar {
            link_parent_child(&mut self.root_window, &mut self.paginator, &mut self.ui_context);
        }
        link_parent_child(&mut self.root_window, &mut self.view_dropdown, &mut self.ui_context);

        match self.current_page {
            Page::Browse => {
                if self.browse_splitter.children.is_empty() {
                    self.browse_splitter.add_child_with_proportion(&mut self.browse_container, 0.49, 100.0);
                    self.browse_splitter.add_child_with_proportion(&mut self.preview, 0.51, 100.0);
                }
                link_parent_child(&mut self.root_window, &mut self.browse_splitter, &mut self.ui_context);
                self.browse_splitter.set_rect(browse_x, content_y, usable_w, content_h);
            }
            Page::Network => {
                if self.network_splitter.children.is_empty() {
                    self.network_splitter.add_child_with_proportion(&mut self.network_container, 0.49, 100.0);
                    self.network_splitter.add_child_with_proportion(&mut self.preview, 0.51, 100.0);
                }
                link_parent_child(&mut self.root_window, &mut self.network_splitter, &mut self.ui_context);
                self.network_splitter.set_rect(browse_x, content_y, usable_w, content_h);
            }
        }

        if let Some((_, textbox)) = &mut self.open_with_dialog {
            link_parent_child(&mut self.root_window, textbox, &mut self.ui_context);
        }

        // Layout widgets recursively inside the parent space
        let mut dummy_pc = pages::PageContent::new();
        if has_sidebar {
            let page_idx = Page::ALL.iter().position(|&p| p == self.current_page).unwrap_or(0);
            self.paginator.set_selected_page(page_idx);
            cce_ui::layout::render_widget(&mut dummy_pc, &mut self.paginator, 0.0, 0.0, sidebar_w, self.height as f32, &mut self.ui_context);
        }

        // Render root window recursively
        let mut window_pc = pages::PageContent::new();
        cce_ui::layout::render_widget(&mut window_pc, &mut self.root_window, 0.0, 0.0, self.width as f32, self.height as f32, &mut self.ui_context);

        // 3. Draw Page custom/static content (drawn to pc)
        let mut pc = pages::PageContent::new();
        match self.current_page {
            Page::Browse => {
                let (bx, by, bw, bh) = self.browse_container.rect();
                let browse_pc = pages::browse::view(&mut self.browse, &mut self.view_dropdown, bx, by, bw, bh, self.select_mode, &mut self.ui_context);

                pc.rects.extend(browse_pc.rects);
                pc.texts.extend(browse_pc.texts);
                pc.buttons.extend(browse_pc.buttons);
            }
            Page::Network => {
                let (nx, ny, nw, nh) = self.network_container.rect();
                let network_pc = pages::network::view(&mut self.network, &self.browse, &mut self.view_dropdown, nx, ny, nw, nh, &mut self.ui_context);

                pc.rects.extend(network_pc.rects);
                pc.texts.extend(network_pc.texts);
                pc.buttons.extend(network_pc.buttons);
            }
        }

        // Draw bottom selection bar if select_mode is enabled
        if self.select_mode {
            let bar_y = self.height as f32 - select_bar_h - 16.0;
            // Divider line
            pc.rect([0.15, 0.20, 0.16, 1.0], browse_x, bar_y, usable_w, 1.0);

            let accent = [0.36, 0.56, 0.38, 1.0];
            let text_fg = [0.83, 0.83, 0.83, 1.0];

            let btn_h = cce_ui::layout::button_height();
            let btn_y = bar_y + (select_bar_h - btn_h) / 2.0;

            // Cancel button
            let cancel_x = self.width as f32 - 180.0;
            pc.button(
                "Cancel",
                cancel_x,
                btn_y,
                70.0,
                btn_h,
                [0.25, 0.12, 0.12, 0.5],
                [0.35, 0.15, 0.15, 0.8],
                text_fg,
                Message::SelectCancel,
            );

            // Open/Select/Save button
            let open_x = self.width as f32 - 100.0;
            let button_label = if self.save_mode { "Save" } else { "Select" };
            pc.button(
                button_label,
                open_x,
                btn_y,
                80.0,
                btn_h,
                accent,
                [0.46, 0.66, 0.48, 1.0],
                [0.10, 0.16, 0.11, 1.0],
                Message::SelectOpen,
            );
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

            // Border
            context_menu_pc.rect([0.22, 0.22, 0.28, 1.0], cx, cy, cw, ch);
            // Background
            context_menu_pc.rect(cce_ui::color::popover_bg_color(), cx + 1.0, cy + 1.0, cw - 2.0, ch - 2.0);
            // Hover highlight
            if let Some(h_idx) = self.context_menu.hovered {
                let iy = cy + h_idx as f32 * ROW_H;
                context_menu_pc.rect([0.20, 0.40, 0.65, 0.6], cx + 2.0, iy + 2.0, cw - 4.0, 20.0);
            }
            // Text options
            for (idx, (opt, _)) in self.context_menu.options.iter().enumerate() {
                let iy = cy + idx as f32 * ROW_H + (ROW_H - 12.0) / 2.0;
                let text_color = if idx == 0 {
                    [0.44, 0.44, 0.47, 1.0]
                } else if self.context_menu.hovered == Some(idx) {
                    [1.0, 1.0, 1.0, 1.0]
                } else {
                    [0.8, 0.8, 0.83, 1.0]
                };
                context_menu_pc.text(opt, cx + 8.0, iy, 12.0, text_color);
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

            let cancel_hover = self.cursor_x >= btn_cancel_x && self.cursor_x <= btn_cancel_x + btn_cancel_w
                && self.cursor_y >= btn_cancel_y && self.cursor_y <= btn_cancel_y + btn_cancel_h;
            let cancel_bg = if cancel_hover { [0.35, 0.15, 0.15, 0.8] } else { [0.25, 0.12, 0.12, 0.5] };
            dialog_pc.button("Cancel", btn_cancel_x, btn_cancel_y, btn_cancel_w, btn_cancel_h, cancel_bg, [0.35, 0.15, 0.15, 0.8], [0.83, 0.83, 0.83, 1.0], Message::OpenWithCancel);

            let open_hover = self.cursor_x >= btn_open_x && self.cursor_x <= btn_open_x + btn_open_w
                && self.cursor_y >= btn_open_y && self.cursor_y <= btn_open_y + btn_open_h;
            let open_bg = if open_hover { [0.46, 0.66, 0.48, 1.0] } else { [0.36, 0.56, 0.38, 1.0] };
            dialog_pc.button("Open", btn_open_x, btn_open_y, btn_open_w, btn_open_h, open_bg, [0.46, 0.66, 0.48, 1.0], [0.1, 0.16, 0.11, 1.0], Message::OpenWithSubmit);
        }

        // Translate everything into widgets and text_items!
        // We collect from: window_pc, pc, popover_pc, context_menu_pc, dialog_pc
        let mut page_buttons = Vec::new();

        for (part_idx, pc_part) in [&window_pc, &pc, &popover_pc, &context_menu_pc, &dialog_pc].into_iter().enumerate() {
            let is_page_content = part_idx == 1;

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
                });
            }
            for (btn, action) in &pc_part.buttons {
                let base = btn.base().unwrap();
                let bg = btn.bg.unwrap_or([0.16, 0.16, 0.24, 1.0]);
                let hover_bg = btn.hover_bg.unwrap_or([0.25, 0.30, 0.26, 1.0]);
                let label = base.label.as_deref().unwrap_or("");
                let label_size = 12.0;
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

                widgets.push(AppWidget {
                    x: wx,
                    y: wy,
                    w: ww,
                    h: wh,
                    color: col,
                    radius: 4.0, // standard button radius
                    corners: (true, true, true, true),
                });

                let text_x = if btn.justify == cce_ui::widget::Justification::Left {
                    base.x + 8.0
                } else {
                    let text_w = label.chars().count() as f32 * label_size * 0.65;
                    base.x + (base.w - text_w) / 2.0
                };
                let text_y = base.y + (base.h - label_size * 1.4) / 2.0;

                let start_bounds = if is_page_content {
                    [0.0, content_y, self.width as f32, content_y + content_h]
                } else {
                    [0.0, 0.0, self.width as f32, self.height as f32]
                };
                let text_w = label.chars().count() as f32 * label_size * 0.65;
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

                text_items.push(TextItem::new(
                    &mut self.font_system,
                    label,
                    label_size,
                    text_x,
                    text_y,
                    glyphon::Color::rgb(
                        (label_color[0] * 255.0) as u8,
                        (label_color[1] * 255.0) as u8,
                        (label_color[2] * 255.0) as u8,
                    ),
                    None,
                    final_button_bounds,
                ));

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

                text_items.push(TextItem::new(
                    &mut self.font_system,
                    text,
                    *size,
                    *x,
                    *y,
                    glyphon::Color::rgb(
                        (col[0] * 255.0) as u8,
                        (col[1] * 255.0) as u8,
                        (col[2] * 255.0) as u8,
                    ),
                    font.as_deref(),
                    final_bounds,
                ));
            }
        }

        self.widgets = widgets;
        self.text_items = text_items;
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

    fn is_movable_backplate_at(&self, px: f32, py: f32) -> bool {
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
            if let Some(base) = btn.base() {
                if px >= base.x && px <= base.x + base.w && py >= base.y && py <= base.y + base.h {
                    return false;
                }
            }
        }
        if self.current_page == Page::Browse {
            if self.browse_splitter.dragging_idx.is_some() || self.browse_splitter.hovered_idx.is_some() {
                return false;
            }
        } else if self.current_page == Page::Network {
            if self.network_splitter.dragging_idx.is_some() || self.network_splitter.hovered_idx.is_some() {
                return false;
            }
        }
        // 5. Fallback to ui_context's check for registered widgets
        self.ui_context.is_movable_backplate_at(px, py)
    }

    fn new(_qh: &QueueHandle<cce_ui::engine::EngineState<Self>>, sender: calloop::channel::Sender<Self::Message>) -> Self {
        // Parse command line arguments
        let args: Vec<String> = std::env::args().collect();
        let select_directory = args.iter().any(|arg| arg == "--select-dir");
        let save_mode = args.iter().any(|arg| arg == "--save");
        let select_mode = save_mode || args.iter().any(|arg| arg == "--select" || arg == "--select-dir");

        cce_ui::scale::set_scale_factor(1.0);

        let browse = pages::browse::BrowseState::default();
        let _current_dir = browse.current_dir.clone();

        let pages_names = Page::ALL.iter().map(|p| p.label().to_string()).collect::<Vec<_>>();
        let paginator = cce_ui::widget::Paginator::new(pages_names);
        let view_dropdown = cce_ui::widget::Dropdown::new(
            vec!["List".to_string(), "Graph".to_string()],
            0,
        ).with_font_family(&cce_ui::layout::list_font_parsed().0);

        let fs_service = services::fs::FsService::new(sender.clone());
        let initial_w = if select_mode { 900 } else { 1200 };
        let initial_h = if select_mode { 500 } else { 720 };
        let root_window = cce_ui::widget::Backplate::new(0.0, 0.0, initial_w as f32, initial_h as f32)
            .with_background(cce_ui::color::page_low_color())
            .with_radius(cce_ui::color::backplate_corner_radius());

        let mut app = Self {
            current_page: Page::Browse,
            browse,
            network: pages::network::NetworkState::default(),
            preview: Default::default(),
            select_mode,
            select_directory,
            save_mode,
            widgets: Vec::new(),
            text_items: Vec::new(),
            font_system: cce_ui::create_font_system(),
            needs_rebuild: true,
            width: initial_w,
            height: initial_h,
            scale_factor: 1.0,
            page_buttons: Vec::new(),
            cursor_x: 0.0,
            cursor_y: 0.0,
            paginator,
            view_dropdown,
            just_initialized: true,
            ui_context: cce_ui::context::UiContext::new(),
            watcher: None,
            fs_service,
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
            root_window,
            browse_splitter: cce_ui::widget::SplitBox::new(cce_ui::widget::SplitDirection::Horizontal, 12.0),
            network_splitter: cce_ui::widget::SplitBox::new(cce_ui::widget::SplitDirection::Horizontal, 12.0),
            browse_container: BrowseContainer {
                base: cce_ui::widget::Widget::new(),
                parent: None,
                breadcrumb: std::ptr::null_mut(),
                list_box: std::ptr::null_mut(),
                save_name_box: std::ptr::null_mut(),
                select_mode,
            },
            network_container: NetworkContainer {
                base: cce_ui::widget::Widget::new(),
                parent: None,
                breadcrumb: std::ptr::null_mut(),
                graph: std::ptr::null_mut(),
            },
            last_click_time: std::time::Instant::now(),
            last_clicked_idx: None,
        };

        // Initialize container references
        app.browse_container.breadcrumb = &mut app.browse.breadcrumb;
        app.browse_container.list_box = &mut app.browse.list_box;
        app.browse_container.save_name_box = &mut app.browse.save_name_box;

        app.network_container.breadcrumb = &mut app.network.breadcrumb;
        app.network_container.graph = &mut app.network.graph;

        // Start initial directory loading via FsService
        app.fs_service.send(services::fs::FsRequest::ReadLastDir);

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
                app_id: "clear-filesystem-chooser".to_string(),
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
    }

    fn view(&mut self, _quads: &mut Vec<(f32, f32, f32, f32, [f32; 4])>, size: LogicalSize, scale: f64) {
        if self.needs_rebuild || self.width != size.width as u32 || self.height != size.height as u32 || self.scale_factor != scale {
            self.width = size.width as u32;
            self.height = size.height as u32;
            self.scale_factor = scale;
            cce_ui::scale::set_scale_factor(scale as f32);
            self.rebuild_layout();
        }
    }

    fn view_rounded_quads(&mut self, quads: &mut Vec<(f32, f32, f32, f32, f32, [f32; 4], (bool, bool, bool, bool))>, size: LogicalSize, scale: f64) {
        if self.needs_rebuild || self.width != size.width as u32 || self.height != size.height as u32 || self.scale_factor != scale {
            self.width = size.width as u32;
            self.height = size.height as u32;
            self.scale_factor = scale;
            cce_ui::scale::set_scale_factor(scale as f32);
            self.rebuild_layout();
        }
        for w in &self.widgets {
            quads.push((w.x, w.y, w.w, w.h, w.radius, w.color, w.corners));
        }
    }

    fn display_list(&mut self) -> Option<cce_ui::scene::paint::DisplayList> {
        // Phase 3 single paint path (flat-list bridge). rebuild_layout flattens every source
        // (browse/network page, popovers, context menu, dialogs) into self.widgets, which
        // view_rounded_quads runs above — so build the DisplayList straight from that list.
        // CCE_LEGACY_PAINT falls back.
        if std::env::var("CCE_LEGACY_PAINT").is_ok() {
            return None;
        }
        use cce_ui::scene::layout::Rect;
        let mut pc = cce_ui::scene::paint::PaintCtx::new();
        for w in &self.widgets {
            let rect = Rect { x: w.x, y: w.y, width: w.w, height: w.h };
            if w.radius > 0.1 {
                pc.rounded_rect(rect, w.radius, w.corners, w.color);
            } else {
                pc.quad(rect, w.color);
            }
        }
        Some(pc.finish())
    }

    fn text_items(&self) -> &[TextItem] {
        &self.text_items
    }

    fn clear_color(&self) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn handle_pointer_move(&mut self, pos: LogicalPosition, needs_rebuild: &mut bool) {
        self.cursor_x = pos.x;
        self.cursor_y = pos.y;

        let mut changed = false;

        if self.open_with_dialog.is_some() {
            if let Some((_path, textbox)) = &mut self.open_with_dialog {
                let _ = textbox.cursor_moved(pos.x, pos.y, &mut self.ui_context);
            }
            *needs_rebuild = true;
            self.needs_rebuild = true;
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
            if self.browse_splitter.on_cursor_moved(pos.x, pos.y, &mut self.ui_context) {
                changed = true;
            }
        } else if self.current_page == Page::Network {
            if self.network_splitter.on_cursor_moved(pos.x, pos.y, &mut self.ui_context) {
                changed = true;
            }
        }

        if !self.select_mode && self.paginator.cursor_moved(pos.x, pos.y, &mut self.ui_context) {
            changed = true;
        }

        if self.view_dropdown.cursor_moved(pos.x, pos.y, &mut self.ui_context) {
            changed = true;
        }

        if self.current_page == Page::Browse {
            if self.select_mode {
                if self.browse.save_name_box.cursor_moved(pos.x, pos.y, &mut self.ui_context) {
                    changed = true;
                }
            }
            if self.browse.list_box.cursor_moved(pos.x, pos.y, &mut self.ui_context) {
                changed = true;
            }
            if self.browse.breadcrumb.on_cursor_moved(pos.x, pos.y, &mut self.ui_context) {
                changed = true;
            }
        } else if self.current_page == Page::Network {
            if self.network.breadcrumb.on_cursor_moved(pos.x, pos.y, &mut self.ui_context) {
                changed = true;
            }
            if self.network.graph.is_dragging() {
                if self.network.graph.drag_update(pos.x, pos.y) {
                    changed = true;
                }
            } else {
                if self.network.graph.on_cursor_moved(pos.x, pos.y, &mut self.ui_context) {
                    changed = true;
                }
            }
        }

        // Always check if buttons hover state changed
        for (btn, _action) in &self.page_buttons {
            let base = btn.base().unwrap();
            let _hovering = self.cursor_x >= base.x && self.cursor_x <= base.x + base.w
                && self.cursor_y >= base.y && self.cursor_y <= base.y + base.h;
            // Trigger redraw on pointer moves so hover transitions are smooth
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
                        if textbox.mouse_input(button, state, pos.x, pos.y, &mut self.ui_context) {
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
                    if textbox.mouse_input(button, state, pos.x, pos.y, &mut self.ui_context) {
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
            let is_browse = self.current_page == Page::Browse;
            let breadcrumb = if is_browse { &mut self.browse.breadcrumb } else { &mut self.network.breadcrumb };
            if breadcrumb.hit_test(pos.x, pos.y, &self.ui_context) {
                if breadcrumb.mouse_input(button, state, pos.x, pos.y, &mut self.ui_context) {
                    let idx = breadcrumb.right_clicked_seg.unwrap_or(breadcrumb.path.len());
                    let path_str = breadcrumb.path_to_seg(idx);
                    
                    let header = format!("[Breadcrumb]: {}", path_str);
                    let options = vec![
                        (header, None),
                        ("Copy Path".to_string(), Some(Message::CopyPath(path_str))),
                    ];

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

            // Check if right-clicked on a list button
            let mut right_clicked_action = None;
            for (btn, action) in &self.page_buttons {
                let base = btn.base().unwrap();
                if pos.x >= base.x && pos.x <= base.x + base.w && pos.y >= base.y && pos.y <= base.y + base.h {
                    right_clicked_action = Some(action.clone());
                    break;
                }
            }

            if let Some(Message::Browse(browse_action)) = right_clicked_action {
                let entry_idx = match browse_action {
                    pages::browse::BrowseMessage::SelectEntry(idx) => Some(idx),
                    pages::browse::BrowseMessage::NavigateTo(idx) => Some(idx),
                    _ => None,
                };

                if let Some(idx) = entry_idx {
                    if let Some(entry) = self.browse.entries.get(idx) {
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

                        options.push(("Delete".to_string(), Some(Message::Browse(pages::browse::BrowseMessage::DeleteEntry(idx)))));

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
        let menu_match = !self.select_mode && self.paginator.mouse_input(button, state, pos.x, pos.y, &mut self.ui_context);
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

        if self.view_dropdown.mouse_input(button, state, pos.x, pos.y, &mut self.ui_context) {
            *needs_rebuild = true;
            self.needs_rebuild = true;
            if self.view_dropdown.take_change() {
                let new_page = if self.view_dropdown.selected == 0 { Page::Browse } else { Page::Network };
                return Some(Message::SwitchPage(new_page));
            }
            return None;
        }

        if self.current_page == Page::Browse {
            if self.browse_splitter.mouse_input(button, state, pos.x, pos.y, &mut self.ui_context) {
                *needs_rebuild = true;
                self.needs_rebuild = true;
                return None;
            }
        } else if self.current_page == Page::Network {
            if self.network_splitter.mouse_input(button, state, pos.x, pos.y, &mut self.ui_context) {
                *needs_rebuild = true;
                self.needs_rebuild = true;
                return None;
            }
        }

        if self.current_page == Page::Browse {
            if self.select_mode {
                if self.browse.save_name_box.mouse_input(button, state, pos.x, pos.y, &mut self.ui_context) {
                    if state == ElementState::Pressed {
                        self.ui_context.set_focused(&mut self.browse.save_name_box);
                    }
                    *needs_rebuild = true;
                    self.needs_rebuild = true;
                }
            }
            if self.browse.list_box.mouse_input(button, state, pos.x, pos.y, &mut self.ui_context) {
                *needs_rebuild = true;
                self.needs_rebuild = true;
                if let Some(idx) = self.browse.list_box.take_double_click() {
                    return Some(Message::Browse(pages::browse::BrowseMessage::NavigateTo(idx)));
                }
                if let Some(idx) = self.browse.list_box.take_click() {
                    return Some(Message::Browse(pages::browse::BrowseMessage::SelectEntry(idx)));
                }
            }
            if button == MouseButton::Left && state == ElementState::Pressed {
                if self.browse.breadcrumb.hit_test(pos.x, pos.y, &self.ui_context) {
                    if self.browse.breadcrumb.mouse_input(button, state, pos.x, pos.y, &mut self.ui_context) {
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
                        if self.network.breadcrumb.mouse_input(button, state, pos.x, pos.y, &mut self.ui_context) {
                            if let Some(seg) = self.network.breadcrumb.path_click() {
                                let target_path = pages::browse::path_to_segment(&self.browse.current_dir, seg);
                                self.fs_service.send(services::fs::FsRequest::ReadDirectory(target_path));
                                changed = true;
                            }
                        }
                    } else if self.network.graph.mouse_input(button, state, pos.x, pos.y, &mut self.ui_context) {
                        if self.network.graph.is_dragging() {
                            self.network.graph.drag_begin(pos.x, pos.y);
                        }
                        changed = true;
                    }
                } else if state == ElementState::Released {
                    if self.network.graph.is_dragging() {
                        self.network.graph.drag_end();
                        changed = true;
                    } else {
                        if self.network.graph.mouse_input(button, state, pos.x, pos.y, &mut self.ui_context) {
                            changed = true;
                        }
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
        }

        if state == ElementState::Pressed {
            let clicked_search = self.current_page == Page::Browse && self.browse.list_box.search_enabled && self.browse.list_box.search_visible && self.browse.list_box.search_box.hit_test(pos.x, pos.y, &self.ui_context);
            let clicked_save_name = self.select_mode && self.current_page == Page::Browse && self.browse.save_name_box.hit_test(pos.x, pos.y, &self.ui_context);
            if !clicked_search && self.browse.list_box.search_enabled {
                self.browse.list_box.search_box.unfocus();
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
                let base = btn.base().unwrap();
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
        if self.current_page == Page::Browse || self.current_page == Page::Network {
            // Hit-test against the preview widget's actual laid-out rect. Recomputing a
            // hardcoded 50/50 split here was wrong once the list/preview splitter had been
            // dragged off-center, so wheel events over the preview were misrouted.
            let (prev_x, prev_y, prev_w, prev_h) = self.preview.rect();
            let content_h = prev_h;

            // Inner content-preview region (below the metadata header).
            let px = prev_x + 12.0;
            let py = prev_y + 32.0;
            let pw = prev_w - 24.0;
            let ph = prev_h * 0.5 - 40.0;

            if pos.x as f32 >= px && pos.x as f32 <= px + pw && pos.y as f32 >= py && pos.y as f32 <= py + ph {
                if self.preview.handle_mouse_wheel(delta, content_h) {
                    *needs_rebuild = true;
                    self.needs_rebuild = true;
                }
                return;
            }
        }

        if self.current_page == Page::Browse {
            if self.browse.list_box.mouse_wheel(delta, pos.x, pos.y, &mut self.ui_context) {
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
        } else if self.current_page == Page::Network {
            if self.network.graph.mouse_wheel(delta, pos.x as f32, pos.y as f32, &mut self.ui_context) {
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
        }
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
            if textbox.keyboard_input(event, &mut self.ui_context) {
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
            return None;
        }

        if self.current_page == Page::Network {
            if self.network.graph.keyboard_input(event, &mut self.ui_context) {
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

        // If the search textbox is focused, forward key inputs to it
        if self.current_page == Page::Browse {
            let is_open_search = !self.browse.list_box.search_visible && {
                let open_key = cce_ui::color::list_open_search_key();
                event.state == ElementState::Pressed && cce_ui::widget::match_key_shortcut(event, &open_key)
            };
            
            if self.browse.list_box.search_visible || is_open_search {
                if self.browse.list_box.keyboard_input(event, &mut self.ui_context) {
                    *needs_rebuild = true;
                    self.needs_rebuild = true;
                    if self.browse.list_box.search_box.take_change() {
                        return Some(Message::Browse(pages::browse::BrowseMessage::SearchChanged(
                            self.browse.list_box.search_box.text.clone()
                        )));
                    }
                    return None;
                }
            }
        }

        // If the save_name_box is focused, forward key inputs to it
        if self.select_mode && self.browse.save_name_box.editing {
            if self.browse.save_name_box.keyboard_input(event, &mut self.ui_context) {
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

        // Global key navigation
        if self.current_page == Page::Browse {
            let key_char = match &event.logical_key {
                cce_ui::widget::Key::Character(c) => Some(c.as_str()),
                _ => None,
            };

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
                cce_ui::widget::Key::Named(cce_ui::widget::NamedKey::Enter) => {
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
                }
                cce_ui::widget::Key::Named(cce_ui::widget::NamedKey::Delete) => {
                    if let Some(idx) = self.browse.selected {
                        return Some(Message::Browse(pages::browse::BrowseMessage::DeleteEntry(idx)));
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
                _ => match key_char {
                    Some("j") => {
                        if let Some(index) = pages::browse::next_selection_index(&self.browse, pages::browse::BrowseNavigation::Down) {
                            return Some(Message::Browse(pages::browse::BrowseMessage::SelectEntry(index)));
                        }
                    }
                    Some("k") => {
                        if let Some(index) = pages::browse::next_selection_index(&self.browse, pages::browse::BrowseNavigation::Up) {
                            return Some(Message::Browse(pages::browse::BrowseMessage::SelectEntry(index)));
                        }
                    }
                    Some("l") => {
                        if let Some(idx) = self.browse.selected {
                            if let Some(entry) = self.browse.entries.get(idx) {
                                if entry.is_dir && !is_project_dir(&entry.path) {
                                    return Some(Message::Browse(pages::browse::BrowseMessage::NavigateTo(idx)));
                                }
                            }
                        }
                    }
                    Some("h") => {
                        if let Some(parent) = self.browse.current_dir.parent() {
                            return Some(Message::Browse(pages::browse::BrowseMessage::NavigateToPath(parent.to_path_buf())));
                        }
                    }
                    Some(".") => {
                        return Some(Message::Browse(pages::browse::BrowseMessage::ToggleHidden));
                    }
                    _ => {}
                }
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
