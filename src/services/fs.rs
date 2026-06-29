use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;
use crate::pages::browse::DirEntry;
use image::GenericImageView;
use cce_ui::widget::ImagePreviewData;

#[derive(Debug, Clone, Default)]
pub struct PreviewData {
    pub name: String,
    pub is_dir: bool,
    pub size: String,
    pub permissions: String,
    pub modified: String,
    pub file_type: String,
    pub target: String,
    pub content_preview: Option<String>,
    pub image_preview: Option<ImagePreviewData>,
}

#[derive(Debug, Clone)]
pub enum FsRequest {
    ReadDirectory(PathBuf),
    RefreshDirectory(PathBuf),
    ReadPreview(PathBuf),
    DeletePath(PathBuf, bool), // (path, is_dir)
    ReadLastDir,
    SaveLastDir(PathBuf),
}

pub struct FsService {
    pub sender: mpsc::Sender<FsRequest>,
}

impl FsService {
    pub fn new(app_sender: calloop::channel::Sender<crate::Message>) -> Self {
        let (tx, mut rx) = mpsc::channel::<FsRequest>(100);

        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                let app_sender = app_sender.clone();
                match req {
                    FsRequest::ReadDirectory(path) => {
                        tokio::spawn(async move {
                            let entries = read_directory_internal(&path);
                            let _ = app_sender.send(crate::Message::Browse(
                                crate::pages::browse::BrowseMessage::DirectoryLoaded(path, entries),
                            ));
                        });
                    }
                    FsRequest::RefreshDirectory(path) => {
                        tokio::spawn(async move {
                            let entries = read_directory_internal(&path);
                            let _ = app_sender.send(crate::Message::Browse(
                                crate::pages::browse::BrowseMessage::DirectoryRefreshed(path, entries),
                            ));
                        });
                    }
                    FsRequest::ReadPreview(path) => {
                        tokio::spawn(async move {
                            let preview_data = load_preview_data_internal(&path);
                            let _ = app_sender.send(crate::Message::Preview(
                                crate::pages::preview::PreviewMessage::PreviewLoaded { path, data: preview_data },
                            ));
                        });
                    }
                    FsRequest::DeletePath(path, is_dir) => {
                        tokio::spawn(async move {
                            let res = if is_dir {
                                fs::remove_dir_all(&path)
                            } else {
                                fs::remove_file(&path)
                            };
                            let result = res.map_err(|e| e.to_string());
                            let _ = app_sender.send(crate::Message::Browse(
                                crate::pages::browse::BrowseMessage::Deleted(path, result),
                            ));
                        });
                    }
                    FsRequest::ReadLastDir => {
                        tokio::spawn(async move {
                            let last_dir = read_last_dir_internal();
                            let _ = app_sender.send(crate::Message::Browse(
                                crate::pages::browse::BrowseMessage::LastDirLoaded(last_dir),
                            ));
                        });
                    }
                    FsRequest::SaveLastDir(dir) => {
                        tokio::spawn(async move {
                            save_last_dir_internal(&dir);
                        });
                    }
                }
            }
        });

        Self { sender: tx }
    }

    pub fn send(&self, req: FsRequest) {
        let sender = self.sender.clone();
        tokio::spawn(async move {
            let _ = sender.send(req).await;
        });
    }
}

// ── Internal Helper Functions ───────────────────────────────────────

