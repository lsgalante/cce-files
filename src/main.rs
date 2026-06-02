mod pages;

use std::path::PathBuf;
use wayland_client::QueueHandle;
use glyphon::{Attrs, Buffer, FontSystem, Metrics};

use clear_ui::engine::{Application, LogicalPosition, LogicalSize, WindowSettings};
use clear_ui::widget::{MouseButton, ElementState, MouseScrollDelta, KeyEvent, TextItem, Widget};

use pages::Page;

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
    preview: pages::preview::PreviewState,
    keybindings: pages::keybindings::KeybindingsState,

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
}

// ── Messages ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Message {
    SwitchPage(Page),
    Browse(pages::browse::BrowseMessage),
    Preview(pages::preview::PreviewMessage),
    Keybindings(pages::keybindings::KeybindingsMessage),
    KeyboardEvent(KeyEvent),
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
        let mut widgets = Vec::new();
        let mut text_items = Vec::new();

        clear_ui::widget::hover_animation::reset_frame_registration();
        clear_ui::widget::hover_animation::set_cursor_pos(self.cursor_x, self.cursor_y);

        let mut pc = pages::PageContent::new();

        // 1. Draw Sidebar Background
        let sidebar_bg = [0.086, 0.141, 0.094, 1.0]; // matching 0x16, 0x24, 0x18
        pc.rect(sidebar_bg, 0.0, 0.0, 200.0, self.height as f32);

        // Sidebar Divider Line (accent border)
        pc.rect([0.36, 0.56, 0.38, 1.0], 200.0, 0.0, 1.0, self.height as f32);

        // Sidebar Title
        pc.text("Clear Filesystem", 16.0, 16.0, 15.0, [0.36, 0.56, 0.38, 1.0]);

        // Sidebar buttons
        let mut btn_y = 48.0;
        for page in Page::ALL {
            let active = page == self.current_page;
            let bg = if active {
                [0.16, 0.29, 0.18, 1.0]
            } else {
                [0.12, 0.18, 0.13, 1.0]
            };
            let fg = if active {
                [0.56, 0.83, 0.56, 1.0]
            } else {
                [0.60, 0.60, 0.67, 1.0]
            };

            let label = format!("{}  {}", page.icon(), page.label());

            pc.button(
                &label,
                12.0,
                btn_y,
                176.0,
                36.0,
                bg,
                if active { bg } else { [0.18, 0.28, 0.20, 1.0] },
                fg,
                Message::SwitchPage(page),
            );

            btn_y += 42.0;
        }

        // 2. Draw Content Background
        let content_bg = [0.10, 0.165, 0.11, 1.0]; // matching 0x1a, 0x2a, 0x1c
        pc.rect(content_bg, 201.0, 0.0, self.width as f32 - 201.0, self.height as f32);

        // 3. Draw Page content
        let usable_w = self.width as f32 - 201.0 - 32.0;
        let browse_w = usable_w * 0.55;
        let preview_w = usable_w * 0.45;
        let browse_x = 217.0;
        let preview_x = browse_x + browse_w + 12.0;
        let content_y = 16.0;
        let content_h = self.height as f32 - 32.0;

        match self.current_page {
            Page::Browse => {
                let browse_pc = pages::browse::view(&mut self.browse, browse_x, content_y, browse_w, content_h);
                let preview_pc = pages::preview::view(&self.preview, preview_x, content_y, preview_w, content_h);

                pc.rects.extend(browse_pc.rects);
                pc.texts.extend(browse_pc.texts);
                pc.buttons.extend(browse_pc.buttons);

                pc.rects.extend(preview_pc.rects);
                pc.texts.extend(preview_pc.texts);
                pc.buttons.extend(preview_pc.buttons);
            }
            Page::Keybindings => {
                let keys_pc = pages::keybindings::view(&self.keybindings, browse_x, content_y, usable_w, content_h);

                pc.rects.extend(keys_pc.rects);
                pc.texts.extend(keys_pc.texts);
                pc.buttons.extend(keys_pc.buttons);
            }
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
            });
        }

        // Add raw texts
        for (text, size, x, y, col, _font) in &pc.texts {
            text_items.push(TextItem {
                buffer: make_text_buffer(&mut self.font_system, text, *size),
                x: *x,
                y: *y,
                color: glyphon::Color::rgb(
                    (col[0] * 255.0) as u8,
                    (col[1] * 255.0) as u8,
                    (col[2] * 255.0) as u8,
                ),
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
        let browse = pages::browse::BrowseState::default();
        let current_dir = browse.current_dir.clone();

        let mut app = Self {
            current_page: Page::Browse,
            browse,
            preview: pages::preview::PreviewState::default(),
            keybindings: pages::keybindings::KeybindingsState,
            widgets: Vec::new(),
            text_items: Vec::new(),
            font_system: FontSystem::new(),
            needs_rebuild: true,
            width: 1200,
            height: 720,
            scale_factor: 1.0,
            sender: sender.clone(),
            page_buttons: Vec::new(),
            cursor_x: 0.0,
            cursor_y: 0.0,
        };

        // Start initial directory loading
        let sender_clone = sender.clone();
        tokio::spawn(async move {
            let entries = pages::browse::read_directory(&current_dir);
            let _ = sender_clone.send(Message::Browse(pages::browse::BrowseMessage::DirectoryLoaded(entries)));
        });

        app.rebuild_layout();
        app
    }

    fn settings(&self) -> WindowSettings {
        WindowSettings {
            title: "Clear Filesystem Interface".to_string(),
            app_id: "clear-filesystem-interface".to_string(),
            width: 1200,
            height: 720,
            fullscreen: false,
            min_size: Some((1020, 600)),
        }
    }

    fn update(&mut self, msg: Self::Message, needs_rebuild: &mut bool, _exit: &mut bool) {
        match msg {
            Message::SwitchPage(page) => {
                self.current_page = page;
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
            Message::Browse(msg) => {
                let is_navigation = match &msg {
                    pages::browse::BrowseMessage::NavigateTo(_) | pages::browse::BrowseMessage::NavigateToPath(_) => true,
                    _ => false,
                };
                let handle = pages::browse::update(&mut self.browse, msg);
                if is_navigation {
                    let sender_clone = self.sender.clone();
                    tokio::spawn(async move {
                        let entries = handle.await.unwrap_or_default();
                        let _ = sender_clone.send(Message::Browse(pages::browse::BrowseMessage::DirectoryLoaded(entries)));
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

                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
            Message::Preview(msg) => {
                let nav_path = match &msg {
                    pages::preview::PreviewMessage::NavigateTo(path) => Some(path.clone()),
                    _ => None,
                };
                pages::preview::update(&mut self.preview, msg);
                if let Some(path) = nav_path {
                    let sender_clone = self.sender.clone();
                    tokio::spawn(async move {
                        let entries = pages::browse::read_directory(&path);
                        let _ = sender_clone.send(Message::Browse(pages::browse::BrowseMessage::DirectoryLoaded(entries)));
                    });
                }
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
            Message::Keybindings(msg) => {
                pages::keybindings::update(&mut self.keybindings, msg);
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
            Message::KeyboardEvent(_) => {}
        }
    }

    fn tick(&mut self, _dt: f32, _needs_rebuild: &mut bool) {}

    fn view(&mut self, quads: &mut Vec<(f32, f32, f32, f32, [f32; 4])>, size: LogicalSize, scale: f64) {
        if self.needs_rebuild || self.width != size.width as u32 || self.height != size.height as u32 || self.scale_factor != scale {
            self.width = size.width as u32;
            self.height = size.height as u32;
            self.scale_factor = scale;
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
        [0.10, 0.165, 0.11, 1.0]
    }

    fn handle_pointer_move(&mut self, pos: LogicalPosition, needs_rebuild: &mut bool) {
        self.cursor_x = pos.x;
        self.cursor_y = pos.y;

        let mut changed = false;

        if self.current_page == Page::Browse {
            if self.browse.search_box.cursor_moved(pos.x, pos.y) {
                changed = true;
            }
            if self.browse.list_box.cursor_moved(pos.x, pos.y) {
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

        if self.current_page == Page::Browse {
            if self.browse.search_box.mouse_input(button, state, pos.x, pos.y) {
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
            if self.browse.list_box.mouse_input(button, state, pos.x, pos.y) {
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
        }

        if state == ElementState::Pressed {
            let clicked_search = self.current_page == Page::Browse && self.browse.search_box.hit_test(pos.x, pos.y);
            if !clicked_search {
                self.browse.search_box.unfocus();
                clear_ui::widget::focus::clear_focus();
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
            if self.browse.list_box.mouse_wheel(delta, pos.x, pos.y) {
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
        }
    }

    fn handle_key_input(&mut self, event: &KeyEvent, needs_rebuild: &mut bool) -> Option<Self::Message> {
        if event.state != ElementState::Pressed {
            return None;
        }

        // If the search textbox is focused, forward key inputs to it
        if self.current_page == Page::Browse && self.browse.search_box.editing {
            if self.browse.search_box.keyboard_input(event) {
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
                            if entry.is_dir {
                                return Some(Message::Browse(pages::browse::BrowseMessage::NavigateTo(idx)));
                            }
                        }
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
                                if entry.is_dir {
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
                        clear_ui::widget::focus::set_focused(&mut self.browse.search_box);
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
