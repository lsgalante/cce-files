use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use crate::pages::PageContent;

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
}

#[derive(Debug, Clone)]
pub enum PreviewMessage {
    SetPath { path: PathBuf },
    NavigateTo(PathBuf),
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

// ── View ────────────────────────────────────────────────────────────

pub fn view(state: &PreviewState, cx: f32, cy: f32, cw: f32, _ch: f32) -> PageContent {
    let mut pc = PageContent::new();
    let text_fg = [0.83, 0.83, 0.83, 1.0];
    let text_dim = [0.53, 0.53, 0.60, 1.0];
    let label_fg = [0.56, 0.83, 0.56, 1.0];
    let accent_bg = [0.36, 0.56, 0.38, 1.0];

    if state.path.is_none() {
        pc.text("Select a file to view details", cx + 12.0, cy + 12.0, 13.0, text_dim);
        return pc;
    }

    let icon = if state.is_dir { "📁" } else { "📄" };

    // Title row
    pc.text(icon, cx + 12.0, cy + 12.0, 24.0, text_fg);
    
    let name_truncated = if state.name.len() > 30 {
        format!("{}...", &state.name[..27])
    } else {
        state.name.clone()
    };
    pc.text(&name_truncated, cx + 46.0, cy + 18.0, 18.0, text_fg);

    // Metadata details
    let details = [
        ("Path", &state.path_display),
        ("Type", &state.file_type),
        ("Size", &state.size),
        ("Permissions", &state.permissions),
        ("Modified", &state.modified),
    ];

    let mut y = cy + 54.0;
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
        // Divider
        pc.rect([0.15, 0.20, 0.16, 1.0], cx + 12.0, y, cw - 24.0, 1.0);
        y += 12.0;
        pc.text("Target", cx + 12.0, y, 12.0, label_fg);
        
        let target_str = if state.target.len() > 40 {
            format!("...{}", &state.target[state.target.len() - 37..])
        } else {
            state.target.clone()
        };
        pc.text(&target_str, cx + 112.0, y, 12.0, text_dim);
        y += 20.0;
    }

    if state.is_dir {
        if let Some(path) = &state.path {
            y += 16.0;
            pc.button(
                "Open directory",
                cx + 12.0,
                y,
                140.0,
                32.0,
                accent_bg,
                [0.46, 0.66, 0.48, 1.0],
                [0.10, 0.16, 0.11, 1.0],
                crate::Message::Preview(PreviewMessage::NavigateTo(path.clone())),
            );
        }
    }

    pc
}

// ── Update ──────────────────────────────────────────────────────────

pub fn update(state: &mut PreviewState, msg: PreviewMessage) {
    match msg {
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
            };
        }
        PreviewMessage::NavigateTo(_) => {}
    }
}
