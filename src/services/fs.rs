use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;
use crate::pages::browse::DirEntry;
use crate::util::{format_size, format_permissions};
use image::GenericImageView;

/// A downscaled RGBA thumbnail of an image file. `pixels` is flat RGBA8
/// (width * height * 4 bytes) — exactly what `cce_ui::vk::upload_rgba` takes.
#[derive(Debug, Clone, Default)]
pub struct ImagePreviewData {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

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
    /// Read a selection's preview. The number is the request's generation
    /// (`FsService::preview_latest` holds the newest): a read that has been
    /// superseded before it starts is skipped, and its result, should it
    /// finish anyway, is dropped by the app.
    ReadPreview(PathBuf, u64),
    /// Move to the freedesktop trash (the default Delete).
    TrashPath(PathBuf),
    /// Unrecoverable delete — the trash's own rows, and "Delete Permanently".
    DeletePath(PathBuf, bool), // (path, is_dir)
    /// Restore a trashed item (a path under Trash/files) to its origin.
    RestorePath(PathBuf),
    EmptyTrash,
    /// Create a new, uniquely named folder inside the given directory.
    CreateDir(PathBuf),
    /// Rename (from, to) — refused when `to` already exists.
    RenamePath(PathBuf, PathBuf),
    /// Walk a whole subtree for the Space view. The flag is the caller's
    /// cancel token — raising it abandons a scan whose answer is no longer
    /// wanted (see `SpaceState::begin_scan`).
    ScanTree(PathBuf, std::sync::Arc<std::sync::atomic::AtomicBool>),
    ReadLastDir,
    SaveLastDir(PathBuf),
}

