mod pages;

use iced::widget::{column, container, row, text};
use iced::{keyboard, Color, Element, Length, Task, Theme};

use pages::Page;

// ── State ───────────────────────────────────────────────────────────

struct AppState {
    current_page: Page,
    browse: pages::browse::BrowseState,
    preview: pages::preview::PreviewState,
    keybindings: pages::keybindings::KeybindingsState,
}

// ── Messages ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum Message {
    SwitchPage(Page),
    Browse(pages::browse::BrowseMessage),
    Preview(pages::preview::PreviewMessage),
    Keybindings(pages::keybindings::KeybindingsMessage),
    KeyboardEvent(keyboard::Event),
}

// ── View ────────────────────────────────────────────────────────────

fn view(state: &AppState) -> Element<'_, Message> {
    let bg = Color::from_rgb8(0x1a, 0x2a, 0x1c);
    let sidebar_bg = Color::from_rgb8(0x16, 0x24, 0x18);

    let mut sidebar = column![text("Clear Filesystem")
        .size(15)
        .color(Color::from_rgb8(0x5c, 0x90, 0x60))]
    .spacing(4)
    .padding([16, 10]);

    for page in Page::ALL {
        sidebar = sidebar.push(pages::sidebar_button(page, page == state.current_page));
    }

    let sidebar_container = container(sidebar)
        .width(200)
        .height(Length::Fill)
        .style(move |_theme| container::Style {
            background: Some(sidebar_bg.into()),
            border: iced::Border {
                radius: 12.0.into(),
                ..iced::Border::default()
            },
            ..container::Style::default()
        });

    let content: Element<Message> = match state.current_page {
        Page::Browse => browse_preview_view(state),
        Page::Keybindings => pages::keybindings::view(&state.keybindings),
    };

    let content_container = container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(20)
        .style(move |_theme| container::Style {
            background: Some(bg.into()),
            border: iced::Border {
                radius: 12.0.into(),
                ..iced::Border::default()
            },
            ..container::Style::default()
        });

    let inner = row![sidebar_container, content_container]
        .width(Length::Fill)
        .height(Length::Fill)
        .spacing(6);

    container(inner)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(6)
        .style(move |_theme| container::Style {
            background: Some(bg.into()),
            ..container::Style::default()
        })
        .into()
}

fn browse_preview_view(state: &AppState) -> Element<'_, Message> {
    let panel_bg = Color::from_rgb8(0x16, 0x24, 0x18);
    let heading = Color::from_rgb8(0x8f, 0xd4, 0x8f);

    let browse_panel = container(
        column![
            text("Browse").size(13).color(heading),
            pages::browse::view(&state.browse),
        ]
        .spacing(10)
        .width(Length::Fill)
        .height(Length::Fill),
    )
    .width(Length::FillPortion(2))
    .height(Length::Fill)
    .padding(14)
    .style(move |_theme| container::Style {
        background: Some(panel_bg.into()),
        border: iced::Border {
            radius: 10.0.into(),
            ..iced::Border::default()
        },
        ..container::Style::default()
    });

    let preview_panel = container(
        column![
            text("Info").size(13).color(heading),
            pages::preview::view(&state.preview),
        ]
        .spacing(10)
        .width(Length::Fill)
        .height(Length::Fill),
    )
    .width(Length::FillPortion(3))
    .height(Length::Fill)
    .padding(14)
    .style(move |_theme| container::Style {
        background: Some(panel_bg.into()),
        border: iced::Border {
            radius: 10.0.into(),
            ..iced::Border::default()
        },
        ..container::Style::default()
    });

    row![browse_panel, preview_panel]
        .spacing(12)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

// ── Update ──────────────────────────────────────────────────────────

