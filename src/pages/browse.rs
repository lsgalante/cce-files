use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::pages::PageContent;
use clear_ui::widget::{Element, Breadcrumb, PathController};
use clear_ui::layout::SectionContext;

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
    pub search_box: clear_ui::widget::TextBox,
    pub list_box: clear_ui::widget::ScrollingList,
    pub selected: Option<usize>,
    pub breadcrumb: Breadcrumb,
    pub save_name_box: clear_ui::widget::TextBox,
}

impl Default for BrowseState {
    fn default() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
        let mut breadcrumb = Breadcrumb::new();
        breadcrumb.set_network_opacity(0.95);
        let mut state = Self {
            current_dir: PathBuf::from(home),
            all_entries: Vec::new(),
            entries: Vec::new(),
            show_hidden: false,
            search_box: clear_ui::widget::TextBox::new(String::new()).with_max_width(None),
            list_box: clear_ui::widget::ScrollingList::new(28.0, 2.0),
            selected: None,
            breadcrumb,
            save_name_box: clear_ui::widget::TextBox::new(String::new()).with_max_width(None),
        };
        state.list_box.scroll_box.show_border = false;
        state.update_breadcrumb();
        state
    }
}

impl BrowseState {
    pub fn update_breadcrumb(&mut self) {
        let mut segments = Vec::new();
        for component in self.current_dir.components() {
            let s = component.as_os_str().to_string_lossy().to_string();
            if s != "/" && !s.is_empty() {
                segments.push(s);
            }
        }
        self.breadcrumb.set_path(&segments);
    }
}

