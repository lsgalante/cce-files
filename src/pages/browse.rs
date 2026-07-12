use std::path::{Path, PathBuf};

use crate::pages::PageContent;
use cce_ui::widget::{Adapted, WidgetHost, Breadcrumb, PathController};
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
    pub list: crate::row_list::RowList,
    pub search_visible: bool,
    pub search_box: cce_ui::widget::Adapted<cce_ui::widget::TextBox>,
    pub selected: Option<usize>,
    pub breadcrumb: Adapted<Breadcrumb>,
    pub save_name_box: cce_ui::widget::Adapted<cce_ui::widget::TextBox>,
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
            list: crate::row_list::RowList::new(cce_ui::layout::button_height(), 2.0),
            search_visible: false,
            search_box: cce_ui::widget::TextBox::new(String::new())
                .with_placeholder("Search...")
                .with_update_on_type(true),
            selected: None,
            breadcrumb,
            save_name_box: cce_ui::widget::TextBox::new(String::new()).with_max_width(None),
        };
        state.update_breadcrumb();
        state
    }
}

impl BrowseState {
    /// Path of the currently selected entry, if any.
    pub fn selected_path(&self) -> Option<PathBuf> {
        self.selected.and_then(|idx| self.entries.get(idx).map(|e| e.path.clone()))
    }

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

/// Reconstruct the ancestor path for a clicked breadcrumb segment index.
/// `seg == 0` is the root `/`; each subsequent index adds one path component.
pub fn path_to_segment(current_dir: &Path, seg: usize) -> PathBuf {
    let mut target_path = PathBuf::new();
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
    target_path
}

// ── Helpers ─────────────────────────────────────────────────────────

pub fn read_directory(path: &Path) -> Vec<DirEntry> {
    crate::services::fs::read_directory_internal(path)
}

use crate::util::{format_size, format_permissions};

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

pub fn view(state: &mut BrowseState, view_dropdown: &mut cce_ui::widget::Adapted<cce_ui::widget::Dropdown>, cx: f32, cy: f32, cw: f32, ch: f32, select_mode: bool, ctx: &mut cce_ui::context::UiContext) -> PageContent {
    let mut pc = PageContent::new();
    let text_dim = cce_ui::color::TEXT_DIM;


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

    // 1. Allocate and render Breadcrumb and Dropdown next to it
    let dropdown_w = 120.0;
    let breadcrumb_w = client_w - dropdown_w - gap;
    let (bx, by, bw, bh) = layout.allocate(breadcrumb_w, breadcrumb_h);
    cce_ui::layout::render_widget(&mut pc, &mut state.breadcrumb, bx, by, bw, bh, ctx);
    cce_ui::layout::render_widget(&mut pc, view_dropdown, bx + bw + gap, by, dropdown_w, breadcrumb_h, ctx);

    // 2. Allocate and render List (ScrollBox)
    // The scrolling list height occupies the remaining vertical space:
    let list_h_val = if select_mode {
        client_h - breadcrumb_h - textbox_h - 2.0 * gap
    } else {
        client_h - breadcrumb_h - gap
    };
    let (list_x, list_y, list_w, list_h) = layout.allocate(client_w, list_h_val);

    // Update List columns dynamically based on list width
    let show_size = list_w > 400.0;
    let show_perm = list_w > 480.0;
    let show_modified = list_w > 280.0;

    let mut cols = vec![
        crate::row_list::ListColumn {
            name: "Name".to_string(),
            width: crate::row_list::ColumnWidth::Flex,
            justification: cce_ui::widget::Justification::Left,
        }
    ];
    if show_size {
        cols.push(crate::row_list::ListColumn {
            name: "Size".to_string(),
            width: crate::row_list::ColumnWidth::RightOffset(290.0),
            justification: cce_ui::widget::Justification::Left,
        });
    }
    if show_perm {
        cols.push(crate::row_list::ListColumn {
            name: "Permissions".to_string(),
            width: crate::row_list::ColumnWidth::RightOffset(210.0),
            justification: cce_ui::widget::Justification::Left,
        });
    }
    if show_modified {
        cols.push(crate::row_list::ListColumn {
            name: "Modified".to_string(),
            width: crate::row_list::ColumnWidth::RightOffset(120.0),
            justification: cce_ui::widget::Justification::Left,
        });
    }
    state.list.columns = cols;

    // Populate rows
    state.list.rows = state.entries.iter().enumerate().map(|(idx, entry)| {
        let size_str = if entry.is_dir {
            "—".to_string()
        } else {
            format_size(entry.size)
        };
        let perm_str = format_permissions(entry.permissions);

        let mut cells = vec![entry.name.clone()];
        if show_size {
            cells.push(size_str);
        }
        if show_perm {
            cells.push(perm_str);
        }
        if show_modified {
            cells.push(entry.modified.clone());
        }

        crate::row_list::Row {
            cells,
            icon: Some(entry_icon(entry.is_dir, &entry.name).to_string()),
            selected: state.selected == Some(idx),
        }
    }).collect();

    // Dissolved List (Phase 6z): scroll state, rows, and frame prims are app-owned.
    // When the search strip is open it reserves the bottom of the frame, exactly as
    // the legacy List::set_rect carved its scroll frame.
    let search_h = 26.0;
    let search_margin_y = 6.0;
    let search_offset = if state.search_visible { search_h + 2.0 * search_margin_y } else { 0.0 };
    state.list.set_rect(list_x, list_y, list_w, list_h, search_offset);
    state.list.update_bounds_from_rows();

    // Auto-scroll to keep selection in view
    if let Some(selected_idx) = state.selected {
        state.list.scroll_into_view(selected_idx);
    }

    state.list.push_prims(&mut pc);
    if state.search_visible {
        cce_ui::layout::render_widget(
            &mut pc,
            &mut state.search_box,
            list_x + 8.0,
            list_y + list_h - search_offset + search_margin_y,
            list_w - 16.0,
            search_h,
            ctx,
        );
    }

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
    if select_mode {
        let (tx, ty, tw, th) = layout.allocate(client_w, textbox_h);
        state.save_name_box.set_row_rect(tx, tw);
        cce_ui::layout::render_widget(&mut pc, &mut state.save_name_box, tx, ty, tw, th, ctx);
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
            state.search_visible = false;
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

    fn entry(name: &str, path: &str, is_dir: bool) -> DirEntry {
        DirEntry {
            name: name.to_string(),
            path: PathBuf::from(path),
            is_dir,
            size: 0,
            permissions: 0o644,
            modified: String::new(),
        }
    }

    #[test]
    fn search_changed_filters_entries() {
        let mut state = BrowseState::default();
        state.all_entries = vec![entry("apple", "/a/apple", false), entry("banana", "/a/banana", false)];
        let req = update(&mut state, BrowseMessage::SearchChanged("ban".to_string()));
        assert!(req.is_none());
        assert_eq!(state.entries.len(), 1);
        assert_eq!(state.entries[0].name, "banana");
        assert_eq!(state.selected, Some(0));
    }

    #[test]
    fn navigate_to_path_reads_directory() {
        let mut state = BrowseState::default();
        let req = update(&mut state, BrowseMessage::NavigateToPath(PathBuf::from("/tmp")));
        assert!(matches!(req, Some(crate::services::fs::FsRequest::ReadDirectory(p)) if p == PathBuf::from("/tmp")));
    }

    #[test]
    fn navigate_to_plain_directory_reads_it() {
        let dir = std::env::temp_dir().join(format!("cce_nav_{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)));
        std::fs::create_dir_all(&dir).unwrap();
        let mut state = BrowseState::default();
        state.entries = vec![entry("sub", dir.to_str().unwrap(), true)];
        let req = update(&mut state, BrowseMessage::NavigateTo(0));
        assert!(matches!(req, Some(crate::services::fs::FsRequest::ReadDirectory(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn navigate_to_project_directory_does_not_enter() {
        let dir = std::env::temp_dir().join(format!("cce_proj_{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("state.json"), "{}").unwrap();
        let mut state = BrowseState::default();
        state.entries = vec![entry("proj", dir.to_str().unwrap(), true)];
        let req = update(&mut state, BrowseMessage::NavigateTo(0));
        assert!(req.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn navigate_to_file_does_nothing() {
        let mut state = BrowseState::default();
        state.entries = vec![entry("f.txt", "/a/f.txt", false)];
        let req = update(&mut state, BrowseMessage::NavigateTo(0));
        assert!(req.is_none());
    }

    #[test]
    fn directory_loaded_populates_and_saves() {
        let mut state = BrowseState::default();
        let entries = vec![entry("x", "/d/x", false), entry("y", "/d/y", true)];
        let req = update(&mut state, BrowseMessage::DirectoryLoaded(PathBuf::from("/d"), entries));
        assert_eq!(state.current_dir, PathBuf::from("/d"));
        assert_eq!(state.entries.len(), 2);
        assert!(matches!(req, Some(crate::services::fs::FsRequest::SaveLastDir(p)) if p == PathBuf::from("/d")));
    }

    #[test]
    fn directory_refreshed_preserves_selection_by_path() {
        let mut state = BrowseState::default();
        state.current_dir = PathBuf::from("/d");
        state.all_entries = vec![entry("a", "/d/a", false), entry("b", "/d/b", false)];
        apply_filters(&mut state);
        state.selected = Some(1); // "b"
        let new_entries = vec![entry("b", "/d/b", false), entry("a", "/d/a", false), entry("c", "/d/c", false)];
        let req = update(&mut state, BrowseMessage::DirectoryRefreshed(PathBuf::from("/d"), new_entries));
        assert!(req.is_none());
        assert_eq!(
            state.selected.and_then(|i| state.entries.get(i)).map(|e| e.name.as_str()),
            Some("b")
        );
    }

    #[test]
    fn directory_refreshed_ignores_other_dir() {
        let mut state = BrowseState::default();
        state.current_dir = PathBuf::from("/d");
        state.all_entries = vec![entry("a", "/d/a", false)];
        apply_filters(&mut state);
        let req = update(&mut state, BrowseMessage::DirectoryRefreshed(PathBuf::from("/other"), vec![entry("z", "/other/z", false)]));
        assert!(req.is_none());
        assert_eq!(state.entries.len(), 1);
        assert_eq!(state.entries[0].name, "a");
    }

    #[test]
    fn toggle_hidden_shows_dotfiles() {
        let mut state = BrowseState::default();
        state.all_entries = vec![entry(".hidden", "/a/.hidden", false), entry("visible", "/a/visible", false)];
        apply_filters(&mut state);
        assert_eq!(state.entries.len(), 1);
        let req = update(&mut state, BrowseMessage::ToggleHidden);
        assert!(req.is_none());
        assert!(state.show_hidden);
        assert_eq!(state.entries.len(), 2);
    }

    #[test]
    fn last_dir_loaded_some_reads_that_dir() {
        let mut state = BrowseState::default();
        let req = update(&mut state, BrowseMessage::LastDirLoaded(Some(PathBuf::from("/some/dir"))));
        assert!(matches!(req, Some(crate::services::fs::FsRequest::ReadDirectory(p)) if p == PathBuf::from("/some/dir")));
    }

    #[test]
    fn last_dir_loaded_none_falls_back() {
        let mut state = BrowseState::default();
        let req = update(&mut state, BrowseMessage::LastDirLoaded(None));
        assert!(matches!(req, Some(crate::services::fs::FsRequest::ReadDirectory(_))));
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
    #[serial_test::serial]
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
        let current_dir = PathBuf::from("/home/user/documents");

        // seg 0 is the root, then each index adds a component.
        assert_eq!(path_to_segment(&current_dir, 0), PathBuf::from("/"));
        assert_eq!(path_to_segment(&current_dir, 1), PathBuf::from("/home"));
        assert_eq!(path_to_segment(&current_dir, 2), PathBuf::from("/home/user"));
        assert_eq!(path_to_segment(&current_dir, 3), PathBuf::from("/home/user/documents"));
    }
}
