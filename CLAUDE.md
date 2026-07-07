# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`cce-files` is a Wayland-native file manager, one app in the larger **CCE** desktop-environment ecosystem (the sibling `cce-*` crates under `../`). It renders directly with wgpu + glyphon on a Wayland surface — there is no GTK/Qt/web layer. All GUI primitives come from the sibling crate **`cce-ui`** (`../cce-ui`, a path dependency), which owns the windowing/event loop, the widget toolkit, layout, fonts, and colors.

## Build / run / test

```sh
make build      # cargo build --release
make install    # build + install binary to ~/.local/bin/cce-files
make run        # cargo run  (needs a live Wayland compositor)
cargo test      # run unit tests (fs and browse modules have them)
cargo test test_is_project_dir_detection    # run a single test by name
```

Running the binary requires a Wayland session — it will not run headless. Edition is **2024**; the `cce-ui` sibling is edition 2021. When touching layout/widget behavior, the actual widget implementations live in `../cce-ui/src/widget/`, not here.

## Architecture

### Elm-style app on the cce-ui engine
`main.rs` defines `FilesystemApp`, which implements `cce_ui::engine::Application`. That trait drives everything through a message loop:
- **`Message`** (`lib.rs`) is the single top-level event enum; page-specific messages nest inside it (`Message::Browse(BrowseMessage)`, `Message::Preview(PreviewMessage)`).
- **`update()`** mutates state in response to a `Message` and may dispatch async work to `FsService`.
- **`view` / `rebuild_layout()`** produce the frame. cce-ui calls `view_rounded_quads()` / `text_items()` to pull the rendered geometry.

Each page has a `state` struct and a `view()` that returns a `PageContent` (`pages/mod.rs`) — a flat list of rects, texts, and buttons — which `rebuild_layout` later translates into GPU quads and glyphon `TextItem`s. Interactive pages also carry their own `Message` enum + `update()` (`pages/browse.rs`, `pages/preview.rs`); Network is view-only, driven directly from `BrowseState` and pointer/graph events, so it has no message type of its own.

Shared, page-independent formatting helpers (`format_size`, `format_permissions`) live in `src/util.rs`.

### FsService: all filesystem IO is async and off-thread
`services/fs.rs` runs a Tokio task that receives `FsRequest`s (read dir, refresh, read preview, delete, load/save last dir), performs the blocking IO, and sends results back into the app as `Message`s over a `calloop::channel::Sender`. `update()` never does blocking IO directly — it sends an `FsRequest` and handles the resulting message later. A `notify` watcher (`start_watching`) debounces filesystem events and triggers `RefreshDirectory`.

### rebuild_layout is the render heart (main.rs)
`rebuild_layout()` gathers geometry from five sources — the root window, page content, popovers, the context menu, and the open-with dialog — and flattens them into `self.widgets` + `self.text_items`. Two non-obvious concerns live here:
- **Viewport clipping**: page content is clipped to the content region so scrolled rows/text don't overflow into the breadcrumb or selection bar.
- **Overlay occlusion**: text/buttons under a popover, context menu, or dialog are either discarded or bound-clipped so they don't bleed through overlays. This is the logic behind commits like "Fix text rendering through popovers/overlays."

### Widget hierarchy uses raw pointers
`BrowseContainer` and `NetworkContainer` are composite widgets whose children (`breadcrumb`, `list_box`, `save_name_box`, `graph`) are held as `*mut dyn Element` and wired up in `set_parent` via `ctx.register_widget` / `ctx.link_ids`. This mirrors the cce-ui widget model; the containers are `unsafe impl Send/Sync`. When adding a child widget to a container, replicate the register + link + `set_parent` sequence, and clear it in `rebuild_layout`'s teardown block.

### Two pages, one preview
- **Browse** — the `List` widget (columnar, integrated search box) plus a `Breadcrumb`. The right pane is a `Preview` widget, split from the list by a `SplitBox`.
- **Network** — a `Graph` view of the same directory (nodes = entries), also split against the preview.
The active page is picked by `view_dropdown` next to the breadcrumb (there is no sidebar — it was removed; `has_sidebar` is hardcoded `false`).

## Domain specifics

- **CCE projects**: a directory containing `state.json` or `state.kdl` is treated as a *project* (`is_project_dir`), gets MIME `application/x-cce-project`, and on double-click is opened by its handler rather than entered. "Enter Directory" in the context menu overrides this.
- **Opening files**: `open_file()` resolves a handler via `get_mime_type` → `get_default_application`, which checks (1) `~/.config/cce/mime.kdl` custom associations, then (2) `xdg-mime` + `.desktop` parsing. Bare command names are resolved against `~/.local/bin` before falling back to `xdg-open`. Always launch via `cce_ui::process::spawn_detached`.
- **Chooser modes**: launched with `--select`, `--select-dir`, or `--save`, the app becomes a file picker for other CCE apps — it shows a bottom action bar, prints the chosen path to stdout, and `std::process::exit(0)` on selection (or exit code 1 on cancel). This is why `SelectOpen`/`SelectCancel` call `process::exit` directly.
- **Persistence**: the last-visited directory is saved to `~/.config/cce/cce-files/cce-files-last-dir.txt` and restored on launch.
- **Double-click**: opening is temporal — `last_click_time` / `last_clicked_idx` in `update()` detect a double-click within 500ms rather than relying on a windowing double-click event.
- **Fonts**: `cce_ui::create_font_system()` loads fonts from `/home/lsgalante/Dropbox/Fonts` (hardcoded in cce-ui). Set `CCE_LOAD_SYSTEM_FONTS` to also load system fonts.
