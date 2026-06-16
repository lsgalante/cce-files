mod pages;
mod services;

use wayland_client::QueueHandle;
use glyphon::{Attrs, Buffer, FontSystem, Metrics};

use clear_ui::engine::{Application, LogicalPosition, LogicalSize, WindowSettings};
use clear_ui::widget::{MouseButton, ElementState, MouseScrollDelta, KeyEvent, TextItem, Element, PageSelector, MenuController};
use clear_ui::widget::{GraphController, PathController};

use notify::{Watcher, RecommendedWatcher, RecursiveMode, Config};

use pages::Page;
use pages::browse::is_project_dir;

// ── State ───────────────────────────────────────────────────────────

struct AppWidget {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: [f32; 4],
    hover_color: [f32; 4],
    hovering: bool,
    action: Option<Message>,
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
    page_buttons: Vec<(clear_ui::widget::Button, Message)>,
    cursor_x: f32,
    cursor_y: f32,
    menubar: clear_ui::widget::MenuBar,
    just_initialized: bool,
    ui_context: clear_ui::context::UiContext,
    watcher: Option<notify::RecommendedWatcher>,
    fs_service: crate::services::fs::FsService,
    context_menu: ContextMenu,
}

// ── Messages ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Message {
    SwitchPage(Page),
    Browse(pages::browse::BrowseMessage),
    Preview(pages::preview::PreviewMessage),
    KeyboardEvent(KeyEvent),
    SelectOpen,
    SelectCancel,
}

// ── Helpers ─────────────────────────────────────────────────────────