fn update(state: &mut AppState, message: Message) -> Task<Message> {
    match message {
        Message::SwitchPage(page) => {
            state.current_page = page;
            Task::none()
        }
        Message::Browse(msg) => {
            let selected_path = match &msg {
                pages::browse::BrowseMessage::SelectEntry(idx) => {
                    state.browse.entries.get(*idx).map(|e| e.path.clone())
                }
                pages::browse::BrowseMessage::NavigateTo(idx) => {
                    state.browse.entries.get(*idx).map(|e| e.path.clone())
                }
                _ => None,
            };
            let scroll_task = match &msg {
                pages::browse::BrowseMessage::SelectEntry(idx) => {
                    Some(pages::browse::scroll_to_selection_if_needed(&state.browse, *idx))
                }
                _ => None,
            };
            let task = pages::browse::update(&mut state.browse, msg);

            // When an entry is selected, push file info to preview
            if let Some(path) = selected_path {
                let preview_task = pages::preview::update(
                    &mut state.preview,
                    pages::preview::PreviewMessage::SetPath { path },
                );
                if let Some(scroll_task) = scroll_task {
                    return Task::batch(vec![task, preview_task, scroll_task]);
                }
                return Task::batch(vec![task, preview_task]);
            }
            task
        }
        Message::Preview(msg) => {
            // If the user double-clicks (navigates into) from preview, update browse
            let nav_path = match &msg {
                pages::preview::PreviewMessage::NavigateTo(path) => Some(path.clone()),
                _ => None,
            };
            let task = pages::preview::update(&mut state.preview, msg);
            if let Some(path) = nav_path {
                let browse_task = pages::browse::update(
                    &mut state.browse,
                    pages::browse::BrowseMessage::DirectoryLoaded(
                        pages::browse::read_directory(&path),
                    ),
                );
                return Task::batch(vec![task, browse_task]);
            }
            task
        }
        Message::Keybindings(msg) => pages::keybindings::update(&mut state.keybindings, msg),
        Message::KeyboardEvent(event) => {
            if state.current_page != Page::Browse {
                return Task::none();
            }

            let key_char = match &event {
                keyboard::Event::KeyPressed {
                    key: keyboard::Key::Character(c),
                    ..
                } => Some(c.as_str()),
                _ => None,
            };

            match event {
                keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::ArrowUp),
                    ..
                } if key_char.is_none() || key_char == Some("k") => {
                    if let Some(index) =
                        pages::browse::next_selection_index(&state.browse, pages::browse::BrowseNavigation::Up)
                    {
                        return update(
                            state,
                            Message::Browse(pages::browse::BrowseMessage::SelectEntry(index)),
                        );
                    }
                    Task::none()
                }
                keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::ArrowDown),
                    ..
                } if key_char.is_none() || key_char == Some("j") => {
                    if let Some(index) =
                        pages::browse::next_selection_index(&state.browse, pages::browse::BrowseNavigation::Down)
                    {
                        return update(
                            state,
                            Message::Browse(pages::browse::BrowseMessage::SelectEntry(index)),
                        );
                    }
                    Task::none()
                }
                keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Enter),
                    ..
                } => {
                    if let Some(idx) = state.browse.selected {
                        if let Some(entry) = state.browse.entries.get(idx) {
                            if entry.is_dir {
                                return update(
                                    state,
                                    Message::Browse(pages::browse::BrowseMessage::NavigateTo(idx)),
                                );
                            }
                        }
                    }
                    Task::none()
                }
                keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Backspace),
                    ..
                } => {
                    if let Some(parent) = state.browse.current_dir.parent() {
                        let parent = parent.to_path_buf();
                        return update(
                            state,
                            Message::Browse(pages::browse::BrowseMessage::NavigateToPath(parent)),
                        );
                    }
                    Task::none()
                }
                _ => match key_char {
                    Some("j") => {
                        if let Some(index) =
                            pages::browse::next_selection_index(&state.browse, pages::browse::BrowseNavigation::Down)
                        {
                            return update(
                                state,
                                Message::Browse(pages::browse::BrowseMessage::SelectEntry(index)),
                            );
                        }
                        Task::none()
                    }
                    Some("k") => {
                        if let Some(index) =
                            pages::browse::next_selection_index(&state.browse, pages::browse::BrowseNavigation::Up)
                        {
                            return update(
                                state,
                                Message::Browse(pages::browse::BrowseMessage::SelectEntry(index)),
                            );
                        }
                        Task::none()
                    }
                    Some("l") => {
                        if let Some(idx) = state.browse.selected {
                            if let Some(entry) = state.browse.entries.get(idx) {
                                if entry.is_dir {
                                    return update(
                                        state,
                                        Message::Browse(pages::browse::BrowseMessage::NavigateTo(idx)),
                                    );
                                }
                            }
                        }
                        Task::none()
                    }
                    Some("h") => {
                        if let Some(parent) = state.browse.current_dir.parent() {
                            let parent = parent.to_path_buf();
                            return update(
                                state,
                                Message::Browse(pages::browse::BrowseMessage::NavigateToPath(parent)),
                            );
                        }
                        Task::none()
                    }
                    Some(".") => {
                        return update(
                            state,
                            Message::Browse(pages::browse::BrowseMessage::ToggleHidden),
                        );
                    }
                    _ => Task::none(),
                },
            }
        }
    }
}

// ── Subscription ────────────────────────────────────────────────────

fn subscription(state: &AppState) -> iced::Subscription<Message> {
    match state.current_page {
        Page::Browse => iced::Subscription::batch(vec![
            pages::browse::subscription(&state.browse),
            keyboard::listen().map(Message::KeyboardEvent),
        ]),
        Page::Keybindings => pages::keybindings::subscription(&state.keybindings),
    }
}

// ── Boot / Theme / Title ────────────────────────────────────────────

fn boot() -> (AppState, Task<Message>) {
    let browse = pages::browse::BrowseState::default();
    let current_dir = browse.current_dir.clone();

    let state = AppState {
        current_page: Page::Browse,
        browse,
        preview: pages::preview::PreviewState::default(),
        keybindings: pages::keybindings::KeybindingsState,
    };

    let task = Task::perform(
        async move { pages::browse::read_directory(&current_dir) },
        |entries| Message::Browse(pages::browse::BrowseMessage::DirectoryLoaded(entries)),
    );

    (state, task)
}

fn title(_state: &AppState) -> String {
    String::from("Clear Filesystem Interface")
}

fn theme(_state: &AppState) -> Theme {
    Theme::Dark
}

// ── Main ────────────────────────────────────────────────────────────

fn main() -> iced::Result {
    let mut window = iced::window::Settings::default();
    window.size = iced::Size::new(1200.0, 720.0);
    window.platform_specific.application_id = String::from("clear-filesystem-interface");

    iced::application(boot, update, view)
        .title(title)
        .theme(theme)
        .subscription(subscription)
        .window(window)
        .run()
}
