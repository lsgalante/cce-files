use iced::widget::{column, row, rule, text};
use iced::{Color, Element, Length, Task};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use crate::Message;

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

pub fn view(state: &PreviewState) -> Element<'_, Message> {
    let accent = Color::from_rgb8(0x5c, 0x90, 0x60);
    let text_fg = Color::from_rgb8(0xd4, 0xd4, 0xd4);
    let text_dim = Color::from_rgb8(0x88, 0x88, 0x99);
    let label_fg = Color::from_rgb8(0x8f, 0xd4, 0x8f);

    if state.path.is_none() {
        return column![text("Select a file to view details")
            .size(13)
            .color(text_dim)]
        .width(Length::Fill)
        .into();
    }

    let icon = if state.is_dir { "📁" } else { "📄" };

    let mut content = column![
        row![]
            .spacing(10)
            .push(text(icon).size(24))
            .push(text(state.name.clone()).size(18).color(text_fg)),
    ]
    .spacing(16);

    // File details
    let details = column![
        info_row("Path", &state.path_display, label_fg, text_dim),
        info_row("Type", &state.file_type, label_fg, text_dim),
        info_row("Size", &state.size, label_fg, text_dim),
        info_row("Permissions", &state.permissions, label_fg, text_dim),
        info_row("Modified", &state.modified, label_fg, text_dim),
    ]
    .spacing(6);

    content = content.push(details);

    if !state.target.is_empty() {
        content = content.push(
            column![
                rule::horizontal(1).style(|_theme| rule::Style {
                    color: Color::from_rgb8(0x26, 0x33, 0x28),
                    radius: 0.0.into(),
                    fill_mode: rule::FillMode::Full,
                    snap: true,
                }),
                info_row("Target", &state.target, label_fg, text_dim),
            ]
            .spacing(6),
        );
    }

    // Open button for directories
    if state.is_dir {
        if let Some(path) = &state.path {
            let path_clone = path.clone();
            content = content.push(
                iced::widget::button(text("Open directory").size(12).color(Color::from_rgb8(0x1a, 0x2a, 0x1c)))
                    .style(move |_theme, _status| iced::widget::button::Style {
                        background: Some(accent.into()),
                        border: iced::Border {
                            radius: 6.0.into(),
                            ..iced::Border::default()
                        },
                        ..iced::widget::button::Style::default()
                    })
                    .padding([8, 16])
                    .on_press(Message::Preview(PreviewMessage::NavigateTo(path_clone.clone()))),
            );
        }
    }

    content.width(Length::Fill).into()
}

fn info_row<'a>(label: &'a str, value: &'a str, label_fg: Color, value_fg: Color) -> iced::widget::Row<'a, Message> {
    row![]
        .spacing(8)
        .push(text(label).size(12).color(label_fg).width(100))
        .push(text(value).size(12).color(value_fg).width(Length::Fill))
}

// ── Update ──────────────────────────────────────────────────────────

pub fn update(state: &mut PreviewState, msg: PreviewMessage) -> Task<Message> {
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
            Task::none()
        }
        PreviewMessage::NavigateTo(_) => Task::none(),
    }
}

// ── Subscription ────────────────────────────────────────────────────

pub fn subscription(_state: &PreviewState) -> iced::Subscription<Message> {
    iced::Subscription::none()
}
