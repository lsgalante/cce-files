use iced::widget::{button, column, operation, row, scrollable, text, text_input};
use iced::{Color, Element, Length, Task};
use iced::widget::scrollable::Viewport;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::Message;

// ── Data ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DirEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub permissions: u32,
    pub modified: String,
}

#[derive(Debug, Clone)]
pub struct BrowseState {
    pub current_dir: PathBuf,
    pub all_entries: Vec<DirEntry>,
    pub entries: Vec<DirEntry>,
    pub show_hidden: bool,
    pub search: String,
    pub selected: Option<usize>,
    pub scroll_offset_y: f32,
    pub viewport_height: f32,
}

impl Default for BrowseState {
    fn default() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
        Self {
            current_dir: PathBuf::from(home),
            all_entries: Vec::new(),
            entries: Vec::new(),
            show_hidden: false,
            search: String::new(),
            selected: None,
            scroll_offset_y: 0.0,
            viewport_height: 400.0,
        }
    }
}

#[derive(Debug, Clone)]
pub enum BrowseMessage {
    SearchChanged(String),
    SelectEntry(usize),
    NavigateTo(usize),
    NavigateToPath(PathBuf),
    DirectoryLoaded(Vec<DirEntry>),
    ToggleHidden,
    Scrolled(Viewport),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowseNavigation {
    Up,
    Down,
}

const LIST_SCROLL_ID: &str = "browse-file-list";
const LIST_ROW_HEIGHT: f32 = 34.0;
const LIST_ROW_SPACING: f32 = 2.0;

// ── Helpers ─────────────────────────────────────────────────────────

pub fn read_directory(path: &Path) -> Vec<DirEntry> {
    let mut entries: Vec<DirEntry> = match fs::read_dir(path) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let meta = e.metadata().ok()?;
                let name = e.file_name().to_string_lossy().to_string();
                let is_dir = meta.is_dir();
                let size = meta.len();
                let permissions = meta.permissions().mode();
                let modified = meta
                    .modified()
                    .ok()
                    .and_then(|t| {
                        let secs = t.duration_since(std::time::UNIX_EPOCH).ok()?;
                        let datetime =
                            chrono::DateTime::from_timestamp(secs.as_secs() as i64, 0)?;
                        Some(datetime.format("%Y-%m-%d %H:%M").to_string())
                    })
                    .unwrap_or_else(|| "—".to_string());
                Some(DirEntry {
                    name,
                    path: e.path(),
                    is_dir,
                    size,
                    permissions,
                    modified,
                })
            })
            .collect(),
        Err(_) => return Vec::new(),
    };

    // Sort: directories first, then files; alphabetically within each group
    entries.sort_by(|a, b| {
        match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        }
    });

    entries
}