fn make_text_buffer(fs: &mut FontSystem, text: &str, size: f32, font: Option<&str>) -> Buffer {
    let scale = clear_ui::scale::scale_factor();
    let mut font_size = size;
    let mut family_name = None;

    if let Some(f) = font {
        let (parsed_family, parsed_size) = clear_ui::layout::parse_font_string(f);
        if let Some(ps) = parsed_size {
            font_size = ps;
        }
        family_name = Some(parsed_family);
    }

    let physical_size = font_size * scale;
    let metrics = Metrics::new(physical_size, physical_size * 1.4);
    let mut buf = Buffer::new(fs, metrics);
    let mut attrs = Attrs::new();

    if let Some(ref fam) = family_name {
        let family = match fam.as_str() {
            "monospace" => glyphon::Family::Name(clear_ui::layout::get_system_monospace_font()),
            "sans-serif" => glyphon::Family::SansSerif,
            "serif" => glyphon::Family::Serif,
            _ => glyphon::Family::Name(fam),
        };
        attrs = attrs.family(family);
    }

    buf.set_text(fs, text, attrs, glyphon::Shaping::Advanced);
    buf.shape_until_scroll(fs, true);
    buf
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

                let _ = fs_service.send(crate::services::fs::FsRequest::RefreshDirectory(path_clone.clone())).await;
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
                eprintln!("Failed to create watcher: {:?}", e);
                return;
            }
        };

        if let Err(e) = watcher.watch(&path, RecursiveMode::NonRecursive) {
            eprintln!("Failed to watch path {}: {:?}", path.display(), e);
            return;
        }

        self.watcher = Some(watcher);
    }

    fn rebuild_layout(&mut self) {
        self.browse.save_name_box.prepare_text(&mut self.font_system);
        self.browse.search_box.prepare_text(&mut self.font_system);

        let mut widgets = Vec::new();
        let mut text_items = Vec::new();

        clear_ui::widget::hover_animation::reset_frame_registration();
        clear_ui::widget::hover_animation::set_cursor_pos(self.cursor_x, self.cursor_y);

        let mut pc = pages::PageContent::new();

        let has_sidebar = !self.select_mode;
        let sidebar_w = if has_sidebar { self.menubar.sidebar_w() } else { 0.0 };
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

        // Full window page background
        let bg_color = clear_ui::color::page_low_color();
        pc.rect(bg_color, 0.0, 0.0, self.width as f32, self.height as f32);

        let mut menubar_pc = pages::PageContent::new();
        if has_sidebar {
            // 1. Render the Menubar widget directly
            let page_idx = Page::ALL.iter().position(|&p| p == self.current_page).unwrap_or(0);
            self.menubar.menus.set_selected(Some(page_idx));
            clear_ui::layout::render_widget(&mut menubar_pc, &mut self.menubar, 0.0, 0.0, sidebar_w, self.height as f32, &mut self.ui_context);
        }

        // 3. Draw Page content
        match self.current_page {
            Page::Browse => {
                // Draw background plates for columns
                let plate_bg = [0.08, 0.13, 0.09, 0.45];
                let border_color = [0.15, 0.23, 0.17, 0.7];

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
                let preview_pc = pages::preview::view(&self.preview, preview_x, content_y, preview_w, content_h);

                pc.rects.extend(browse_pc.rects);
                pc.texts.extend(browse_pc.texts);
                pc.buttons.extend(browse_pc.buttons);

                pc.rects.extend(preview_pc.rects);
                pc.texts.extend(preview_pc.texts);
                pc.buttons.extend(preview_pc.buttons);
            }
            Page::Network => {
                let network_pc = pages::network::view(&mut self.network, &self.browse, browse_x, content_y, browse_w, content_h, &mut self.ui_context);
                let preview_pc = pages::preview::view(&self.preview, preview_x, content_y, preview_w, content_h);

                pc.rects.extend(network_pc.rects);
                pc.texts.extend(network_pc.texts);
                pc.buttons.extend(network_pc.buttons);

                pc.rects.extend(preview_pc.rects);
                pc.texts.extend(preview_pc.texts);
                pc.buttons.extend(preview_pc.buttons);
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
                let linear_col = clear_ui::color::to_linear([r_f, g_f, b_f, 1.0]);
                if clear_ui::color::node_color() != linear_col {
                    clear_ui::color::set_node_color(linear_col);
                }
            }
        }

        if has_sidebar {
            // Add Sidebar Background and elements directly from the menubar to pc
            pc.rects.extend(menubar_pc.rects);



            // Add the menubar's texts (tab labels) on top of the sidebar background
            pc.texts.extend(menubar_pc.texts);
            pc.buttons.extend(menubar_pc.buttons);
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

        // 4. Translate PageContent into rendering quads and TextItems
        for (col, x, y, w, h) in &pc.rects {
            widgets.push(AppWidget {
                x: *x,
                y: *y,
                w: *w,
                h: *h,
                color: *col,
                hover_color: *col,
                hovering: false,
                action: None,
            });
        }

        // Add page buttons
        for (btn, action) in &pc.buttons {
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
                action: Some(action.clone()),
            });

            // Draw button text
            let text_x = if btn.left_align {
                base.x + 8.0
            } else {
                let text_w = label.chars().count() as f32 * label_size * 0.65;
                base.x + (base.w - text_w) / 2.0
            };
            let text_y = base.y + (base.h - label_size * 1.4) / 2.0;

            text_items.push(TextItem {
                buffer: make_text_buffer(&mut self.font_system, label, label_size, None),
                x: text_x,
                y: text_y,
                color: glyphon::Color::rgb(
                    (label_color[0] * 255.0) as u8,
                    (label_color[1] * 255.0) as u8,
                    (label_color[2] * 255.0) as u8,
                ),
                bounds: None,
            });
        }

        // Add raw texts
        for (text, size, x, y, col, font, bounds) in &pc.texts {
            text_items.push(TextItem {
                buffer: make_text_buffer(&mut self.font_system, text, *size, font.as_deref()),
                x: *x,
                y: *y,
                color: glyphon::Color::rgb(
                    (col[0] * 255.0) as u8,
                    (col[1] * 255.0) as u8,
                    (col[2] * 255.0) as u8,
                ),
                bounds: *bounds,
            });
        }

        // Render context menu overlay if visible
        if self.context_menu.visible {
            let cx = self.context_menu.x;
            let cy = self.context_menu.y;
            let cw = self.context_menu.w;
            let ch = self.context_menu.h;

            // Border
            widgets.push(AppWidget {
                x: cx, y: cy, w: cw, h: ch,
                color: [0.22, 0.22, 0.28, 1.0], hover_color: [0.22, 0.22, 0.28, 1.0],
                hovering: false,
                action: None,
            });
            // Background
            widgets.push(AppWidget {
                x: cx + 1.0, y: cy + 1.0, w: cw - 2.0, h: ch - 2.0,
                color: [0.06, 0.06, 0.09, 1.0], hover_color: [0.06, 0.06, 0.09, 1.0],
                hovering: false,
                action: None,
            });
            // Hover highlight
            if let Some(h_idx) = self.context_menu.hovered {
                let iy = cy + h_idx as f32 * 24.0;
                widgets.push(AppWidget {
                    x: cx + 2.0, y: iy + 2.0, w: cw - 4.0, h: 20.0,
                    color: [0.20, 0.40, 0.65, 0.6], hover_color: [0.20, 0.40, 0.65, 0.6],
                    hovering: false,
                    action: None,
                });
            }
            // Text options
            for (idx, (opt, _)) in self.context_menu.options.iter().enumerate() {
                let iy = cy + idx as f32 * 24.0 + (24.0 - 12.0) / 2.0;
                let text_color = if idx == 0 {
                    glyphon::Color::rgb(0x70, 0x70, 0x78)
                } else if self.context_menu.hovered == Some(idx) {
                    glyphon::Color::rgb(0xff, 0xff, 0xff)
                } else {
                    glyphon::Color::rgb(0xcc, 0xcc, 0xd4)
                };
                text_items.push(TextItem {
                    buffer: make_text_buffer(&mut self.font_system, opt, 12.0, None),
                    x: cx + 8.0, y: iy,
                    color: text_color,
                    bounds: None,
                });
            }
        }

        self.widgets = widgets;
        self.text_items = text_items;
        self.page_buttons = pc.buttons;
        self.needs_rebuild = false;
    }
}

