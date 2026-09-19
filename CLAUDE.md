# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`cce-files` is a Wayland-native file manager, one app in the larger **CCE** desktop-environment ecosystem (the sibling `cce-*` crates under `../`). It renders directly on a Wayland surface with raw Vulkan (via **ash**), with **cosmic-text** for text shaping — there is no GTK/Qt/web layer. All GUI primitives come from the sibling crate **`cce-ui`** (`../cce-ui`, a path dependency), which owns the windowing/event loop, the widget toolkit, layout, fonts, and colors; the rendering itself lives in `cce-ui/src/vk/` (`VkRenderer`), so this crate declares no graphics dependency of its own.

## Build / run / test

```sh
make build      # cargo build --release
make install    # release build, then `ccebuild install --no-build cce-files`
make run        # cargo run  (needs a live Wayland compositor)
cargo test      # run unit tests (nine modules have them; browse.rs has the most)
cargo test test_is_project_dir_detection    # run a single test by name
```

Running the binary requires a Wayland session — it will not run headless. Edition is **2024**; the `cce-ui` sibling is edition 2021. When touching layout/widget behavior, the actual widget implementations live in `../cce-ui/src/widget/`, not here.

## Architecture

### Elm-style app on the cce-ui engine
`main.rs` defines `FilesystemApp`, which implements `cce_ui::engine::Application`. That trait drives everything through a message loop:
- **`Message`** (`lib.rs`) is the single top-level event enum; page-specific messages nest inside it (`Message::Browse(BrowseMessage)`, `Message::Preview(PreviewMessage)`).
- **`update()`** mutates state in response to a `Message` and may dispatch async work to `FsService`.
- **`view` / `rebuild_layout()`** produce the frame, and **`display_list()`** is the single paint path: it re-runs `rebuild_layout()` when the size, the scale, or `needs_rebuild` says to, then replays the flattened buffers into a `PaintCtx`. `display_list_text()` opts the text into the engine's shaping pass. (The legacy `view_rounded_quads()` / `text_items()` pull methods are gone.)

