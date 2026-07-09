use std::path::PathBuf;

pub use cce_ui::widget::PreviewState;

// ── Data ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum PreviewMessage {
    SetPath { path: PathBuf },
    Clear,
    PreviewLoaded { path: PathBuf, data: crate::services::fs::PreviewData },
}

// ── Update ──────────────────────────────────────────────────────────

pub fn update(state: &mut PreviewState, msg: PreviewMessage) {
    match msg {
        PreviewMessage::Clear => {
            *state = PreviewState::default();
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
            state.image_preview = data.image_preview;
            state.scroll_line = 0;
        }
    }
}
