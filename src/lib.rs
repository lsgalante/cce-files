pub mod pages;
pub mod services;

use cce_ui::widget::KeyEvent;
use pages::Page;

#[derive(Debug, Clone)]
pub enum Message {
    SwitchPage(Page),
    Browse(pages::browse::BrowseMessage),
    Preview(pages::preview::PreviewMessage),
    KeyboardEvent(KeyEvent),
    SelectOpen,
    SelectCancel,
    PromptOpenWith(std::path::PathBuf),
    OpenWithSubmit,
    OpenWithCancel,
    CopyPath(String),
}
