use wayland_client::QueueHandle;
use glyphon::FontSystem;

use cce_ui::engine::{Application, LogicalPosition, LogicalSize, WindowSettings};
use cce_ui::widget::{MouseButton, ElementState, MouseScrollDelta, KeyEvent, TextItem, Element, PageSelector, Paginator, MenuController};
use cce_ui::widget::{GraphController, PathController};

use notify::{Watcher, RecommendedWatcher, RecursiveMode, Config};

use cce_filesystem_interface::{Message, pages, services};
use cce_filesystem_interface::pages::Page;
use cce_filesystem_interface::pages::browse::is_project_dir;

// ── State ───────────────────────────────────────────────────────────

#[allow(dead_code)]
struct AppWidget {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: [f32; 4],
    hover_color: [f32; 4],
    hovering: bool,
    action: Option<Message>,
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

#[allow(dead_code)]
struct FilesystemApp {
    current_page: Page,
    browse: pages::browse::BrowseState,
    network: pages::network::NetworkState,
    settings: pages::settings::SettingsState,
    preview: pages::preview::PreviewState,

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
    sender: calloop::channel::Sender<Message>,
    page_buttons: Vec<(cce_ui::widget::Button, Message)>,
    cursor_x: f32,
    cursor_y: f32,
    paginator: Paginator,
    just_initialized: bool,
    ui_context: cce_ui::context::UiContext,
    watcher: Option<notify::RecommendedWatcher>,
    fs_service: services::fs::FsService,
    context_menu: ContextMenu,
    open_with_dialog: Option<(std::path::PathBuf, cce_ui::widget::TextBox)>,
    root_window: cce_ui::widget::Window,
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
        self.browse.save_name_box.prepare_text(&mut self.font_system);
        self.browse.search_box.prepare_text(&mut self.font_system);

        let mut widgets = Vec::new();
        let mut text_items = Vec::new();

        cce_ui::widget::hover_animation::reset_frame_registration();
        cce_ui::widget::hover_animation::set_cursor_pos(self.cursor_x, self.cursor_y);

        // Update root window size, background color, opacity, corner radius
        self.root_window.set_rect(0.0, 0.0, self.width as f32, self.height as f32);
        let bg_color = cce_ui::color::page_low_color();
        self.root_window.background_color = Some(bg_color);
        self.root_window.radius = cce_ui::color::window_corner_radius();

        // Rebuild Element Focus Hierarchy
        self.root_window.clear_children(&mut self.ui_context);

        // Clear all widgets' hierarchy links
        self.paginator.clear_children(&mut self.ui_context); self.paginator.set_parent(None, &mut self.ui_context);
        self.preview.clear_children(&mut self.ui_context); self.preview.set_parent(None, &mut self.ui_context);
        self.browse.search_box.clear_children(&mut self.ui_context); self.browse.search_box.set_parent(None, &mut self.ui_context);
        self.browse.save_name_box.clear_children(&mut self.ui_context); self.browse.save_name_box.set_parent(None, &mut self.ui_context);
        self.browse.list_box.clear_children(&mut self.ui_context); self.browse.list_box.set_parent(None, &mut self.ui_context);
        self.browse.breadcrumb.clear_children(&mut self.ui_context); self.browse.breadcrumb.set_parent(None, &mut self.ui_context);
        self.network.breadcrumb.clear_children(&mut self.ui_context); self.network.breadcrumb.set_parent(None, &mut self.ui_context);
        self.network.graph.clear_children(&mut self.ui_context); self.network.graph.set_parent(None, &mut self.ui_context);
        self.settings.color_selector.clear_children(&mut self.ui_context); self.settings.color_selector.set_parent(None, &mut self.ui_context);
        if let Some((_, textbox)) = &mut self.open_with_dialog {
            textbox.clear_children(&mut self.ui_context);
            textbox.set_parent(None, &mut self.ui_context);
        }