#[derive(Debug, Clone)]
pub enum BrowseMessage {
    SearchChanged(String),
    SelectEntry(usize),
    NavigateTo(usize),
    NavigateToPath(PathBuf),
    DirectoryLoaded(PathBuf, Vec<DirEntry>),
    ToggleHidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowseNavigation {
    Up,
    Down,
}

pub fn is_project_dir(path: &Path) -> bool {
    path.is_dir() && path.join("state.json").exists()
}

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

// ── View ────────────────────────────────────────────────────────────

pub fn view(state: &mut BrowseState, cx: f32, cy: f32, cw: f32, ch: f32, select_mode: bool, ctx: &mut clear_ui::context::UiContext) -> PageContent {
    let mut pc = PageContent::new();
    let text_dim = [0.53, 0.53, 0.60, 1.0];
    let heading_fg = [0.56, 0.83, 0.56, 1.0]; // Color::from_rgb8(0x8f, 0xd4, 0x8f)
    let selected_bg = [0.16, 0.29, 0.18, 0.8]; // Color::from_rgb8(0x2a, 0x4a, 0x2e)
    let row_bg = [0.12, 0.18, 0.13, 0.6]; // Color::from_rgb8(0x1e, 0x2e, 0x20)
    let accent_fg = [0.36, 0.56, 0.38, 1.0]; // Color::from_rgb8(0x5c, 0x90, 0x60)
    let text_fg = [0.83, 0.83, 0.83, 1.0];

    // Render the Breadcrumb widget into PageContent
    clear_ui::layout::render_widget(&mut pc, &mut state.breadcrumb, cx + 12.0, cy + 6.0, cw - 24.0, 24.0, ctx);

    // 1. Files Section layout
    let outer_x = cx + 12.0;
    let outer_y = cy + 42.0;
    let outer_w = cw - 24.0;

    // Search Section starts at the bottom
    let search_sec_h = 56.0;
    let search_sec_y = cy + ch - search_sec_h;

    // Files Section height takes the remaining space above Search Section
    let outer_h = search_sec_y - outer_y - 12.0;

    let mut files_sec = SectionContext::new(&mut pc, outer_x - 8.0, outer_y - 12.0, outer_w + 16.0, "Files", false, false);
    files_sec.content_y = outer_y + outer_h - 12.0;
    files_sec.finish();

    // Inner file list scroll box (inset inside the Files Section)
    let list_x = outer_x + 4.0;
    let list_y = outer_y + 4.0;
    let list_w = outer_w - 8.0;
    let list_h = outer_h - 8.0;

    clear_ui::layout::render_widget(&mut pc, &mut state.list_box, list_x, list_y, list_w, list_h, ctx);

    state.list_box.update_bounds(state.entries.len(), list_y, list_h);

    // Count label in the bottom right corner of the scrolling list
    let count_str = format!(
        "{} items{}",
        state.entries.len(),
        if state.show_hidden { " (.)" } else { "" }
    );
    let count_text_w = count_str.len() as f32 * 6.0;
    let count_x = list_x + list_w - count_text_w - 24.0;
    let count_y = list_y + list_h - 18.0;
    pc.text(&count_str, count_x, count_y, 11.0, text_dim);

    if select_mode {
        // Two columns at the bottom: Search and File Name
        let sec_w = (outer_w - 12.0) / 2.0;

        // Left column: Search
        let mut search_sec = SectionContext::new(&mut pc, outer_x - 8.0, search_sec_y - 12.0, sec_w + 16.0, "Search", false, false);
        search_sec.content_y = search_sec_y + search_sec_h - 12.0;
        search_sec.finish();

        let search_x = outer_x + 12.0;
        let search_y = search_sec_y + 14.0;
        let search_w = sec_w - 24.0;
        let search_h = 28.0;

        state.search_box.set_row_rect(search_x, search_w);
        clear_ui::layout::render_widget(&mut pc, &mut state.search_box, search_x, search_y, search_w, search_h, ctx);

        // Right column: File Name
        let filename_sec_x = outer_x + sec_w + 12.0;
        let mut filename_sec = SectionContext::new(&mut pc, filename_sec_x - 8.0, search_sec_y - 12.0, sec_w + 16.0, "File Name", false, false);
        filename_sec.content_y = search_sec_y + search_sec_h - 12.0;
        filename_sec.finish();

        let filename_x = filename_sec_x + 12.0;
        let filename_y = search_sec_y + 14.0;
        let filename_w = sec_w - 24.0;
        let filename_h = 28.0;

        state.save_name_box.set_row_rect(filename_x, filename_w);
        clear_ui::layout::render_widget(&mut pc, &mut state.save_name_box, filename_x, filename_y, filename_w, filename_h, ctx);
    } else {
        // 2. Search Section borders (Full Width)
        let mut search_sec = SectionContext::new(&mut pc, outer_x - 8.0, search_sec_y - 12.0, outer_w + 16.0, "Search", false, false);
        search_sec.content_y = search_sec_y + search_sec_h - 12.0;
        search_sec.finish();

        // Inner search textbox
        let search_x = outer_x + 12.0;
        let search_y = search_sec_y + 14.0;
        let search_w = outer_w - 24.0;
        let search_h = 28.0;

        state.search_box.set_row_rect(search_x, search_w);
        clear_ui::layout::render_widget(&mut pc, &mut state.search_box, search_x, search_y, search_w, search_h, ctx);
    }

    for (idx, entry) in state.entries.iter().enumerate() {
        if let Some(draw_y) = state.list_box.get_item_draw_y(idx, 2.0) {
            let is_selected = state.selected == Some(idx);
            let bg = if is_selected { selected_bg } else { row_bg };
            let fg = if is_selected {
                heading_fg
            } else if entry.is_dir {
                accent_fg
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

            // Determine click action
            let action = if is_selected {
                crate::Message::Browse(BrowseMessage::NavigateTo(idx))
            } else {
                crate::Message::Browse(BrowseMessage::SelectEntry(idx))
            };

            // Button for row selection/navigation
            pc.button(
                "",
                list_x + 4.0,
                draw_y,
                list_w - 24.0,
                28.0,
                bg,
                if is_selected { selected_bg } else { [0.22, 0.32, 0.24, 0.8] },
                fg,
                action,
            );

            // Draw contents inside the button boundary:
            pc.text(icon, list_x + 12.0, draw_y + 7.0, 13.0, fg);
            
            let show_size = list_w > 400.0;
            let show_perm = list_w > 480.0;
            let show_modified = list_w > 280.0;

            let next_col_x = if show_size {
                list_w - 290.0
            } else if show_modified {
                list_w - 120.0
            } else {
                list_w - 20.0
            };

            let max_chars = ((next_col_x - 40.0) / 8.0).max(10.0) as usize;
            let name_truncated = if entry.name.chars().count() > max_chars {
                if max_chars > 3 {
                    let mut s: String = entry.name.chars().take(max_chars - 3).collect();
                    s.push_str("...");
                    s
                } else {
                    entry.name.clone()
                }
            } else {
                entry.name.clone()
            };

            pc.text(&name_truncated, list_x + 32.0, draw_y + 7.0, 13.0, fg);
            if show_size {
                pc.text(&size_str, list_x + list_w - 290.0, draw_y + 8.0, 11.0, text_dim);
            }
            if show_perm {
                pc.text(&perm_str, list_x + list_w - 210.0, draw_y + 8.0, 11.0, text_dim);
            }
            if show_modified {
                pc.text(&entry.modified, list_x + list_w - 120.0, draw_y + 8.0, 11.0, text_dim);
            }
        }
    }

    pc
}

// ── Update ──────────────────────────────────────────────────────────

fn apply_filters(state: &mut BrowseState) {
    let search_query = if state.search_box.editing {
        state.search_box.edit_buffer.to_lowercase()
    } else {
        state.search_box.text.to_lowercase()
    };
    state.entries = state
        .all_entries
        .iter()
        .filter(|e| state.show_hidden || !e.name.starts_with('.'))
        .filter(|e| {
            if search_query.is_empty() {
                return true;
            }
            e.name.to_lowercase().contains(&search_query)
        })
        .cloned()
        .collect();
    state.selected = if state.entries.is_empty() { None } else { Some(0) };
}

pub fn update(state: &mut BrowseState, msg: BrowseMessage) -> (PathBuf, tokio::task::JoinHandle<Vec<DirEntry>>) {
    match msg {
        BrowseMessage::SearchChanged(q) => {
            state.search_box.text = q;
            apply_filters(state);
            (state.current_dir.clone(), tokio::spawn(async { Vec::new() }))
        }
        BrowseMessage::SelectEntry(i) => {
            state.selected = Some(i);
            (state.current_dir.clone(), tokio::spawn(async { Vec::new() }))
        }
        BrowseMessage::NavigateTo(idx) => {
            if let Some(entry) = state.entries.get(idx) {
                if entry.is_dir && !is_project_dir(&entry.path) {
                    let path = entry.path.clone();
                    let p = path.clone();
                    return (path, tokio::spawn(async move { read_directory(&p) }));
                }
            }
            (state.current_dir.clone(), tokio::spawn(async { Vec::new() }))
        }
        BrowseMessage::NavigateToPath(path) => {
            let p = path.clone();
            (path, tokio::spawn(async move { read_directory(&p) }))
        }
        BrowseMessage::DirectoryLoaded(path, entries) => {
            state.current_dir = path;
            state.all_entries = entries;
            state.search_box.text.clear();
            state.search_box.edit_buffer.clear();
            apply_filters(state);
            state.update_breadcrumb();
            (state.current_dir.clone(), tokio::spawn(async { Vec::new() }))
        }
        BrowseMessage::ToggleHidden => {
            state.show_hidden = !state.show_hidden;
            apply_filters(state);
            (state.current_dir.clone(), tokio::spawn(async { Vec::new() }))
        }
    }
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
            search_box: clear_ui::widget::TextBox::new(String::new()).with_max_width(None),
            list_box: clear_ui::widget::ScrollingList::new(28.0, 2.0),
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

    #[test]
    fn test_is_project_dir_detection() {
        let unique_dir = std::env::temp_dir().join(format!("clear_test_dir_{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)));
        std::fs::create_dir_all(&unique_dir).unwrap();
        
        // Initially, path is a directory but doesn't have state.json
        assert!(!is_project_dir(&unique_dir));
        
        // Create state.json
        let file_path = unique_dir.join("state.json");
        std::fs::write(&file_path, "{}").unwrap();
        
        // Now it should be recognized as a project dir
        assert!(is_project_dir(&unique_dir));
        
        // If it's a file rather than a directory, even if named state.json, it shouldn't be a project dir itself
        assert!(!is_project_dir(&file_path));

        // Clean up
        let _ = std::fs::remove_file(&file_path);
        let _ = std::fs::remove_dir(&unique_dir);
    }
}
