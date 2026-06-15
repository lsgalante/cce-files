use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use crate::pages::PageContent;
use clear_ui::layout::SectionContext;

// ── Data ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct PreviewState {
    pub path: Option<PathBuf>,
    pub path_display: String,
    pub name: String,
    pub is_dir: bool,
    pub size: String,
    pub permissions: String,
    pub modified: String,
    pub file_type: String,
    pub target: String, // for symlinks
    pub content_preview: Option<String>,
    pub scroll_line: usize,
}

#[derive(Debug, Clone)]
pub enum PreviewMessage {
    SetPath { path: PathBuf },
    Clear,
}

// ── Helpers ─────────────────────────────────────────────────────────

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

fn infer_file_type(name: &str, is_dir: bool) -> String {
    if is_dir {
        return "Directory".to_string();
    }
    match name.rsplit('.').next() {
        Some("rs") => "Rust source".to_string(),
        Some("toml") => "TOML config".to_string(),
        Some("json") => "JSON data".to_string(),
        Some("yaml") | Some("yml") => "YAML config".to_string(),
        Some("png") => "PNG image".to_string(),
        Some("jpg") | Some("jpeg") => "JPEG image".to_string(),
        Some("svg") => "SVG image".to_string(),
        Some("gif") => "GIF image".to_string(),
        Some("mp3") => "MP3 audio".to_string(),
        Some("wav") => "WAV audio".to_string(),
        Some("flac") => "FLAC audio".to_string(),
        Some("mp4") => "MP4 video".to_string(),
        Some("mkv") => "Matroska video".to_string(),
        Some("zip") => "ZIP archive".to_string(),
        Some("tar") => "Tar archive".to_string(),
        Some("gz") => "Gzip archive".to_string(),
        Some("py") => "Python source".to_string(),
        Some("sh") | Some("bash") => "Shell script".to_string(),
        Some("md") => "Markdown".to_string(),
        Some("txt") => "Plain text".to_string(),
        Some("c") | Some("h") => "C source".to_string(),
        Some("cpp") | Some("hpp") | Some("cc") => "C++ source".to_string(),
        Some("hs") => "Haskell source".to_string(),
        Some("exe") => "Windows executable".to_string(),
        Some("pdf") => "PDF document".to_string(),
        _ => "File".to_string(),
    }
}

impl PreviewState {
    pub fn handle_mouse_wheel(&mut self, delta: &clear_ui::widget::MouseScrollDelta, ch: f32) -> bool {
        let content = match &self.content_preview {
            Some(c) => c,
            None => return false,
        };
        let total_lines = content.lines().count();
        let half_h = ch * 0.5;
        let mut max_visible_lines = 0;
        let mut text_y = 44.0;
        while text_y + 14.0 <= half_h - 16.0 {
            max_visible_lines += 1;
            text_y += 15.0;
        }
        if total_lines <= max_visible_lines {
            if self.scroll_line != 0 {
                self.scroll_line = 0;
                return true;
            }
            return false;
        }
        let max_scroll = total_lines.saturating_sub(max_visible_lines);
        let scroll_speed = 3.0;
        let diff = match delta {
            clear_ui::widget::MouseScrollDelta::LineDelta(_, y) => {
                -y * scroll_speed
            }
            clear_ui::widget::MouseScrollDelta::PixelDelta(pos) => {
                -pos.y as f32 / 15.0
            }
        };
        let prev_scroll = self.scroll_line;
        let new_scroll = (self.scroll_line as f32 + diff).round() as isize;
        self.scroll_line = new_scroll.clamp(0, max_scroll as isize) as usize;
        self.scroll_line != prev_scroll
    }
}

// ── View ────────────────────────────────────────────────────────────