Each page has a `state` struct and a `view()` that returns a `PageContent` (`pages/mod.rs`) — a flat list of rects, texts, and buttons — which `rebuild_layout` later flattens into `self.widgets` + `self.texts` for the paint path. (`PageContent` also carries `reliefs`, `grooves`, and `images` — the edge-only relief walls drawn over the flat rects, the breadcrumb's slanted seams, and GPU-textured quads.) Interactive pages also carry their own `Message` enum + `update()` (`pages/browse.rs`, `pages/preview.rs`); Network is view-only, driven directly from `BrowseState` and pointer/graph events, so it has no message type of its own.

Shared, page-independent formatting helpers (`format_size`, `format_permissions`) live in `src/util.rs`.

### FsService: all filesystem IO is async and off-thread
`services/fs.rs` runs a Tokio task that receives `FsRequest`s (read dir, refresh, read preview, delete, load/save last dir), performs the blocking IO, and sends results back into the app as `Message`s over a `calloop::channel::Sender`. `update()` never does blocking IO directly — it sends an `FsRequest` and handles the resulting message later. A `notify` watcher (`start_watching`) debounces filesystem events and triggers `RefreshDirectory`.

### rebuild_layout is the render heart (main.rs)
`rebuild_layout()` gathers geometry from five sources — the root window, page content, popovers, the context menu, and the open-with dialog — and flattens them into `self.widgets` + `self.texts`. Two non-obvious concerns live here:
- **Viewport clipping**: page content is clipped to the content region so scrolled rows/text don't overflow into the breadcrumb or selection bar.
- **Overlay occlusion**: text/buttons under a popover, context menu, or dialog are either discarded or bound-clipped so they don't bleed through overlays. This is the logic behind commits like "Fix text rendering through popovers/overlays."

### Widgets register parentless, once per rebuild
There are no composite container widgets. Every widget is owned outright by the app or by a page's state struct — `self.paginator`, `self.view_dropdown`, `self.browse.breadcrumb`, `self.browse.save_name_box`, `self.network.graph`, `self.space.breadcrumb`, … — and each `rebuild_layout` re-establishes the whole hierarchy from scratch: `ui_context.clear_hierarchy()`, then a teardown block calling `clear_children` + `set_parent(None)` on every widget, then registration via `ctx.register_widget(w.base().id(), w.as_ptr_mut())` with `set_parent(None)` again. Raw pointers are still involved (`as_ptr_mut`, plus a `self_ptr` alias so the registration loop can hold the app twice), but they belong to the cce-ui widget model rather than to any container of this crate's.

**When adding a widget, add it to both halves of that pass** — the teardown block and the registration block. Skipping the teardown leaves hierarchy links alive across frames.

(`BrowseContainer` and `NetworkContainer` were shims holding children as `*mut dyn Element`; they dissolved in Phase 6y along with the root plate container and `SplitBox`. Their positioning duplicated what the pages already computed from the pane rect — that coincidence was the Phase 0 double-paint — and the only part worth keeping, the divider, became `SplitPane`. See the comment above `SplitPane` in `main.rs`.)

### Three pages, one preview
- **Browse** — the `List` widget (columnar, integrated search box) plus a `Breadcrumb`. The right pane is a `Preview` widget, split from the list by `SplitPane` — app-owned (`main.rs`), carrying the `SplitBox` two-child horizontal math verbatim plus the divider quad, its hover tint, and the proportion drag.
- **Network** — a `Graph` view of the same directory (nodes = entries), also split against the preview.
- **Space** — a GrandPerspective-style treemap of the whole subtree, also split against the preview.
The active page is picked by `view_dropdown` next to the breadcrumb (there is no sidebar — it was removed; `has_sidebar` is hardcoded `false`). The dropdown's labels name the *visualization* ("List"/"Graph"/"Space") and are a separate list from `Page::label()` ("Browse"/"Network"/"Space") — but **its order must track `Page::ALL`**, because the selected index is indexed straight into it.

### The Space treemap
Unlike the other two pages, Space needs data no other page has: the recursive size of everything below the current directory. `services/scan.rs` walks it on the FsService's blocking pool (`FsRequest::ScanTree`), never following symlinks and never crossing a device boundary — the latter is what keeps a scan of `/` out of `/proc`, `/sys`, and mounted drives. Progress is reported every 150 ms; the finished tree arrives as `SpaceMessage::Scanned`.

`pages/space.rs` then lays that tree out with a **squarified** treemap (Bruls/Huizing/van Wijk), which keeps tiles near-square so areas stay visually comparable — a naive slice-and-dice degenerates into unreadable slivers. Layout is recursive, with a directory's children nested inside its rect, and is cached against the pane rect (`laid_out`) so it only recomputes on a resize or a new tree. Tiles under `MIN_TILE` px are dropped rather than emitted as sub-pixel slivers; that culling, not `MAX_TILES`, is what actually bounds tile count. Files are colored by extension `Category`; directories paint only a frame.

Two things to know when touching it:
- Tiles are flattened **parents-before-children**, so the hit-test is `rposition` (last match = deepest tile).
- Selection is held as a `PathBuf`, not an index, because a relayout renumbers every tile. Same reason `last_space_path` (not a row index) drives Space's double-click detection.

## Domain specifics

- **CCE projects**: a directory containing `state.json` or `state.kdl` is treated as a *project* (`is_project_dir`), gets MIME `application/x-cce-project`, and on double-click is opened by its handler rather than entered. "Enter Directory" in the context menu overrides this.
- **Opening files**: `open_file()` resolves a handler via `get_mime_type` → `get_default_application`, which checks (1) `~/.config/cce/mime.kdl` custom associations, then (2) `xdg-mime` + `.desktop` parsing. Bare command names are resolved against `~/.local/bin` before falling back to `xdg-open`. Always launch via `cce_ui::process::spawn_detached`.
- **Chooser modes**: launched with `--select`, `--select-dir`, or `--save`, the app becomes a file picker for other CCE apps — it shows a bottom action bar, prints the chosen path to stdout, and `std::process::exit(0)` on selection (or exit code 1 on cancel). This is why `SelectOpen`/`SelectCancel` call `process::exit` directly.
- **Persistence**: the last-visited directory is saved to `~/.config/cce/cce-files/cce-files-last-dir.txt` and restored on launch.
- **Double-click**: opening is temporal — `last_click_time` / `last_clicked_idx` in `update()` detect a double-click within 500ms rather than relying on a windowing double-click event.
- **Fonts**: `cce_ui::create_font_system()` loads bundled fonts from `cce_ui::fonts_dir()` — `$CCE_FONTS_DIR`, else `$HOME/Dropbox/Fonts`. It is resolved, not hardcoded, and the override is what a shadow session needs: a shadow HOME cannot see the real `~/Dropbox/Fonts`, so without `CCE_FONTS_DIR` its screenshots render in a fallback sans. Set `CCE_LOAD_SYSTEM_FONTS` to also load system fonts.