        use cce_ui::widget::focus::link_parent_child;
        let has_sidebar = !self.select_mode;
        let sidebar_w = if has_sidebar { self.paginator.sidebar_w() } else { 0.0 };
        let browse_x = if has_sidebar { sidebar_w + 17.0 } else { 16.0 };
        let usable_w = self.width as f32 - sidebar_w - (if has_sidebar { 1.0 } else { 0.0 }) - 32.0;
        let browse_w = (usable_w - 12.0) * 0.5;
        let preview_w = (usable_w - 12.0) * 0.5;
        let preview_x = browse_x + browse_w + 12.0;
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
        link_parent_child(&mut self.root_window, &mut self.preview, &mut self.ui_context);

        match self.current_page {
            Page::Browse => {
                link_parent_child(&mut self.root_window, &mut self.browse.breadcrumb, &mut self.ui_context);
                link_parent_child(&mut self.root_window, &mut self.browse.list_box, &mut self.ui_context);
                if self.select_mode {
                    link_parent_child(&mut self.root_window, &mut self.browse.save_name_box, &mut self.ui_context);
                } else {
                    link_parent_child(&mut self.root_window, &mut self.browse.search_box, &mut self.ui_context);
                }
            }
            Page::Network => {
                link_parent_child(&mut self.root_window, &mut self.network.breadcrumb, &mut self.ui_context);
                link_parent_child(&mut self.root_window, &mut self.network.graph, &mut self.ui_context);
            }
            Page::Settings => {
                link_parent_child(&mut self.root_window, &mut self.settings.color_selector, &mut self.ui_context);
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
        if self.current_page == Page::Browse || self.current_page == Page::Network {
            cce_ui::layout::render_widget(&mut dummy_pc, &mut self.preview, preview_x, content_y, preview_w, content_h, &mut self.ui_context);
        }

        // Render root window recursively
        let mut window_pc = pages::PageContent::new();
        cce_ui::layout::render_widget(&mut window_pc, &mut self.root_window, 0.0, 0.0, self.width as f32, self.height as f32, &mut self.ui_context);

        // 3. Draw Page custom/static content (drawn to pc)
        let mut pc = pages::PageContent::new();
        match self.current_page {
            Page::Browse => {
                // Draw background plates for columns
                let plate_bg = cce_ui::color::scrollinglist_bg_color();
                let border_color = cce_ui::color::color_borders_color();

                // Left column plate (Browse)
                pc.rect(plate_bg, browse_x, content_y, browse_w, content_h);
                pc.rect(border_color, browse_x, content_y, browse_w, 1.0);
                pc.rect(border_color, browse_x, content_y + content_h - 1.0, browse_w, 1.0);
                pc.rect(border_color, browse_x, content_y, 1.0, content_h);
                pc.rect(border_color, browse_x + browse_w - 1.0, content_y, 1.0, content_h);

                // Right column plate (Preview)
                pc.rect(plate_bg, preview_x, content_y, preview_w, content_h);
                pc.rect(border_color, preview_x, content_y, preview_w, 1.0);
                pc.rect(border_color, preview_x, content_y + content_h - 1.0, preview_w, 1.0);
                pc.rect(border_color, preview_x, content_y, 1.0, content_h);
                pc.rect(border_color, preview_x + preview_w - 1.0, content_y, 1.0, content_h);

                let browse_pc = pages::browse::view(&mut self.browse, browse_x, content_y, browse_w, content_h, self.select_mode, &mut self.ui_context);

                pc.rects.extend(browse_pc.rects);
                pc.texts.extend(browse_pc.texts);
                pc.buttons.extend(browse_pc.buttons);
            }
            Page::Network => {
                let network_pc = pages::network::view(&mut self.network, &self.browse, browse_x, content_y, browse_w, content_h, &mut self.ui_context);

                pc.rects.extend(network_pc.rects);
                pc.texts.extend(network_pc.texts);
                pc.buttons.extend(network_pc.buttons);
            }
            Page::Settings => {
                let settings_pc = pages::settings::view(&mut self.settings, browse_x, content_y, usable_w, content_h, &mut self.ui_context);

                pc.rects.extend(settings_pc.rects);
                pc.texts.extend(settings_pc.texts);
                pc.buttons.extend(settings_pc.buttons);

                // Sync color selector changes back to global node color
                let col = self.settings.color_selector.color;
                let r_f = col[0] as f32 / 255.0;
                let g_f = col[1] as f32 / 255.0;
                let b_f = col[2] as f32 / 255.0;
                let linear_col = cce_ui::color::to_linear([r_f, g_f, b_f, 1.0]);
                if cce_ui::color::node_color() != linear_col {
                    cce_ui::color::set_node_color(linear_col);
                }
            }
        }

        // Draw bottom selection bar if select_mode is enabled
        if self.select_mode {
            let bar_y = self.height as f32 - select_bar_h - 16.0;
            // Divider line
            pc.rect([0.15, 0.20, 0.16, 1.0], browse_x, bar_y, usable_w, 1.0);

            let accent = [0.36, 0.56, 0.38, 1.0];
            let text_fg = [0.83, 0.83, 0.83, 1.0];

            // Cancel button
            let cancel_x = self.width as f32 - 180.0;
            pc.button(
                "Cancel",
                cancel_x,
                bar_y + 10.0,
                70.0,
                28.0,
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
                bar_y + 10.0,
                80.0,
                28.0,
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
                let iy = cy + h_idx as f32 * 24.0;
                context_menu_pc.rect([0.20, 0.40, 0.65, 0.6], cx + 2.0, iy + 2.0, cw - 4.0, 20.0);
            }
            // Text options
            for (idx, (opt, _)) in self.context_menu.options.iter().enumerate() {
                let iy = cy + idx as f32 * 24.0 + (24.0 - 12.0) / 2.0;
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
        if let Some((_path, _textbox)) = &mut self.open_with_dialog {
            let dialog_w = 400.0;
            let dialog_h = 160.0;
            let dialog_x = (self.width as f32 - dialog_w) / 2.0;
            let dialog_y = (self.height as f32 - dialog_h) / 2.0;

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

            // Textbox quads & labels are rendered via textbox.all_quads inside the layout, so we'll grab them from the textbox child widget since textbox is linked to root_window.

            // Render Buttons: Cancel & Open
            let btn_cancel_x = dialog_x + dialog_w - 180.0;
            let btn_cancel_y = dialog_y + dialog_h - 44.0;
            let btn_cancel_w = 70.0;
            let btn_cancel_h = 28.0;

            let btn_open_x = dialog_x + dialog_w - 100.0;
            let btn_open_y = dialog_y + dialog_h - 44.0;
            let btn_open_w = 80.0;
            let btn_open_h = 28.0;

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

        for pc_part in &[window_pc, pc, popover_pc, context_menu_pc, dialog_pc] {
            for (c, x, y, w, h, r, corners) in &pc_part.rects {
                widgets.push(AppWidget {
                    x: *x,
                    y: *y,
                    w: *w,
                    h: *h,
                    color: *c,
                    hover_color: *c,
                    hovering: false,
                    radius: *r,
                    corners: *corners,
                    action: None,
                });
            }
            for (btn, action) in &pc_part.buttons {
                let base = btn.base().unwrap();
                let bg = btn.bg.unwrap_or([0.16, 0.16, 0.24, 1.0]);
                let hover_bg = btn.hover_bg.unwrap_or([0.25, 0.30, 0.26, 1.0]);
                let label = base.label.as_deref().unwrap_or("");
                let label_size = 12.0;
                let label_color = btn.label_color.unwrap_or([0.83, 0.83, 0.83, 1.0]);

                let hovering = self.cursor_x >= base.x && self.cursor_x <= base.x + base.w
                    && self.cursor_y >= base.y && self.cursor_y <= base.y + base.h;
                let col = if hovering { hover_bg } else { bg };

                widgets.push(AppWidget {
                    x: base.x,
                    y: base.y,
                    w: base.w,
                    h: base.h,
                    color: col,
                    hover_color: hover_bg,
                    hovering,
                    radius: 4.0, // standard button radius
                    corners: (true, true, true, true),
                    action: Some(action.clone()),
                });

                let text_x = if btn.left_align {
                    base.x + 8.0
                } else {
                    let text_w = label.chars().count() as f32 * label_size * 0.65;
                    base.x + (base.w - text_w) / 2.0
                };
                let text_y = base.y + (base.h - label_size * 1.4) / 2.0;

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
                    None,
                ));

                page_buttons.push((btn.clone(), action.clone()));
            }
            for (text, size, x, y, col, font, bounds) in &pc_part.texts {
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
                    *bounds,
                ));
            }
        }

        self.widgets = widgets;
        self.text_items = text_items;
        self.page_buttons = page_buttons;
        self.needs_rebuild = false;
    }
}

// ── Application Trait Implementation ────────────────────────────────

impl Application for FilesystemApp {
    type Message = Message;