// ── Application Trait Implementation ────────────────────────────────

impl Application for FilesystemApp {
    type Message = Message;

    fn new(_qh: &QueueHandle<clear_ui::engine::EngineState<Self>>, sender: calloop::channel::Sender<Self::Message>) -> Self {
        // Parse command line arguments
        let args: Vec<String> = std::env::args().collect();
        let select_directory = args.iter().any(|arg| arg == "--select-dir");
        let save_mode = args.iter().any(|arg| arg == "--save");
        let select_mode = save_mode || args.iter().any(|arg| arg == "--select" || arg == "--select-dir");

        clear_ui::scale::set_scale_factor(1.0);

        let browse = pages::browse::BrowseState::default();
        let current_dir = browse.current_dir.clone();

        let pages_names = Page::ALL.iter().map(|p| p.label().to_string()).collect::<Vec<_>>();
        let mut menubar = clear_ui::widget::MenuBar::new(0.0, 0.0, 56.0, 0.0)
            .with_vertical(true);
        menubar.set_sidebar_label(Some("CLEAR".to_string()));
        menubar.set_pages(pages_names);

        let fs_service = crate::services::fs::FsService::new(sender.clone());
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
            width: if select_mode { 900 } else { 1200 },
            height: if select_mode { 500 } else { 720 },
            scale_factor: 1.0,
            sender: sender.clone(),
            page_buttons: Vec::new(),
            cursor_x: 0.0,
            cursor_y: 0.0,
            menubar,
            just_initialized: true,
            ui_context: clear_ui::context::UiContext::new(),
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
        };

        // Start initial directory loading via FsService
        app.fs_service.send(crate::services::fs::FsRequest::ReadLastDir);

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
                self.menubar.menus.set_selected(Some(page_idx));
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
                    self.fs_service.send(crate::services::fs::FsRequest::ReadPreview(path));
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
                            self.fs_service.send(crate::services::fs::FsRequest::ReadDirectory(path));
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
                                self.fs_service.send(crate::services::fs::FsRequest::ReadDirectory(path));
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

        if self.menubar.tick(dt, &mut self.ui_context) {
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }

        if self.current_page == Page::Settings {
            if self.settings.color_selector.tick(dt, &mut self.ui_context) {
                let col = self.settings.color_selector.color;
                let r_f = col[0] as f32 / 255.0;
                let g_f = col[1] as f32 / 255.0;
                let b_f = col[2] as f32 / 255.0;
                let linear_col = clear_ui::color::to_linear([r_f, g_f, b_f, 1.0]);
                clear_ui::color::set_node_color(linear_col);
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
        }
    }

