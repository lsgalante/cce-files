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
            let path_display = path.to_string_lossy().to_string();
            *state = PreviewState {
                path: Some(path),
                path_display,
                name: data.name,
                is_dir: data.is_dir,
                size: data.size,
                permissions: data.permissions,
                modified: data.modified,
                file_type: data.file_type,
                target: data.target,
                content_preview: data.content_preview,
                scroll_line: 0,
                ..PreviewState::default()
            };
        }
    }
}