    fn ui_context(&self) -> Option<&cce_ui::context::UiContext> {
        Some(&self.ui_context)
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

        let fs_service = services::fs::FsService::new(sender.clone());
        let initial_w = if select_mode { 900 } else { 1200 };
        let initial_h = if select_mode { 500 } else { 720 };
        let root_window = cce_ui::widget::Window::new(0.0, 0.0, initial_w as f32, initial_h as f32)
            .with_background(cce_ui::color::page_low_color())
            .with_radius(cce_ui::color::window_corner_radius());

        let mut app = Self {
            current_page: Page::Browse,
            browse,
            network: pages::network::NetworkState::default(),
            settings: pages::settings::SettingsState::default(),
            preview: pages::preview::PreviewState::default(),
            select_mode,
            select_directory,
            save_mode,
            widgets: Vec::new(),
            text_items: Vec::new(),
            font_system: {
                let mut fs = FontSystem::new();
                fs.db_mut().load_fonts_dir("/home/lsgalante/Dropbox/Fonts");
                fs
            },
            needs_rebuild: true,
            width: initial_w,
            height: initial_h,
            scale_factor: 1.0,
            sender: sender.clone(),
            page_buttons: Vec::new(),
            cursor_x: 0.0,
            cursor_y: 0.0,
            paginator,
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
        };

        // Start initial directory loading via FsService
        app.fs_service.send(services::fs::FsRequest::ReadLastDir);

        app.rebuild_layout();
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
                title: "Clear Filesystem Interface".to_string(),
                app_id: "cce-filesystem-interface".to_string(),
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
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
            Message::Browse(msg) => {
                let is_file_double_click = if let pages::browse::BrowseMessage::NavigateTo(idx) = &msg {
                    self.browse.entries.get(*idx).map(|e| !e.is_dir || (self.select_mode && !self.select_directory && is_project_dir(&e.path))).unwrap_or(false)
                } else {
                    false
                };

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
                let selected_path = if let Some(idx) = self.browse.selected {
                    self.browse.entries.get(idx).map(|e| e.path.clone())
                } else {
                    None
                };
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

                if is_file_double_click && self.select_mode {
                    self.update(Message::SelectOpen, needs_rebuild, _exit);
                }
            }
            Message::Preview(msg) => {
                pages::preview::update(&mut self.preview, msg);
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
            Message::KeyboardEvent(_) => {}
            Message::SelectOpen => {
                if self.select_directory {
                    let selected_path = if let Some(idx) = self.browse.selected {
                        self.browse.entries.get(idx).map(|e| e.path.clone())
                    } else {
                        None
                    };
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
                            println!("{}", path.display());
                            std::process::exit(0);
                        }
                    } else {
                        let selected_path = if let Some(idx) = self.browse.selected {
                            self.browse.entries.get(idx).map(|e| e.path.clone())
                        } else {
                            None
                        };
                        if let Some(path) = selected_path {
                            if path.is_dir() && !is_project_dir(&path) {
                                self.fs_service.send(services::fs::FsRequest::ReadDirectory(path));
                            } else {
                                println!("{}", path.display());
                                std::process::exit(0);
                            }
                        }
                    }
                }
            }
            Message::SelectCancel => {
                std::process::exit(1);
            }
            Message::PromptOpenWith(path) => {
                let mut tb = cce_ui::widget::TextBox::new(String::new())
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
                        let parts: Vec<&str> = cmd_str.split_whitespace().collect();
                        if !parts.is_empty() {
                            let program = parts[0];
                            let mut command = std::process::Command::new(program);
                            for arg in &parts[1..] {
                                command.arg(arg);
                            }
                            command.arg(&path);
                            let _ = command.spawn();
                        }
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

        if self.current_page == Page::Settings {
            if self.settings.color_selector.tick(dt, &mut self.ui_context) {
                let col = self.settings.color_selector.color;
                let r_f = col[0] as f32 / 255.0;
                let g_f = col[1] as f32 / 255.0;
                let b_f = col[2] as f32 / 255.0;
                let linear_col = cce_ui::color::to_linear([r_f, g_f, b_f, 1.0]);
                cce_ui::color::set_node_color(linear_col);
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
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
                let idx = ((pos.y - cy) / 24.0) as usize;
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

        if !self.select_mode && self.paginator.cursor_moved(pos.x, pos.y, &mut self.ui_context) {
            changed = true;
        }

        if self.current_page == Page::Browse {
            if self.browse.search_box.cursor_moved(pos.x, pos.y, &mut self.ui_context) {
                changed = true;
            }
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
        } else if self.current_page == Page::Settings {
            if self.settings.color_selector.cursor_moved(pos.x, pos.y, &mut self.ui_context) {
                changed = true;
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
            let dialog_w = 400.0;
            let dialog_h = 160.0;
            let dialog_x = (self.width as f32 - dialog_w) / 2.0;
            let dialog_y = (self.height as f32 - dialog_h) / 2.0;

            let tb_x = dialog_x + 20.0;
            let tb_y = dialog_y + 60.0;
            let tb_w = dialog_w - 40.0;
            let tb_h = 28.0;

            let btn_cancel_x = dialog_x + dialog_w - 180.0;
            let btn_cancel_y = dialog_y + dialog_h - 44.0;
            let btn_cancel_w = 70.0;
            let btn_cancel_h = 28.0;

            let btn_open_x = dialog_x + dialog_w - 100.0;
            let btn_open_y = dialog_y + dialog_h - 44.0;
            let btn_open_w = 80.0;
            let btn_open_h = 28.0;

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
                    let idx = ((pos.y - cy) / 24.0) as usize;
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
                            options.push(("Open".to_string(), Some(Message::Browse(pages::browse::BrowseMessage::NavigateTo(idx)))));
                        } else {
                            options.push(("Select".to_string(), Some(Message::Browse(pages::browse::BrowseMessage::SelectEntry(idx)))));
                        }

                        options.push(("Open with...".to_string(), Some(Message::PromptOpenWith(entry.path.clone()))));

                        options.push(("Delete".to_string(), Some(Message::Browse(pages::browse::BrowseMessage::DeleteEntry(idx)))));

                        // Calculate width
                        let max_len = options.iter().map(|(s, _)| s.len()).max().unwrap_or(0);
                        let menu_w = ((max_len as f32 * 7.5) + 24.0).max(120.0);
                        let menu_h = options.len() as f32 * 24.0;

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

        if self.current_page == Page::Browse {
            if self.browse.search_box.mouse_input(button, state, pos.x, pos.y, &mut self.ui_context) {
                if state == ElementState::Pressed {
                    self.ui_context.set_focused(&mut self.browse.search_box);
                }
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
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
            }
            if button == MouseButton::Left && state == ElementState::Pressed {
                if self.browse.breadcrumb.hit_test(pos.x, pos.y, &self.ui_context) {
                    if self.browse.breadcrumb.mouse_input(button, state, pos.x, pos.y, &mut self.ui_context) {
                        if let Some(seg) = self.browse.breadcrumb.path_click() {
                            let mut target_path = std::path::PathBuf::new();
                            let mut current_idx = 0;
                            for component in self.browse.current_dir.components() {
                                target_path.push(component);
                                if component == std::path::Component::RootDir {
                                    if seg == 0 {
                                        break;
                                    }
                                } else {
                                    current_idx += 1;
                                    if current_idx == seg {
                                        break;
                                    }
                                }
                            }
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
                                let mut target_path = std::path::PathBuf::new();
                                let mut current_idx = 0;
                                for component in self.browse.current_dir.components() {
                                    target_path.push(component);
                                    if component == std::path::Component::RootDir {
                                        if seg == 0 {
                                            break;
                                        }
                                    } else {
                                        current_idx += 1;
                                        if current_idx == seg {
                                            break;
                                        }
                                    }
                                }
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
                            let selected_path = Some(entry.path.clone());
                            if let Some(path) = selected_path {
                                self.fs_service.send(services::fs::FsRequest::ReadPreview(path));
                            }
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
        } else if self.current_page == Page::Settings {
            if self.settings.color_selector.mouse_input(button, state, pos.x, pos.y, &mut self.ui_context) {
                if self.settings.color_selector.take_click() {
                    let col = self.settings.color_selector.color;
                    let r_f = col[0] as f32 / 255.0;
                    let g_f = col[1] as f32 / 255.0;
                    let b_f = col[2] as f32 / 255.0;
                    let linear_col = cce_ui::color::to_linear([r_f, g_f, b_f, 1.0]);
                    cce_ui::color::set_node_color(linear_col);
                }
                changed = true;
            }
            // If user clicked outside color selector focus area, unfocus it
            if state == ElementState::Pressed && !self.settings.color_selector.hit_test(pos.x, pos.y, &self.ui_context) {
                self.settings.color_selector.unfocus();
                self.ui_context.clear_focus();
                changed = true;
            }

            if changed {
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
        }

        if state == ElementState::Pressed {
            let clicked_search = self.current_page == Page::Browse && self.browse.search_box.hit_test(pos.x, pos.y, &self.ui_context);
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
            let has_sidebar = !self.select_mode;
            let sidebar_w = if has_sidebar { self.paginator.sidebar_w() } else { 0.0 };
            let browse_x = if has_sidebar { sidebar_w + 17.0 } else { 16.0 };
            let usable_w = self.width as f32 - sidebar_w - (if has_sidebar { 1.0 } else { 0.0 }) - 32.0;
            let browse_w = (usable_w - 12.0) * 0.5;
            let preview_w = (usable_w - 12.0) * 0.5;
            let preview_x = browse_x + browse_w + 12.0;
            let content_y = 16.0;

            let select_bar_h = 48.0;
            let content_h = if self.select_mode {
                self.height as f32 - 32.0 - select_bar_h
            } else {
                self.height as f32 - 32.0
            };

            let half_h = content_h * 0.5;
            let px = preview_x + 12.0;
            let py = content_y + 32.0;
            let pw = preview_w - 24.0;
            let ph = half_h - 40.0;

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

        if self.current_page == Page::Settings && self.settings.color_selector.editing {
            if self.settings.color_selector.keyboard_input(event, &mut self.ui_context) {
                *needs_rebuild = true;
                self.needs_rebuild = true;
                if !self.settings.color_selector.editing {
                    let col = self.settings.color_selector.color;
                    let r_f = col[0] as f32 / 255.0;
                    let g_f = col[1] as f32 / 255.0;
                    let b_f = col[2] as f32 / 255.0;
                    let linear_col = cce_ui::color::to_linear([r_f, g_f, b_f, 1.0]);
                    cce_ui::color::set_node_color(linear_col);
                }
                return None;
            }
        }

        // If the search textbox is focused, forward key inputs to it
        if self.current_page == Page::Browse && self.browse.search_box.editing {
            if self.browse.search_box.keyboard_input(event, &mut self.ui_context) {
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
                            } else if self.select_mode && !self.select_directory {
                                if self.save_mode {
                                    return Some(Message::SelectOpen);
                                } else {
                                    println!("{}", entry.path.display());
                                    std::process::exit(0);
                                }
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
                    Some("/") => {
                        self.browse.search_box.focus();
                        self.ui_context.set_focused(&mut self.browse.search_box);
                        *needs_rebuild = true;
                        self.needs_rebuild = true;
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