    fn view(&mut self, quads: &mut Vec<(f32, f32, f32, f32, [f32; 4])>, size: LogicalSize, scale: f64) {
        if self.needs_rebuild || self.width != size.width as u32 || self.height != size.height as u32 || self.scale_factor != scale {
            self.width = size.width as u32;
            self.height = size.height as u32;
            self.scale_factor = scale;
            clear_ui::scale::set_scale_factor(scale as f32);
            self.rebuild_layout();
        }

        for w in &self.widgets {
            quads.push((w.x, w.y, w.w, w.h, w.color));
        }
    }

    fn text_items(&self) -> &[TextItem] {
        &self.text_items
    }

    fn clear_color(&self) -> [f32; 4] {
        let mut color = clear_ui::color::page_low_color();
        if let Some(opacity) = clear_ui::color::read_opacity_if_configured() {
            color[3] = opacity;
        }
        color
    }

    fn handle_pointer_move(&mut self, pos: LogicalPosition, needs_rebuild: &mut bool) {
        self.cursor_x = pos.x;
        self.cursor_y = pos.y;

        let mut changed = false;

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

        if !self.select_mode && self.menubar.cursor_moved(pos.x, pos.y, &mut self.ui_context) {
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

        eprintln!("[DEBUG] MOUSE INPUT: {:?} {:?} pos=({}, {})", button, state, pos.x, pos.y);
        let menu_match = !self.select_mode && self.menubar.mouse_input(button, state, pos.x, pos.y, &mut self.ui_context);
        if menu_match {
            if let Some((idx, _)) = self.menubar.menu_click() {
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
                            self.fs_service.send(crate::services::fs::FsRequest::ReadDirectory(target_path));
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
                                self.fs_service.send(crate::services::fs::FsRequest::ReadDirectory(target_path));
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
                                self.fs_service.send(crate::services::fs::FsRequest::ReadPreview(path));
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
                        self.fs_service.send(crate::services::fs::FsRequest::ReadDirectory(parent_path));
                        changed = true;
                    }
                } else if dbl_idx >= offset {
                    let entry_idx = dbl_idx - offset;
                    if let Some(entry) = self.browse.entries.get(entry_idx) {
                        if entry.is_dir && !is_project_dir(&entry.path) {
                            let path = entry.path.clone();
                            self.fs_service.send(crate::services::fs::FsRequest::ReadDirectory(path));
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
                    let linear_col = clear_ui::color::to_linear([r_f, g_f, b_f, 1.0]);
                    clear_ui::color::set_node_color(linear_col);
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
            let sidebar_w = if has_sidebar { self.menubar.sidebar_w() } else { 0.0 };
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

        if self.context_menu.visible {
            if event.logical_key == clear_ui::widget::Key::Named(clear_ui::widget::NamedKey::Escape) {
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
                    let linear_col = clear_ui::color::to_linear([r_f, g_f, b_f, 1.0]);
                    clear_ui::color::set_node_color(linear_col);
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
                if event.logical_key == clear_ui::widget::Key::Named(clear_ui::widget::NamedKey::Enter) {
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
                clear_ui::widget::Key::Character(c) => Some(c.as_str()),
                _ => None,
            };

            match &event.logical_key {
                clear_ui::widget::Key::Named(clear_ui::widget::NamedKey::ArrowUp) => {
                    if let Some(index) = pages::browse::next_selection_index(&self.browse, pages::browse::BrowseNavigation::Up) {
                        return Some(Message::Browse(pages::browse::BrowseMessage::SelectEntry(index)));
                    }
                }
                clear_ui::widget::Key::Named(clear_ui::widget::NamedKey::ArrowDown) => {
                    if let Some(index) = pages::browse::next_selection_index(&self.browse, pages::browse::BrowseNavigation::Down) {
                        return Some(Message::Browse(pages::browse::BrowseMessage::SelectEntry(index)));
                    }
                }
                clear_ui::widget::Key::Named(clear_ui::widget::NamedKey::Enter) => {
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
                clear_ui::widget::Key::Named(clear_ui::widget::NamedKey::Delete) => {
                    if let Some(idx) = self.browse.selected {
                        return Some(Message::Browse(pages::browse::BrowseMessage::DeleteEntry(idx)));
                    }
                }
                clear_ui::widget::Key::Named(clear_ui::widget::NamedKey::Escape) => {
                    if self.select_mode {
                        std::process::exit(1);
                    }
                }
                clear_ui::widget::Key::Named(clear_ui::widget::NamedKey::Backspace) => {
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
    clear_ui::engine::run::<FilesystemApp>();
}
