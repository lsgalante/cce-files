use std::path::{Path, PathBuf};

use crate::pages::PageContent;
use cce_ui::widget::{Element, Breadcrumb, PathController};
use cce_ui::layout::{ColumnLayout, LayoutStrategy};

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
    pub search_box: cce_ui::widget::TextBox,
    pub list_box: cce_ui::widget::List,
    pub selected: Option<usize>,
    pub breadcrumb: Breadcrumb,
    pub save_name_box: cce_ui::widget::TextBox,
}

impl Default for BrowseState {
    fn default() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
        let initial_dir = PathBuf::from(home);
        let mut breadcrumb = Breadcrumb::new();
        breadcrumb.set_network_opacity(0.95);
        let mut state = Self {
            current_dir: initial_dir,
            all_entries: Vec::new(),
            entries: Vec::new(),
            show_hidden: false,
            search_box: cce_ui::widget::TextBox::new(String::new())
                .with_max_width(None)
                .with_placeholder("Search"),
            list_box: cce_ui::widget::List::new(cce_ui::layout::button_height(), 2.0),
            selected: None,
            breadcrumb,
            save_name_box: cce_ui::widget::TextBox::new(String::new()).with_max_width(None),
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
    DirectoryRefreshed(PathBuf, Vec<DirEntry>),
    ToggleHidden,
    DeleteEntry(usize),
    Deleted(PathBuf, Result<(), String>),
    LastDirLoaded(Option<PathBuf>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowseNavigation {
    Up,
    Down,
}

pub fn is_project_dir(path: &Path) -> bool {
    path.is_dir() && (path.join("state.json").exists() || path.join("state.kdl").exists())
}

// ── Helpers ─────────────────────────────────────────────────────────

pub fn read_directory(path: &Path) -> Vec<DirEntry> {
    crate::services::fs::read_directory_internal(path)
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

pub fn view(state: &mut BrowseState, cx: f32, cy: f32, cw: f32, ch: f32, select_mode: bool, ctx: &mut cce_ui::context::UiContext) -> PageContent {
    let mut pc = PageContent::new();
    let text_dim = cce_ui::color::TEXT_DIM;
    let heading_fg = cce_ui::color::TEXT_HEADER;
    let text_fg = cce_ui::color::list_font_color();
    let accent_fg = cce_ui::color::TEXT_ACCENT;

    let selected_bg = cce_ui::color::list_entry_highlight_color();
    let row_bg = cce_ui::color::list_entry_bg_color();

    let gap = 12.0;
    let margin = 12.0;
    let mut layout = ColumnLayout::new(gap);
    let client_x = cx + margin;
    let client_y = cy + margin;
    let client_w = cw - 2.0 * margin;
    let client_h = ch - 2.0 * margin;
    layout.init(client_x, client_y, client_w, client_h);

    let breadcrumb_h = 24.0;
    let textbox_h = cce_ui::layout::textbox_height();

    // 1. Allocate and render Breadcrumb
    let (bx, by, bw, bh) = layout.allocate(client_w, breadcrumb_h);
    cce_ui::layout::render_widget(&mut pc, &mut state.breadcrumb, bx, by, bw, bh, ctx);

    // 2. Allocate and render List (ScrollBox)
    // The scrolling list height occupies the remaining vertical space:
    // list_h = client_h - breadcrumb_h - textbox_h - (2 * gap)
    let list_h_val = client_h - breadcrumb_h - textbox_h - 2.0 * gap;
    let (list_x, list_y, list_w, list_h) = layout.allocate(client_w, list_h_val);
    cce_ui::layout::render_widget(&mut pc, &mut state.list_box, list_x, list_y, list_w, list_h, ctx);

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

    // 3. Allocate and render Textbox(es)
    let (tx, ty, tw, th) = layout.allocate(client_w, textbox_h);

    if select_mode {
        // Two columns at the bottom: Search and File Name
        let sec_w = (tw - 12.0) / 2.0;

        // Left column: Search
        let search_x = tx;
        let search_w = sec_w;
        state.search_box.set_row_rect(search_x, search_w);
        cce_ui::layout::render_widget(&mut pc, &mut state.search_box, search_x, ty, search_w, th, ctx);

        // Right column: File Name
        let filename_x = tx + sec_w + 12.0;
        let filename_w = sec_w;
        state.save_name_box.set_row_rect(filename_x, filename_w);
        cce_ui::layout::render_widget(&mut pc, &mut state.save_name_box, filename_x, ty, filename_w, th, ctx);
    } else {
        // Full width search textbox
        state.search_box.set_row_rect(tx, tw);
        cce_ui::layout::render_widget(&mut pc, &mut state.search_box, tx, ty, tw, th, ctx);
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

            let hover_bg = if is_selected {
                selected_bg
            } else {
                let mut h_bg = cce_ui::color::highlight_primary_color();
                h_bg[3] = 0.25;
                h_bg
            };

            // Button for row selection/navigation
            pc.button(
                "",
                list_x + 4.0,
                draw_y,
                list_w - 24.0,
                cce_ui::layout::button_height(),
                bg,
                hover_bg,
                fg,
                action,
            );

            // Draw contents inside the button boundary:
            let row_h = cce_ui::layout::button_height();
            let y_text_13 = cce_ui::layout::center_text_y(draw_y, row_h, 13.0);
            let y_text_11 = cce_ui::layout::center_text_y(draw_y, row_h, 11.0);

            pc.text(icon, list_x + 12.0, y_text_13, 13.0, fg);
            
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

            pc.text(&name_truncated, list_x + 32.0, y_text_13, 13.0, fg);
            if show_size {
                pc.text(&size_str, list_x + list_w - 290.0, y_text_11, 11.0, text_dim);
            }
            if show_perm {
                pc.text(&perm_str, list_x + list_w - 210.0, y_text_11, 11.0, text_dim);
            }
            if show_modified {
                pc.text(&entry.modified, list_x + list_w - 120.0, y_text_11, 11.0, text_dim);
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

pub fn update(state: &mut BrowseState, msg: BrowseMessage) -> Option<crate::services::fs::FsRequest> {
    match msg {
        BrowseMessage::SearchChanged(q) => {
            state.search_box.text = q;
            apply_filters(state);
            None
        }
        BrowseMessage::SelectEntry(i) => {
            state.selected = Some(i);
            None
        }
        BrowseMessage::NavigateTo(idx) => {
            if let Some(entry) = state.entries.get(idx) {
                if entry.is_dir && !is_project_dir(&entry.path) {
                    return Some(crate::services::fs::FsRequest::ReadDirectory(entry.path.clone()));
                }
            }
            None
        }
        BrowseMessage::NavigateToPath(path) => {
            Some(crate::services::fs::FsRequest::ReadDirectory(path))
        }
        BrowseMessage::DirectoryLoaded(path, entries) => {
            state.current_dir = path.clone();
            state.all_entries = entries;
            state.search_box.text.clear();
            state.search_box.edit_buffer.clear();
            apply_filters(state);
            state.update_breadcrumb();
            Some(crate::services::fs::FsRequest::SaveLastDir(path))
        }
        BrowseMessage::DirectoryRefreshed(path, entries) => {
            if state.current_dir == path {
                let selected_path = state.selected.and_then(|idx| state.entries.get(idx).map(|e| e.path.clone()));
                state.all_entries = entries;
                apply_filters(state);
                if let Some(path) = selected_path {
                    state.selected = state.entries.iter().position(|e| e.path == path);
                }
            }
            None
        }
        BrowseMessage::ToggleHidden => {
            state.show_hidden = !state.show_hidden;
            apply_filters(state);
            None
        }
        BrowseMessage::DeleteEntry(idx) => {
            if let Some(entry) = state.entries.get(idx) {
                Some(crate::services::fs::FsRequest::DeletePath(entry.path.clone(), entry.is_dir))
            } else {
                None
            }
        }
        BrowseMessage::Deleted(path, result) => {
            match result {
                Ok(_) => {
                    state.all_entries.retain(|e| e.path != path);
                    state.entries.retain(|e| e.path != path);
                    if state.entries.is_empty() {
                        state.selected = None;
                    } else if let Some(idx) = state.selected {
                        state.selected = Some(idx.min(state.entries.len() - 1));
                    }
                }
                Err(e) => {
                    log::error!("Failed to delete {}: {}", path.display(), e);
                }
            }
            Some(crate::services::fs::FsRequest::ReadDirectory(state.current_dir.clone()))
        }
        BrowseMessage::LastDirLoaded(last_dir) => {
            let path = last_dir.unwrap_or_else(|| {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
                PathBuf::from(home)
            });
            Some(crate::services::fs::FsRequest::ReadDirectory(path))
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
            search_box: cce_ui::widget::TextBox::new(String::new()).with_max_width(None),
            list_box: cce_ui::widget::List::new(cce_ui::layout::button_height(), 2.0),
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
        
        // Initially, path is a directory but doesn't have state.json or state.kdl
        assert!(!is_project_dir(&unique_dir));
        
        // Create state.json
        let file_path_json = unique_dir.join("state.json");
        std::fs::write(&file_path_json, "{}").unwrap();
        
        // Now it should be recognized as a project dir
        assert!(is_project_dir(&unique_dir));
        
        // Remove state.json and verify it's not a project dir
        std::fs::remove_file(&file_path_json).unwrap();
        assert!(!is_project_dir(&unique_dir));

        // Create state.kdl
        let file_path_kdl = unique_dir.join("state.kdl");
        std::fs::write(&file_path_kdl, "name \"test\"").unwrap();

        // Now it should be recognized as a project dir
        assert!(is_project_dir(&unique_dir));

        // If it's a file rather than a directory, even if named state.kdl, it shouldn't be a project dir itself
        assert!(!is_project_dir(&file_path_kdl));

        // Clean up
        let _ = std::fs::remove_file(&file_path_kdl);
        let _ = std::fs::remove_dir(&unique_dir);
    }

    #[test]
    fn test_directory_persistence() {
        let temp_dir = std::env::temp_dir();
        let original_home = std::env::var("HOME");
        let original_xdg = std::env::var("XDG_CONFIG_HOME");
        
        // Mock HOME env variable so we don't overwrite user's actual config
        let mock_home = temp_dir.join("mock_home_dir_cce");
        let _ = std::fs::create_dir_all(&mock_home);
        unsafe {
            std::env::set_var("HOME", &mock_home);
            std::env::remove_var("XDG_CONFIG_HOME");
        }
        
        let test_dir = temp_dir.join("test_persist_dir");
        let _ = std::fs::create_dir_all(&test_dir);
        
        // Save last directory
        crate::services::fs::save_last_dir_internal(&test_dir);
        
        // Read last directory
        let restored = crate::services::fs::read_last_dir_internal();
        assert_eq!(restored, Some(test_dir.clone()));
        
        // Restore HOME env var
        if let Ok(val) = original_home {
            unsafe { std::env::set_var("HOME", val); }
        } else {
            unsafe { std::env::remove_var("HOME"); }
        }
        
        // Restore XDG_CONFIG_HOME
        if let Ok(val) = original_xdg {
            unsafe { std::env::set_var("XDG_CONFIG_HOME", val); }
        } else {
            unsafe { std::env::remove_var("XDG_CONFIG_HOME"); }
        }
        
        // Clean up
        let _ = std::fs::remove_dir_all(&mock_home);
        let _ = std::fs::remove_dir(&test_dir);
    }

    #[tokio::test]
    async fn test_delete_entry() {
        let temp_dir = std::env::temp_dir();
        let test_subdir = temp_dir.join(format!("cce_test_delete_{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)));
        std::fs::create_dir_all(&test_subdir).unwrap();

        let file_path = test_subdir.join("delete_me.txt");
        std::fs::write(&file_path, "test delete content").unwrap();

        let mut state = BrowseState {
            current_dir: test_subdir.clone(),
            all_entries: vec![
                DirEntry {
                    name: "delete_me.txt".to_string(),
                    path: file_path.clone(),
                    is_dir: false,

                    size: 19,
                    permissions: 0o644,
                    modified: String::new(),
                }
            ],
            entries: vec![
                DirEntry {
                    name: "delete_me.txt".to_string(),
                    path: file_path.clone(),
                    is_dir: false,
                    size: 19,
                    permissions: 0o644,
                    modified: String::new(),
                }
            ],
            selected: Some(0),
            ..BrowseState::default()
        };

        // Assert file exists before deletion
        assert!(file_path.exists());

        // Perform update call for DeleteEntry
        let req = update(&mut state, BrowseMessage::DeleteEntry(0));
        assert!(matches!(req, Some(crate::services::fs::FsRequest::DeletePath(_, _))));

        // Directly delete the file to simulate the FsService action
        std::fs::remove_file(&file_path).unwrap();
        assert!(!file_path.exists());

        // Perform update call for Deleted response
        let req2 = update(&mut state, BrowseMessage::Deleted(file_path.clone(), Ok(())));
        assert!(matches!(req2, Some(crate::services::fs::FsRequest::ReadDirectory(_))));

        // Check if state entries are updated
        assert!(state.entries.is_empty());
        assert!(state.all_entries.is_empty());
        assert_eq!(state.selected, None);

        // Clean up directory
        let _ = std::fs::remove_dir_all(&test_subdir);
    }

    #[test]
    fn test_component_reconstruction() {
        let current_dir = PathBuf::from("/home/lsgalante/documents");
        
        // Let's say seg is 1 (meaning "home/")
        let seg = 1;
        let mut target_path = std::path::PathBuf::new();
        let mut current_idx = 0;
        for component in current_dir.components() {
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
        assert_eq!(target_path, PathBuf::from("/home"));
        
        // Let's say seg is 0 (meaning "/")
        let seg = 0;
        let mut target_path = std::path::PathBuf::new();
        let mut current_idx = 0;
        for component in current_dir.components() {
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
        assert_eq!(target_path, PathBuf::from("/"));
    }
}
