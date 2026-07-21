use std::path::PathBuf;

use crate::preview_pane::PreviewPane;

// ── Data ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum PreviewMessage {
    SetPath { path: PathBuf },
    Clear,
    PreviewLoaded { path: PathBuf, data: crate::services::fs::PreviewData },
}

// ── Update ──────────────────────────────────────────────────────────

pub fn update(state: &mut PreviewPane, msg: PreviewMessage) {
    match msg {
        PreviewMessage::Clear => {
            // Free the texture BEFORE the wholesale replace — a plain
            // Default::default() swap would leak the uploaded id.
            state.set_image(None);
            *state = PreviewPane::default();
        }
        PreviewMessage::SetPath { path: _ } => {
            // Deprecated direct SetPath, as we now load previews via the FsService.
        }
        PreviewMessage::PreviewLoaded { path, data } => {
            state.path_display = path.to_string_lossy().to_string();
            state.path = Some(path);
            state.name = data.name;
            state.is_dir = data.is_dir;
            state.size = data.size;
            state.permissions = data.permissions;
            state.modified = data.modified;
            state.file_type = data.file_type;
            state.target = data.target;
            state.content_preview = data.content_preview;
            state.set_image(data.image_preview.map(|img| (img.pixels, img.width, img.height)));
            state.scroll_line = 0;
        }
    }
}
