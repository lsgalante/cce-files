pub mod pages;
pub mod preview_pane;
pub mod row_list;
pub mod services;
pub mod util;

use pages::Page;

#[derive(Debug, Clone)]
pub enum Message {
    SwitchPage(Page),
    Browse(pages::browse::BrowseMessage),
    Preview(pages::preview::PreviewMessage),
    SelectOpen,
    SelectCancel,
    PromptOpenWith(std::path::PathBuf),
    OpenWithSubmit,
    OpenWithCancel,
    CopyPath(String),
}