pub fn read_directory_internal(path: &Path) -> Vec<DirEntry> {
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

fn load_image_preview(path: &Path) -> Option<ImagePreviewData> {
    let img = image::open(path).ok()?;
    let (orig_w, orig_h) = img.dimensions();
    if orig_w == 0 || orig_h == 0 {
        return None;
    }
    let max_dim = 96.0;
    let ratio = (max_dim / orig_w as f32).min(max_dim / orig_h as f32).min(1.0);
    let target_w = (orig_w as f32 * ratio).round() as u32;
    let target_h = (orig_h as f32 * ratio).round() as u32;
    if target_w == 0 || target_h == 0 {
        return None;
    }
    
    let resized = if target_w == orig_w && target_h == orig_h {
        img
    } else {
        img.resize(target_w, target_h, image::imageops::FilterType::Triangle)
    };
    
    let rgba = resized.to_rgba8();
    let pixels = rgba.chunks_exact(4).map(|p| [p[0], p[1], p[2], p[3]]).collect();
    
    Some(ImagePreviewData {
        width: target_w,
        height: target_h,
        pixels,
    })
}

fn load_preview_data_internal(path: &Path) -> PreviewData {
    let meta = fs::symlink_metadata(path).ok();
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

    let mut file_type = infer_file_type(&name, is_dir);
    if !is_dir {
        if let Some(mime) = get_mime_type(path) {
            if let Some((app_name, _)) = get_default_application(&mime) {
                file_type = format!("{} ({}) [Open with: {}]", file_type, mime, app_name);
            } else {
                file_type = format!("{} ({})", file_type, mime);
            }
        }
    }


    // Check symlink target
    let target = if meta.as_ref().map(|m| m.file_type().is_symlink()).unwrap_or(false) {
        fs::read_link(path)
            .map(|t| t.to_string_lossy().to_string())
            .unwrap_or_default()
    } else {
        String::new()
    };

    let mut content_preview = None;
    let mut image_preview = None;

    if is_dir {
        if let Ok(entries) = fs::read_dir(path) {
            let mut names = Vec::new();
            for entry in entries.flatten().take(100) {
                let name = entry.file_name().to_string_lossy().to_string();
                let is_sub_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                let icon = if is_sub_dir { "📁" } else { "📄" };
                names.push(format!("{} {}", icon, name));
            }
            if names.is_empty() {
                content_preview = Some("[Empty directory]".to_string())
            } else {
                content_preview = Some(names.join("\n"))
            }
        }
    } else {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        let is_img_ext = matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico");
        
        if is_img_ext {
            image_preview = load_image_preview(path);
        }
        
        if image_preview.is_none() {
            if let Ok(mut file) = fs::File::open(path) {
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
                        content_preview = Some(utf8_str);
                    } else {
                        content_preview = Some("[Binary file content]".to_string());
                    }
                }
            }
        }
    }

    PreviewData {
        name,
        is_dir,
        size,
        permissions,
        modified,
        file_type,
        target,
        content_preview,
        image_preview,
    }
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

fn get_last_dir_file_path() -> Option<PathBuf> {
    let dir = if let Ok(xdg_config) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg_config.is_empty() {
            PathBuf::from(xdg_config)
        } else {
            let home = std::env::var("HOME").ok()?;
            PathBuf::from(home).join(".config")
        }
    } else {
        let home = std::env::var("HOME").ok()?;
        PathBuf::from(home).join(".config")
    };
    let dir = dir.join("cce").join("cce-files");
    let _ = fs::create_dir_all(&dir);
    Some(dir.join("cce-files-last-dir.txt"))
}

pub fn read_last_dir_internal() -> Option<PathBuf> {
    let path = get_last_dir_file_path()?;
    if path.exists() {
        let content = fs::read_to_string(path).ok()?;
        let trimmed = content.trim();
        if !trimmed.is_empty() {
            let pb = PathBuf::from(trimmed);
            if pb.exists() && pb.is_dir() {
                return Some(pb);
            }
        }
    }
    None
}

pub fn save_last_dir_internal(dir: &Path) {
    if let Some(path) = get_last_dir_file_path() {
        let _ = fs::write(path, dir.to_string_lossy().as_bytes());
    }
}

pub fn get_mime_type(path: &Path) -> Option<String> {
    let output = std::process::Command::new("xdg-mime")
        .args(&["query", "filetype"])
        .arg(path)
        .output()
        .ok()?;
    if output.status.success() {
        let mime = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !mime.is_empty() {
            return Some(mime);
        }
    }
    None
}