pub struct FsService {
    pub sender: mpsc::Sender<FsRequest>,
    /// Generation of the newest preview request.
    pub preview_latest: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl FsService {
    pub fn new(app_sender: calloop::channel::Sender<crate::Message>) -> Self {
        let (tx, mut rx) = mpsc::channel::<FsRequest>(100);
        let preview_latest = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let latest = preview_latest.clone();

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
                    FsRequest::ReadPreview(path, generation) => {
                        // A full image decode plus two `xdg-mime` spawns:
                        // blocking work, so the blocking pool. Arrowing down a
                        // folder of photos queues one per row; the ones passed
                        // over before their turn are never decoded.
                        let latest = latest.clone();
                        tokio::task::spawn_blocking(move || {
                            if latest.load(std::sync::atomic::Ordering::Relaxed) != generation {
                                return;
                            }
                            let preview_data = load_preview_data_internal(&path);
                            let _ = app_sender.send(crate::Message::Preview(
                                crate::pages::preview::PreviewMessage::PreviewLoaded {
                                    path,
                                    generation,
                                    data: preview_data,
                                },
                            ));
                        });
                    }
                    FsRequest::TrashPath(path) => {
                        tokio::spawn(async move {
                            let result = super::trash::move_to_trash(&path).map_err(|e| e.to_string());
                            let _ = app_sender.send(crate::Message::Browse(
                                crate::pages::browse::BrowseMessage::Deleted(path, result),
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
                    FsRequest::RestorePath(path) => {
                        tokio::spawn(async move {
                            // A restored row leaves the trash listing exactly like a
                            // deleted row leaves its directory — same message.
                            let result = super::trash::restore(&path).map(|_| ()).map_err(|e| e.to_string());
                            let _ = app_sender.send(crate::Message::Browse(
                                crate::pages::browse::BrowseMessage::Deleted(path, result),
                            ));
                        });
                    }
                    FsRequest::EmptyTrash => {
                        tokio::spawn(async move {
                            let result = super::trash::empty().map(|_| ()).map_err(|e| e.to_string());
                            let _ = app_sender.send(crate::Message::Browse(
                                crate::pages::browse::BrowseMessage::TrashEmptied(result),
                            ));
                        });
                    }
                    FsRequest::CreateDir(parent) => {
                        tokio::spawn(async move {
                            let result = create_new_folder(&parent).map_err(|e| e.to_string());
                            let _ = app_sender.send(crate::Message::Browse(
                                crate::pages::browse::BrowseMessage::FolderCreated(parent, result),
                            ));
                        });
                    }
                    FsRequest::RenamePath(from, to) => {
                        tokio::spawn(async move {
                            let result = rename_no_clobber(&from, &to).map(|_| to).map_err(|e| e.to_string());
                            let _ = app_sender.send(crate::Message::Browse(
                                crate::pages::browse::BrowseMessage::Renamed(from, result),
                            ));
                        });
                    }
                    FsRequest::ScanTree(path, cancel) => {
                        // Minutes of blocking recursion on a large tree, so
                        // this goes to the blocking pool rather than tying up
                        // an async worker the way the short reads above can.
                        tokio::task::spawn_blocking(move || {
                            let progress_sender = app_sender.clone();
                            let progress_dir = path.clone();
                            let mut on_progress = |files, bytes| {
                                let _ = progress_sender.send(crate::Message::Space(
                                    crate::pages::space::SpaceMessage::Progress {
                                        dir: progress_dir.clone(),
                                        files,
                                        bytes,
                                    },
                                ));
                            };
                            let result = super::scan::scan(&path, &cancel, &mut on_progress);
                            match result {
                                Some(res) if !res.cancelled => {
                                    let _ = app_sender.send(crate::Message::Space(
                                        crate::pages::space::SpaceMessage::Scanned {
                                            dir: path,
                                            tree: res.tree,
                                        },
                                    ));
                                }
                                // A cancelled scan's tree is partial. Drop it
                                // silently — the scan that superseded it is
                                // already on its way with the real one.
                                Some(_) => {}
                                None => {
                                    let _ = app_sender.send(crate::Message::Space(
                                        crate::pages::space::SpaceMessage::Failed(format!(
                                            "Cannot scan {}",
                                            path.display()
                                        )),
                                    ));
                                }
                            }
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

        Self { sender: tx, preview_latest }
    }

    pub fn send(&self, req: FsRequest) {
        let sender = self.sender.clone();
        tokio::spawn(async move {
            let _ = sender.send(req).await;
        });
    }
}

// ── Internal Helper Functions ───────────────────────────────────────

/// `fs::rename`, except it will not replace an existing `to` — plain rename
/// silently overwrites a file of the same name.
pub fn rename_no_clobber(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    // The same inode is a case-only rename on a case-insensitive mount, not a clobber.
    let same_file = |a: &fs::Metadata, b: &fs::Metadata| a.dev() == b.dev() && a.ino() == b.ino();
    if let Ok(existing) = fs::symlink_metadata(to)
        && !fs::symlink_metadata(from).is_ok_and(|m| same_file(&m, &existing))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("{} already exists", to.display()),
        ));
    }
    fs::rename(from, to)
}

/// Create "New Folder" in `parent`, or "New Folder 2", "New Folder 3", … when
/// the name is taken. `create_dir` itself is the existence check, so a name
/// claimed between two attempts just moves on to the next one.
pub fn create_new_folder(parent: &Path) -> std::io::Result<PathBuf> {
    for n in 1u32.. {
        let name = if n == 1 { "New Folder".to_string() } else { format!("New Folder {n}") };
        let path = parent.join(name);
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

pub fn read_directory_internal(path: &Path) -> Vec<DirEntry> {
    let in_trash = super::trash::is_trash_files_dir(path);
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
                let origin = if in_trash {
                    super::trash::origin_of(&e.path()).map(|p| p.display().to_string())
                } else {
                    None
                };
                Some(DirEntry {
                    name,
                    path: e.path(),
                    is_dir,
                    size,
                    permissions,
                    modified,
                    origin,
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
    // GPU-textured previews: 512px is crisp at pane size for one live image
    // (1 MB RGBA) while keeping the Triangle resize quick per selection.
    let max_dim = 512.0;
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
    
    let pixels = resized.to_rgba8().into_raw();

    Some(ImagePreviewData {
        width: target_w,
        height: target_h,
        pixels,
    })
}

/// Vector previews rasterize at the same 512px pane budget via cce-ui's shared
/// resvg path (fontdb-backed, thread-safe — this runs on the FsService thread)
/// and hand back the same straight-RGBA ImagePreviewData as raster files.
fn load_svg_preview(path: &Path) -> Option<ImagePreviewData> {
    let data = fs::read(path).ok()?;
    let (pixels, width, height) = cce_ui::rasterize_svg(&data, 512)?;
    Some(ImagePreviewData { width, height, pixels })
}

/// Path-traced thumbnail for a cce-designer project directory, cached as a
/// PNG under `~/.cache/cce/thumbnails/` keyed on the project path and its
/// `state.json` mtime — the GPU renders only on cache misses. Rendering is
/// delegated to `cce-designer --thumbnail` (the geometry, OpenCL nodes
/// included, lives there); returns None if the binary is missing or fails,
/// and the caller falls back to the plain directory listing.
fn load_project_thumbnail(path: &Path) -> Option<ImagePreviewData> {
    use std::hash::{Hash, Hasher};

    let state_file = path.join("state.json");
    let mtime = fs::metadata(&state_file)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.to_string_lossy().hash(&mut hasher);
    mtime.hash(&mut hasher);

    let cache_dir = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?
        .join("cce")
        .join("thumbnails");
    fs::create_dir_all(&cache_dir).ok()?;
    let cached = cache_dir.join(format!("{:016x}.png", hasher.finish()));

    if !cached.exists() {
        // Same ~/.local/bin-first resolution as spawn_command_for_path.
        let mut program = PathBuf::from("cce-designer");
        if let Ok(home) = std::env::var("HOME") {
            let local_bin = PathBuf::from(home).join(".local").join("bin").join("cce-designer");
            if local_bin.exists() {
                program = local_bin;
            }
        }
        let output = std::process::Command::new(program)
            .arg("--thumbnail")
            .arg(path)
            .arg(&cached)
            .args(["--size", "256"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
    }
    load_image_preview(&cached)
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
        // cce-designer project: show a path-traced thumbnail of its geometry.
        if path.join("state.json").exists() {
            image_preview = load_project_thumbnail(path);
        }
        if image_preview.is_none() {
            if let Ok(entries) = fs::read_dir(path) {
                let mut names = Vec::new();
                for entry in entries.flatten().take(100) {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let is_sub_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    // A text body: a directory wears the plain-text marker,
                    // a trailing slash, rather than a pictogram.
                    names.push(if is_sub_dir { format!("{name}/") } else { name });
                }
                if names.is_empty() {
                    content_preview = Some("[Empty directory]".to_string())
                } else {
                    content_preview = Some(names.join("\n"))
                }
            }
        }
    } else {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        let is_img_ext = matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico");
        
        if is_img_ext {
            image_preview = load_image_preview(path);
        } else if matches!(ext.as_str(), "svg" | "svgz") {
            // On parse failure this stays None and the text fallback below
            // shows the SVG source.
            image_preview = load_svg_preview(path);
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

/// Base config directory for cce: `$XDG_CONFIG_HOME/cce`, else `$HOME/.config/cce`.
/// Returns `None` only when neither variable is usable.
pub fn cce_config_dir() -> Option<PathBuf> {
    Some(cce_ui::config::cce_config_dir())
}

/// The last-dir file under a cce config dir (`cce_config_dir()` in the app; a temp dir
/// in tests, which is why it is a parameter: setting HOME in a test points every test
/// running alongside it at the wrong config too).
fn last_dir_file_in(config_dir: &Path) -> PathBuf {
    config_dir.join("cce-files").join("cce-files-last-dir.txt")
}

pub fn read_last_dir_internal() -> Option<PathBuf> {
    read_last_dir_in(&cce_config_dir()?)
}

pub fn read_last_dir_in(config_dir: &Path) -> Option<PathBuf> {
    let path = last_dir_file_in(config_dir);
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
    if let Some(config_dir) = cce_config_dir() {
        save_last_dir_in(&config_dir, dir);
    }
}

pub fn save_last_dir_in(config_dir: &Path, dir: &Path) {
    let path = last_dir_file_in(config_dir);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, dir.to_string_lossy().as_bytes());
}

/// Types decided by extension alone, ahead of `xdg-mime`. On this machine
/// `xdg-mime query filetype` falls back to `file --mime-type`, which sniffs
/// content: .gltf reads as application/json, .obj as text/plain, binary .stl
/// and .ply as application/octet-stream, so Open never reached cce-model.
/// shared-mime-info registers no PLY type; model/x-ply is the one
/// cce-model.desktop claims. `ext` must already be lowercase.
fn mime_for_extension(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "kdl" => "application/x-kdl",
        "stl" => "model/stl",
        "obj" => "model/obj",
        "gltf" => "model/gltf+json",
        "glb" => "model/gltf-binary",
        "ply" => "model/x-ply",
        _ => return None,
    })
}

pub fn get_mime_type(path: &Path) -> Option<String> {
    // Matches `browse::is_project_dir`: a designer project is a dir holding a state.json.
    if path.is_dir() && path.join("state.json").exists() {
        return Some("application/x-cce-project".to_string());
    }
    if let Some(mime) = path
        .extension()
        .and_then(|e| e.to_str())
        .and_then(|ext| mime_for_extension(&ext.to_ascii_lowercase()))
    {
        return Some(mime.to_string());
    }
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
    load_kdl_associations_in(&cce_config_dir()?)
}

/// `mime.kdl` under `config_dir`; see `last_dir_file_in` for why it is a parameter.
pub fn load_kdl_associations_in(config_dir: &Path) -> Option<std::collections::HashMap<String, String>> {
    let path = config_dir.join("mime.kdl");
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

/// Where `.desktop` files live, in XDG precedence order: `$XDG_DATA_HOME`
/// (else `~/.local/share`), then `$XDG_DATA_DIRS` (else /usr/local/share and
/// /usr/share), each with `applications` appended. It has to match the dirs
/// `xdg-mime query default` searched, or a handler it names is not found
/// here. Unset or relative values are skipped, per the spec.
fn applications_dirs() -> Vec<PathBuf> {
    let var = |key: &str| std::env::var_os(key).filter(|v| !v.is_empty());
    let data_home = var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| Some(PathBuf::from(var("HOME")?).join(".local/share")));
    let data_dirs: Vec<PathBuf> = var("XDG_DATA_DIRS")
        .map(|v| std::env::split_paths(&v).filter(|p| p.is_absolute()).collect())
        .filter(|dirs: &Vec<PathBuf>| !dirs.is_empty())
        .unwrap_or_else(|| vec!["/usr/local/share".into(), "/usr/share".into()]);
    data_home.into_iter().chain(data_dirs).map(|d| d.join("applications")).collect()
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

    let mut desktop_path = None;
    for dir in applications_dirs() {
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

/// Parse a command string, resolve a bare program name against `~/.local/bin`,
/// append `path` as the final argument, and spawn it detached.
/// Returns `true` if a process was spawned.
pub fn spawn_command_for_path(cmd: &str, path: &Path) -> bool {
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    if parts.is_empty() {
        return false;
    }
    let program = parts[0];
    let mut program_path = PathBuf::from(program);
    if !program_path.is_absolute() && !program.contains('/') {
        if let Ok(home) = std::env::var("HOME") {
            let local_bin = PathBuf::from(home).join(".local").join("bin").join(program);
            if local_bin.exists() {
                program_path = local_bin;
            }
        }
    }
    let mut command = std::process::Command::new(program_path);
    for arg in &parts[1..] {
        command.arg(arg);
    }
    command.arg(path);
    spawn_detached(command).is_ok()
}

/// Spawn `cmd` and reap it on a background thread, so the child never lingers
/// as a zombie once it exits. This was `cce_ui::process::spawn_detached` until
/// the toolkit dropped that module (cce-ui 4e94236) as caller-less — it had two callers here.
fn spawn_detached(mut cmd: std::process::Command) -> std::io::Result<()> {
    let mut child = cmd.spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

pub fn open_file(path: &Path) {
    let opened = get_mime_type(path)
        .and_then(|mime| get_default_application(&mime))
        .map(|(_, cmd)| !cmd.is_empty() && spawn_command_for_path(&cmd, path))
        .unwrap_or(false);

    if !opened {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(path);
        let _ = spawn_detached(command);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rename_no_clobber() {
        let dir = std::env::temp_dir().join(format!(
            "cce_test_rename_{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let (a, b, c) = (dir.join("a.txt"), dir.join("b.txt"), dir.join("c.txt"));
        fs::write(&a, "a").unwrap();
        fs::write(&b, "b").unwrap();

        // An existing target is refused and left untouched.
        assert!(rename_no_clobber(&a, &b).is_err());
        assert_eq!(fs::read_to_string(&b).unwrap(), "b");

        rename_no_clobber(&a, &c).unwrap();
        assert!(!a.exists());
        assert_eq!(fs::read_to_string(&c).unwrap(), "a");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_create_new_folder_picks_unique_names() {
        let dir = std::env::temp_dir().join(format!(
            "cce_test_new_folder_{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();

        let first = create_new_folder(&dir).unwrap();
        let second = create_new_folder(&dir).unwrap();
        assert_eq!(first, dir.join("New Folder"));
        assert_eq!(second, dir.join("New Folder 2"));
        assert!(first.is_dir() && second.is_dir());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_kdl() {
        let config_dir = std::env::temp_dir().join(format!(
            "cce_test_kdl_{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("mime.kdl"),
            "associations { association \"text/plain\" \"cce-text-editor\" }",
        )
        .unwrap();

        let assoc = load_kdl_associations_in(&config_dir);
        let _ = fs::remove_dir_all(&config_dir);

        assert_eq!(
            assoc.and_then(|m| m.get("text/plain").cloned()).as_deref(),
            Some("cce-text-editor")
        );
    }

    #[test]
    fn test_model_mime_by_extension() {
        let dir = std::env::temp_dir().join(format!(
            "cce_test_model_mime_{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        // Content that content-sniffing reads as the wrong type, so a pass
        // shows the extension decided it.
        let cases: [(&str, &[u8], &str); 7] = [
            ("part.stl", &[0u8; 84], "model/stl"),
            ("PART.STL", &[0u8; 84], "model/stl"),
            ("mesh.obj", b"v 0 0 0\n", "model/obj"),
            ("scene.gltf", b"{\"asset\":{\"version\":\"2.0\"}}", "model/gltf+json"),
            ("scene.Glb", b"glTF\x02\0\0\0", "model/gltf-binary"),
            ("scan.ply", b"ply\nformat binary_little_endian 1.0\n\0\xff", "model/x-ply"),
            ("config.KDL", b"node 1\n", "application/x-kdl"),
        ];
        let got: Vec<_> = cases
            .iter()
            .map(|(name, bytes, _)| {
                let path = dir.join(name);
                fs::write(&path, bytes).unwrap();
                get_mime_type(&path)
            })
            .collect();
        let _ = fs::remove_dir_all(&dir);

        for ((name, _, want), got) in cases.iter().zip(got) {
            assert_eq!(got.as_deref(), Some(*want), "{name}");
        }
    }
}



