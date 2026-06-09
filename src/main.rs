mod pages;

use wayland_client::QueueHandle;
use glyphon::{Attrs, Buffer, FontSystem, Metrics};

use clear_ui::engine::{Application, LogicalPosition, LogicalSize, WindowSettings};
use clear_ui::widget::{MouseButton, ElementState, MouseScrollDelta, KeyEvent, TextItem, Element};

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
    page_buttons: Vec<pages::ContentButton>,
    cursor_x: f32,
    cursor_y: f32,
    paginator: clear_ui::widget::Paginator,
    just_initialized: bool,
    ui_context: clear_ui::context::UiContext,
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

fn make_text_buffer(fs: &mut FontSystem, text: &str, size: f32) -> Buffer {
    let metrics = Metrics::new(size, size * 1.4);
    let mut buf = Buffer::new(fs, metrics);
    buf.set_text(fs, text, Attrs::new(), glyphon::Shaping::Advanced);
    buf.shape_until_scroll(fs, true);
    buf
}

// ── Layout Rebuild ──────────────────────────────────────────────────

impl FilesystemApp {
    fn rebuild_layout(&mut self) {
        self.browse.save_name_box.prepare_text(&mut self.font_system);
        self.browse.search_box.prepare_text(&mut self.font_system);

        let mut widgets = Vec::new();
        let mut text_items = Vec::new();

        clear_ui::widget::hover_animation::reset_frame_registration();
        clear_ui::widget::hover_animation::set_cursor_pos(self.cursor_x, self.cursor_y);

        let mut pc = pages::PageContent::new();

        // 1. Render the Paginator widget to separate PageContent first
        let page_idx = Page::ALL.iter().position(|&p| p == self.current_page).unwrap_or(0);
        self.paginator.set_selected_page(page_idx);
        let mut paginator_pc = pages::PageContent::new();
        clear_ui::layout::render_widget(&mut paginator_pc, &mut self.paginator, 0.0, 0.0, self.width as f32, self.height as f32, &mut self.ui_context);

        let has_sidebar = true;
        let sidebar_w = if has_sidebar { self.paginator.sidebar_w() } else { 0.0 };
        let browse_x = if has_sidebar { sidebar_w + 17.0 } else { 16.0 };
        let usable_w = self.width as f32 - sidebar_w - (if has_sidebar { 1.0 } else { 0.0 }) - 32.0;
        let browse_w = usable_w * 0.55;
        let preview_w = usable_w * 0.45;
        let preview_x = browse_x + browse_w + 12.0;
        let content_y = 16.0;

        let select_bar_h = 48.0;
        let content_h = if self.select_mode {
            self.height as f32 - 32.0 - select_bar_h
        } else {
            self.height as f32 - 32.0
        };

        if has_sidebar {
            // 2. Add Content Background (where x >= sidebar_w) from the paginator to pc first
            for rect in paginator_pc.rects.iter() {
                if rect.1 >= sidebar_w {
                    pc.rects.push(*rect);
                }
            }
        } else {
            // Full window page background
            let bg_color = clear_ui::color::page_low_color();
            pc.rect(bg_color, 0.0, 0.0, self.width as f32, self.height as f32);
        }

        // 3. Draw Page content
        match self.current_page {
            Page::Browse => {
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
            // 4. Add Sidebar Background and tabs (where x < sidebar_w) from the paginator to pc last
            for rect in paginator_pc.rects.iter() {
                if rect.1 < sidebar_w {
                    pc.rects.push(*rect);
                }
            }

            // Sidebar Divider Line (accent border)
            pc.rect([0.36, 0.56, 0.38, 1.0], sidebar_w, 0.0, 1.0, self.height as f32);

            // Add the paginator's texts (tab labels) on top of the sidebar background
            pc.texts.extend(paginator_pc.texts);
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
        for btn in &pc.buttons {
            let hovering = self.cursor_x >= btn.x && self.cursor_x <= btn.x + btn.w
                && self.cursor_y >= btn.y && self.cursor_y <= btn.y + btn.h;
            let col = if hovering { btn.hover_bg } else { btn.bg };

            widgets.push(AppWidget {
                x: btn.x,
                y: btn.y,
                w: btn.w,
                h: btn.h,
                color: col,
                hover_color: btn.hover_bg,
                hovering,
                action: Some(btn.action.clone()),
            });

            // Draw button text
            let text_x = if btn.left_align {
                btn.x + 8.0
            } else {
                let text_w = btn.label.chars().count() as f32 * btn.label_size * 0.65;
                btn.x + (btn.w - text_w) / 2.0
            };
            let text_y = btn.y + (btn.h - btn.label_size * 1.4) / 2.0;

            text_items.push(TextItem {
                buffer: make_text_buffer(&mut self.font_system, &btn.label, btn.label_size),
                x: text_x,
                y: text_y,
                color: glyphon::Color::rgb(
                    (btn.label_color[0] * 255.0) as u8,
                    (btn.label_color[1] * 255.0) as u8,
                    (btn.label_color[2] * 255.0) as u8,
                ),
                bounds: None,
            });
        }

        // Add raw texts
        for (text, size, x, y, col, _font, bounds) in &pc.texts {
            text_items.push(TextItem {
                buffer: make_text_buffer(&mut self.font_system, text, *size),
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

        // Use signature green theme for the content background of the paginator
        clear_ui::color::set_page_low_color([0.10, 0.165, 0.11, 1.0]);
        clear_ui::scale::set_scale_factor(1.0);

        let browse = pages::browse::BrowseState::default();
        let current_dir = browse.current_dir.clone();

        let pages_names = Page::ALL.iter().map(|p| p.label().to_string()).collect::<Vec<_>>();
        let paginator = clear_ui::widget::Paginator::new(56.0, pages_names)
            .with_sidebar_label("CLEAR")
            .with_tabs_rotated(true);

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
            font_system: FontSystem::new(),
            needs_rebuild: true,
            width: if select_mode { 900 } else { 1200 },
            height: if select_mode { 500 } else { 720 },
            scale_factor: 1.0,
            sender: sender.clone(),
            page_buttons: Vec::new(),
            cursor_x: 0.0,
            cursor_y: 0.0,
            paginator,
            just_initialized: true,
            ui_context: clear_ui::context::UiContext::new(),
        };

        // Start initial directory loading
        let sender_clone = sender.clone();
        let path_clone = current_dir.clone();
        tokio::spawn(async move {
            let entries = pages::browse::read_directory(&path_clone);
            let _ = sender_clone.send(Message::Browse(pages::browse::BrowseMessage::DirectoryLoaded(path_clone, entries)));
        });

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

                let is_navigation = match &msg {
                    pages::browse::BrowseMessage::NavigateTo(idx) => {
                        self.browse.entries.get(*idx).map(|e| e.is_dir && !is_project_dir(&e.path)).unwrap_or(false)
                    }
                    pages::browse::BrowseMessage::NavigateToPath(_) => true,
                    _ => false,
                };
                let (target_path, handle) = pages::browse::update(&mut self.browse, msg);
                if is_navigation {
                    let sender_clone = self.sender.clone();
                    tokio::spawn(async move {
                        let entries = handle.await.unwrap_or_default();
                        let _ = sender_clone.send(Message::Browse(pages::browse::BrowseMessage::DirectoryLoaded(target_path, entries)));
                    });
                }

                // If NavigateTo or SelectEntry happened, update Preview path
                let selected_path = if let Some(idx) = self.browse.selected {
                    self.browse.entries.get(idx).map(|e| e.path.clone())
                } else {
                    None
                };
                if let Some(path) = selected_path {
                    pages::preview::update(&mut self.preview, pages::preview::PreviewMessage::SetPath { path });
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
                            let sender_clone = self.sender.clone();
                            let path_clone = path.clone();
                            tokio::spawn(async move {
                                let entries = pages::browse::read_directory(&path_clone);
                                let _ = sender_clone.send(Message::Browse(pages::browse::BrowseMessage::DirectoryLoaded(path_clone, entries)));
                            });
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
                                let sender_clone = self.sender.clone();
                                let path_clone = path.clone();
                                tokio::spawn(async move {
                                    let entries = pages::browse::read_directory(&path_clone);
                                    let _ = sender_clone.send(Message::Browse(pages::browse::BrowseMessage::DirectoryLoaded(path_clone, entries)));
                                });
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
        let mut color = [0.10, 0.165, 0.11, 1.0];
        if let Some(opacity) = clear_ui::color::read_opacity_if_configured() {
            color[3] = opacity;
        }
        color
    }

    fn handle_pointer_move(&mut self, pos: LogicalPosition, needs_rebuild: &mut bool) {
        self.cursor_x = pos.x;
        self.cursor_y = pos.y;

        let mut changed = false;

        if self.paginator.cursor_moved(pos.x, pos.y, &mut self.ui_context) {
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
        for btn in &self.page_buttons {
            let _hovering = self.cursor_x >= btn.x && self.cursor_x <= btn.x + btn.w
                && self.cursor_y >= btn.y && self.cursor_y <= btn.y + btn.h;
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

        let mut changed = false;

        eprintln!("[DEBUG] MOUSE INPUT: {:?} {:?} pos=({}, {})", button, state, pos.x, pos.y);
        let pag_match = self.paginator.mouse_input(button, state, pos.x, pos.y, &mut self.ui_context);
        eprintln!("[DEBUG] Paginator matched: {}", pag_match);
        if pag_match {
            if self.paginator.take_click() {
                let idx = self.paginator.selected_page();
                eprintln!("[DEBUG] Paginator page changed to idx: {}", idx);
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
                            let sender_clone = self.sender.clone();
                            let path_clone = target_path.clone();
                            tokio::spawn(async move {
                                let entries = pages::browse::read_directory(&path_clone);
                                let _ = sender_clone.send(Message::Browse(pages::browse::BrowseMessage::DirectoryLoaded(path_clone, entries)));
                            });
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
                                let sender_clone = self.sender.clone();
                                  let path_clone = target_path.clone();
                                tokio::spawn(async move {
                                    let entries = pages::browse::read_directory(&path_clone);
                                    let _ = sender_clone.send(Message::Browse(pages::browse::BrowseMessage::DirectoryLoaded(path_clone, entries)));
                                });
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
                                pages::preview::update(&mut self.preview, pages::preview::PreviewMessage::SetPath { path });
                            }
                        }
                        changed = true;
                    }
                } else {
                    if self.browse.selected.is_some() {
                        self.browse.selected = None;
                        changed = true;
                    }
                }
            } else {
                if self.browse.selected.is_some() {
                    self.browse.selected = None;
                    changed = true;
                }
            }

            // Sync double click navigation
            if let Some(dbl_idx) = self.network.graph.double_clicked_node() {
                self.network.graph.clear_double_clicked_node();
                if has_parent && dbl_idx == 0 {
                    if let Some(parent) = self.browse.current_dir.parent() {
                        let parent_path = parent.to_path_buf();
                        let sender_clone = self.sender.clone();
                        let path_clone = parent_path.clone();
                        tokio::spawn(async move {
                            let entries = pages::browse::read_directory(&path_clone);
                            let _ = sender_clone.send(Message::Browse(pages::browse::BrowseMessage::DirectoryLoaded(path_clone, entries)));
                        });
                        changed = true;
                    }
                } else if dbl_idx >= offset {
                    let entry_idx = dbl_idx - offset;
                    if let Some(entry) = self.browse.entries.get(entry_idx) {
                        if entry.is_dir && !is_project_dir(&entry.path) {
                            let path = entry.path.clone();
                            let sender_clone = self.sender.clone();
                            let path_clone = path.clone();
                            tokio::spawn(async move {
                                let entries = pages::browse::read_directory(&path_clone);
                                let _ = sender_clone.send(Message::Browse(pages::browse::BrowseMessage::DirectoryLoaded(path_clone, entries)));
                            });
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
            for btn in &self.page_buttons {
                if pos.x >= btn.x && pos.x <= btn.x + btn.w && pos.y >= btn.y && pos.y <= btn.y + btn.h {
                    *needs_rebuild = true;
                    self.needs_rebuild = true;
                    return Some(btn.action.clone());
                }
            }
        }

        None
    }

    fn handle_mouse_wheel(&mut self, delta: &MouseScrollDelta, pos: LogicalPosition, needs_rebuild: &mut bool) {
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