pub fn view(state: &PreviewState, cx: f32, cy: f32, cw: f32, ch: f32) -> PageContent {
    let mut pc = PageContent::new();
    let text_fg = [0.83, 0.83, 0.83, 1.0];
    let text_dim = [0.53, 0.53, 0.60, 1.0];
    let label_fg = [0.56, 0.83, 0.56, 1.0];

    if state.path.is_none() {
        pc.text("Select a file to view details", cx + 12.0, cy + 12.0, 13.0, text_dim);
        return pc;
    }

    let half_h = ch * 0.5;

    // 1. Top pane: File Preview Section
    let mut preview_sec = SectionContext::new(&mut pc, cx + 4.0, cy + 12.0, cw - 8.0, "Preview", false, false);
    preview_sec.content_y = cy + half_h - 20.0;
    preview_sec.finish(); // Releases borrow on pc

    let bg_color = [0.07, 0.11, 0.08, 0.5];
    pc.rect(bg_color, cx + 12.0, cy + 32.0, cw - 24.0, half_h - 40.0);

    if let Some(content) = &state.content_preview {
        let mut text_y = cy + 44.0;
        for line in content.lines().skip(state.scroll_line) {
            if text_y + 14.0 > cy + half_h - 16.0 {
                break;
            }
            let limit = (((cw - 40.0) / 6.8).floor() as usize).max(20);
            let line_truncated = if line.chars().count() > limit {
                let mut s: String = line.chars().take(limit - 3).collect();
                s.push_str("...");
                s
            } else {
                line.to_string()
            };
            pc.text_with_font(&line_truncated, cx + 20.0, text_y, 11.0, text_fg, "monospace");
            text_y += 15.0;
        }
    } else {
        pc.text("No preview available", cx + 20.0, cy + 44.0, 11.0, text_dim);
    }

    // 2. Bottom pane: Details Section
    let bottom_y = cy + half_h + 12.0;
    let icon = if state.is_dir { "📁" } else { "📄" };

    // Metadata details
    let details = [
        ("Path", &state.path_display),
        ("Type", &state.file_type),
        ("Size", &state.size),
        ("Permissions", &state.permissions),
        ("Modified", &state.modified),
    ];

    let details_content_start_y = bottom_y + 19.0;
    let mut details_content_end_y = details_content_start_y + 36.0 + details.len() as f32 * 20.0;
    if !state.target.is_empty() {
        details_content_end_y += 24.0;
    }

    let mut details_sec = SectionContext::new(&mut pc, cx + 4.0, bottom_y, cw - 8.0, "Details", false, false);
    details_sec.content_y = details_content_end_y;
    details_sec.finish(); // Releases borrow on pc

    let header_y = details_content_start_y + 6.0;
    pc.text(icon, cx + 12.0, header_y, 20.0, text_fg);
    
    let name_truncated = if state.name.len() > 30 {
        format!("{}...", &state.name[..27])
    } else {
        state.name.clone()
    };
    pc.text(&name_truncated, cx + 42.0, header_y + 4.0, 16.0, text_fg);

    let mut y = details_content_start_y + 36.0;
    for (label, val) in &details {
        pc.text(label, cx + 12.0, y, 12.0, label_fg);
        
        let val_str = if val.len() > 40 {
            format!("...{}", &val[val.len() - 37..])
        } else {
            val.to_string()
        };
        pc.text(&val_str, cx + 112.0, y, 12.0, text_dim);
        y += 20.0;
    }

    if !state.target.is_empty() {
        y += 8.0;
        pc.text("Target", cx + 12.0, y, 12.0, label_fg);
        
        let target_str = if state.target.len() > 40 {
            format!("...{}", &state.target[state.target.len() - 37..])
        } else {
            state.target.clone()
        };
        pc.text(&target_str, cx + 112.0, y, 12.0, text_dim);
    }

    pc
}

// ── Update ──────────────────────────────────────────────────────────

pub fn update(state: &mut PreviewState, msg: PreviewMessage) {
    match msg {
        PreviewMessage::Clear => {
            *state = PreviewState::default();
        }
        PreviewMessage::SetPath { path } => {
            let meta = fs::symlink_metadata(&path).ok();
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path.to_string_lossy().to_string());

            let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
            let size = meta.as_ref().map(|m| format_size(m.len())).unwrap_or_else(|| "—".to_string());
            let permissions = meta
                .as_ref()
                .map(|m| format_permissions(m.permissions().mode()))
                .unwrap_or_else(|| "—".to_string());
            let modified = meta
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|t| {
                    let secs = t.duration_since(std::time::UNIX_EPOCH).ok()?;
                    let datetime = chrono::DateTime::from_timestamp(secs.as_secs() as i64, 0)?;
                    Some(datetime.format("%Y-%m-%d %H:%M:%S").to_string())
                })
                .unwrap_or_else(|| "—".to_string());

            let file_type = infer_file_type(&name, is_dir);

            // Check symlink target
            let target = if meta.as_ref().map(|m| m.file_type().is_symlink()).unwrap_or(false) {
                fs::read_link(&path)
                    .map(|t| t.to_string_lossy().to_string())
                    .unwrap_or_default()
            } else {
                String::new()
            };

            let path_display = path.to_string_lossy().to_string();

            // Load content preview
            let content_preview = if is_dir {
                if let Ok(entries) = fs::read_dir(&path) {
                    let mut names = Vec::new();
                    for entry in entries.flatten().take(100) {
                        let name = entry.file_name().to_string_lossy().to_string();
                        let is_sub_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                        let icon = if is_sub_dir { "📁" } else { "📄" };
                        names.push(format!("{} {}", icon, name));
                    }
                    if names.is_empty() {
                        Some("[Empty directory]".to_string())
                    } else {
                        Some(names.join("\n"))
                    }
                } else {
                    None
                }
            } else if let Ok(mut file) = fs::File::open(&path) {
                use std::io::Read;
                let mut buf = vec![0u8; 65536];
                if let Ok(n) = file.read(&mut buf) {
                    buf.truncate(n);
                    let is_text = match std::str::from_utf8(&buf) {
                        Ok(_) => true,
                        Err(err) => err.error_len().is_none() && err.valid_up_to() > 0,
                    };
                    if is_text {
                        let utf8_str = String::from_utf8_lossy(&buf).into_owned();
                        Some(utf8_str)
                    } else {
                        Some("[Binary file content]".to_string())
                    }
                } else {
                    None
                }
            } else {
                None
            };

            *state = PreviewState {
                path: Some(path),
                path_display,
                name,
                is_dir,
                size,
                permissions,
                modified,
                file_type,
                target,
                content_preview,
                scroll_line: 0,
            };
        }
    }
}