pub fn load_kdl_associations() -> Option<std::collections::HashMap<String, String>> {
    let home = std::env::var("HOME").ok()?;
    let path = PathBuf::from(home).join(".config").join("cce").join("mime.kdl");
    if !path.exists() {
        return None;
    }
    let content = fs::read_to_string(path).ok()?;
    let doc: kdl::KdlDocument = content.parse().ok()?;
    let mut map = std::collections::HashMap::new();

    if let Some(associations_node) = doc.get("associations") {
        for child in associations_node.iter_children() {
            if child.name().value() == "association" {
                let mime = child.get(0)
                    .or_else(|| child.get("mime"))
                    .and_then(|val| match val {
                        kdl::KdlValue::String(s) => Some(s.clone()),
                        _ => None,
                    });
                let exec = child.get(1)
                    .or_else(|| child.get("exec"))
                    .and_then(|val| match val {
                        kdl::KdlValue::String(s) => Some(s.clone()),
                        _ => None,
                    });
                if let (Some(m), Some(e)) = (mime, exec) {
                    map.insert(m, e);
                }
            }
        }
    }

    if map.is_empty() {
        None
    } else {
        Some(map)
    }
}

pub fn get_default_application(mime: &str) -> Option<(String, String)> {
    // 1. Check KDL configuration file override
    if let Some(associations) = load_kdl_associations() {
        if let Some(custom_exec) = associations.get(mime) {
            return Some((custom_exec.clone(), custom_exec.clone()));
        }
    }

    // 2. Query default handler desktop file name
    let output = std::process::Command::new("xdg-mime")
        .args(&["query", "default", mime])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let desktop_filename = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if desktop_filename.is_empty() {
        return None;
    }

    // Search for .desktop file in common directories
    let home = std::env::var("HOME").ok().unwrap_or_default();
    let search_paths = vec![
        PathBuf::from(&home).join(".local/share/applications"),
        PathBuf::from("/usr/share/applications"),
        PathBuf::from("/usr/local/share/applications"),
    ];

    let mut desktop_path = None;
    for dir in search_paths {
        let path = dir.join(&desktop_filename);
        if path.exists() {
            desktop_path = Some(path);
            break;
        }
    }

    let path = desktop_path?;
    let content = fs::read_to_string(path).ok()?;

    let mut name = None;
    let mut exec = None;

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("Name=") && name.is_none() {
            name = Some(line["Name=".len()..].trim().to_string());
        } else if line.starts_with("Exec=") && exec.is_none() {
            let mut cmd = line["Exec=".len()..].trim().to_string();
            // Strip standard desktop entry field codes (placeholders)
            let placeholders = ["%f", "%F", "%u", "%U", "%d", "%D", "%n", "%N", "%i", "%c", "%k", "%v"];
            for placeholder in &placeholders {
                cmd = cmd.replace(placeholder, "");
            }
            exec = Some(cmd.trim().to_string());
        }
    }

    match (name, exec) {
        (Some(n), Some(e)) => Some((n, e)),
        (None, Some(e)) => {
            let n_fallback = desktop_filename.strip_suffix(".desktop").unwrap_or(&desktop_filename).to_string();
            Some((n_fallback, e))
        }
        _ => None,
    }
}

pub fn open_file(path: &Path) {
    let mut opened = false;
    if let Some(mime) = get_mime_type(path) {
        if let Some((_, cmd)) = get_default_application(&mime) {
            if !cmd.is_empty() {
                let parts: Vec<&str> = cmd.split_whitespace().collect();
                if !parts.is_empty() {
                    let program = parts[0];
                    let mut command = std::process::Command::new(program);
                    for arg in &parts[1..] {
                        command.arg(arg);
                    }
                    command.arg(path);
                    if command.spawn().is_ok() {
                        opened = true;
                    }
                }
            }
        }
    }
    if !opened {
        let _ = std::process::Command::new("xdg-open")
            .arg(path)
            .spawn();
    }
}