fn format_size(size: u64) -> String {
    if size < 1024 {
        format!("{} B", size)
    } else if size < 1024 * 1024 {
        format!("{:.1} K", size as f64 / 1024.0)
    } else if size < 1024 * 1024 * 1024 {
        format!("{:.1} M", size as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} G", size as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

fn format_permissions(mode: u32) -> String {
    let mut s = String::with_capacity(10);
    s.push(if mode & 0o40000 != 0 { 'd' } else { '-' });
    s.push(if mode & 0o400 != 0 { 'r' } else { '-' });
    s.push(if mode & 0o200 != 0 { 'w' } else { '-' });
    s.push(if mode & 0o100 != 0 { 'x' } else { '-' });
    s.push(if mode & 0o040 != 0 { 'r' } else { '-' });
    s.push(if mode & 0o020 != 0 { 'w' } else { '-' });
    s.push(if mode & 0o010 != 0 { 'x' } else { '-' });
    s.push(if mode & 0o004 != 0 { 'r' } else { '-' });
    s.push(if mode & 0o002 != 0 { 'w' } else { '-' });
    s.push(if mode & 0o001 != 0 { 'x' } else { '-' });
    s
}

fn entry_icon(is_dir: bool, name: &str) -> &'static str {
    if is_dir {
        return "📁";
    }
    match name.rsplit('.').next() {
        Some("rs") | Some("toml") | Some("json") | Some("yaml") | Some("yml") => "📄",
        Some("png") | Some("jpg") | Some("jpeg") | Some("svg") | Some("gif") => "🖼",
        Some("mp3") | Some("wav") | Some("flac") | Some("ogg") => "🎵",
        Some("mp4") | Some("mkv") | Some("avi") | Some("webm") => "🎬",
        Some("zip") | Some("tar") | Some("gz") | Some("bz2") | Some("xz") => "📦",
        Some("py") | Some("sh") | Some("bash") => "📜",
        _ => "📄",
    }
}

pub fn next_selection_index(state: &BrowseState, direction: BrowseNavigation) -> Option<usize> {
    let len = state.entries.len();
    if len == 0 {
        return None;
    }
    match (state.selected, direction) {
        (Some(index), BrowseNavigation::Up) => Some(index.saturating_sub(1)),
        (Some(index), BrowseNavigation::Down) => Some((index + 1).min(len - 1)),
        (None, BrowseNavigation::Up) => Some(len - 1),
        (None, BrowseNavigation::Down) => Some(0),
    }
}

fn row_top(index: usize) -> f32 {
    index as f32 * (LIST_ROW_HEIGHT + LIST_ROW_SPACING)
}

pub fn scroll_to_selection_if_needed(
    state: &BrowseState,
    index: usize,
) -> Task<Message> {
    let top = row_top(index);
    let bottom = top + LIST_ROW_HEIGHT;
    let viewport_top = state.scroll_offset_y;
    let viewport_bottom = viewport_top + state.viewport_height;

    if bottom <= viewport_top {
        operation::scroll_to(
            LIST_SCROLL_ID,
            scrollable::AbsoluteOffset { x: 0.0, y: top },
        )
    } else if top >= viewport_bottom {
        let new_offset = (bottom - state.viewport_height).max(0.0);
        operation::scroll_to(
            LIST_SCROLL_ID,
            scrollable::AbsoluteOffset { x: 0.0, y: new_offset },
        )
    } else {
        Task::none()
    }
}

// ── View ────────────────────────────────────────────────────────────

pub fn view(state: &BrowseState) -> Element<'_, Message> {
    let text_fg = Color::from_rgb8(0xd4, 0xd4, 0xd4);
    let text_dim = Color::from_rgb8(0x88, 0x88, 0x99);
    let selected_bg = Color::from_rgb8(0x2a, 0x4a, 0x2e);
    let row_bg = Color::from_rgb8(0x1e, 0x2e, 0x20);
    let dir_fg = Color::from_rgb8(0x8f, 0xd4, 0x8f);
    let accent = Color::from_rgb8(0x5c, 0x90, 0x60);

    // Breadcrumb path
    let path_str = state.current_dir.to_string_lossy().to_string();
    let breadcrumb = text(path_str).size(11).color(text_dim);

    let search_input = text_input("Search files...", &state.search)
        .on_input(|s| Message::Browse(BrowseMessage::SearchChanged(s)))
        .padding([8, 12]);

    let count_label = text(format!(
        "{} items{}",
        state.entries.len(),
        if state.show_hidden { " (.)" } else { "" }
    ))
    .size(11)
    .color(text_dim);

    let header = column![
        breadcrumb,
        row![search_input, count_label]
            .spacing(8)
            .align_y(iced::Alignment::Center),
    ]
    .spacing(6);

    let mut list = column![].spacing(2);

    for (idx, entry) in state.entries.iter().enumerate() {
        let is_selected = state.selected == Some(idx);
        let bg = if is_selected { selected_bg } else { row_bg };
        let fg = if is_selected {
            dir_fg
        } else if entry.is_dir {
            accent
        } else {
            text_fg
        };

        let icon = entry_icon(entry.is_dir, &entry.name);
        let size_str = if entry.is_dir {
            "—".to_string()
        } else {
            format_size(entry.size)
        };
        let perm_str = format_permissions(entry.permissions);

        let entry_row = button(
            row![]
                .spacing(8)
                .align_y(iced::Alignment::Center)
                .push(text(icon).size(13))
                .push(text(entry.name.clone()).size(13).color(fg).width(Length::Fill))
                .push(text(size_str).size(11).color(text_dim).width(70))
                .push(text(perm_str).size(11).color(text_dim).width(80))
                .push(text(entry.modified.clone()).size(11).color(text_dim).width(120)),
        )
        .style(move |_theme, _status| button::Style {
            background: Some(bg.into()),
            border: iced::Border {
                radius: 4.0.into(),
                ..iced::Border::default()
            },
            ..button::Style::default()
        })
        .padding([6, 10])
        .height(LIST_ROW_HEIGHT)
        .width(Length::Fill)
        .on_press(Message::Browse(BrowseMessage::SelectEntry(idx)));

        list = list.push(entry_row);
    }

    let scroll = scrollable(list)
        .id(LIST_SCROLL_ID)
        .height(Length::Fill)
        .on_scroll(|viewport| Message::Browse(BrowseMessage::Scrolled(viewport)));

    column![header, scroll]
        .spacing(8)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

// ── Update ──────────────────────────────────────────────────────────

fn apply_filters(state: &mut BrowseState) {
    state.entries = state
        .all_entries
        .iter()
        .filter(|e| state.show_hidden || !e.name.starts_with('.'))
        .filter(|e| {
            if state.search.is_empty() {
                return true;
            }
            let q = state.search.to_lowercase();
            e.name.to_lowercase().contains(&q)
        })
        .cloned()
        .collect();
    state.selected = if state.entries.is_empty() { None } else { Some(0) };
}

pub fn update(state: &mut BrowseState, msg: BrowseMessage) -> Task<Message> {
    match msg {
        BrowseMessage::SearchChanged(q) => {
            state.search = q;
            apply_filters(state);
            Task::none()
        }
        BrowseMessage::SelectEntry(i) => {
            state.selected = Some(i);
            Task::none()
        }
        BrowseMessage::NavigateTo(idx) => {
            if let Some(entry) = state.entries.get(idx) {
                if entry.is_dir {
                    let path = entry.path.clone();
                    return Task::perform(
                        async move { read_directory(&path) },
                        |entries| Message::Browse(BrowseMessage::DirectoryLoaded(entries)),
                    );
                }
            }
            Task::none()
        }
        BrowseMessage::NavigateToPath(path) => {
            let p = path.clone();
            Task::perform(
                async move { read_directory(&p) },
                |entries| Message::Browse(BrowseMessage::DirectoryLoaded(entries)),
            )
        }
        BrowseMessage::DirectoryLoaded(entries) => {
            state.all_entries = entries;
            state.search.clear();
            apply_filters(state);
            // Update current_dir from first entry's parent, or keep as-is
            if let Some(first) = state.all_entries.first() {
                if let Some(parent) = first.path.parent() {
                    state.current_dir = parent.to_path_buf();
                }
            }
            Task::none()
        }
        BrowseMessage::ToggleHidden => {
            state.show_hidden = !state.show_hidden;
            apply_filters(state);
            Task::none()
        }
        BrowseMessage::Scrolled(viewport) => {
            let offset = viewport.absolute_offset();
            state.scroll_offset_y = offset.y;
            state.viewport_height = viewport.bounds().height;
            Task::none()
        }
    }
}

// ── Subscription ────────────────────────────────────────────────────

pub fn subscription(_state: &BrowseState) -> iced::Subscription<Message> {
    iced::Subscription::none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with_count(count: usize) -> BrowseState {
        BrowseState {
            entries: (0..count)
                .map(|i| DirEntry {
                    name: format!("file_{i}"),
                    path: PathBuf::from(format!("/tmp/file_{i}")),
                    is_dir: i % 3 == 0,
                    size: 1024 * i as u64,
                    permissions: 0o644,
                    modified: String::new(),
                })
                .collect(),
            ..BrowseState::default()
        }
    }

    #[test]
    fn down_from_none_selects_first_entry() {
        let state = state_with_count(3);
        assert_eq!(next_selection_index(&state, BrowseNavigation::Down), Some(0));
    }

    #[test]
    fn up_from_none_selects_last_entry() {
        let state = state_with_count(3);
        assert_eq!(next_selection_index(&state, BrowseNavigation::Up), Some(2));
    }

    #[test]
    fn navigation_stays_in_bounds() {
        let mut state = state_with_count(3);

        state.selected = Some(0);
        assert_eq!(next_selection_index(&state, BrowseNavigation::Up), Some(0));

        state.selected = Some(2);
        assert_eq!(next_selection_index(&state, BrowseNavigation::Down), Some(2));
    }

    #[test]
    fn format_size_units() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1024), "1.0 K");
        assert_eq!(format_size(1048576), "1.0 M");
        assert_eq!(format_size(1073741824), "1.0 G");
    }

    #[test]
    fn format_permissions_string() {
        assert_eq!(format_permissions(0o40755), "drwxr-xr-x");
        assert_eq!(format_permissions(0o100644), "-rw-r--r--");
        assert_eq!(format_permissions(0o644), "-rw-r--r--");
    }

    #[test]
    fn apply_filters_hides_dotfiles() {
        let mut state = BrowseState::default();
        state.all_entries = vec![
            DirEntry { name: ".hidden".into(), path: PathBuf::from("/a/.hidden"), is_dir: false, size: 0, permissions: 0o644, modified: String::new() },
            DirEntry { name: "visible".into(), path: PathBuf::from("/a/visible"), is_dir: false, size: 0, permissions: 0o644, modified: String::new() },
        ];
        state.show_hidden = false;
        apply_filters(&mut state);
        assert_eq!(state.entries.len(), 1);
        assert_eq!(state.entries[0].name, "visible");
    }

    #[test]
    fn apply_filters_shows_dotfiles_when_enabled() {
        let mut state = BrowseState::default();
        state.all_entries = vec![
            DirEntry { name: ".hidden".into(), path: PathBuf::from("/a/.hidden"), is_dir: false, size: 0, permissions: 0o644, modified: String::new() },
            DirEntry { name: "visible".into(), path: PathBuf::from("/a/visible"), is_dir: false, size: 0, permissions: 0o644, modified: String::new() },
        ];
        state.show_hidden = true;
        apply_filters(&mut state);
        assert_eq!(state.entries.len(), 2);
    }
}
