//! Where the window was, so it comes back there: the page (Browse, Network,
//! Space), the directory, the selected row and the list's scroll. Saved on a
//! page switch and when the window closes (`on_exit` — which a restart, the
//! remote's "Restart focused app" or `ccectl restart-app`, runs as any close
//! does), taken back when one launches — after a restart, at login, or when
//! reopened. The chooser (`--select`, `--select-dir`, `--save`) neither saves
//! nor restores: it is a picker another app opened for one answer.
//!
//! **A restart replays the same command line**, so `cce-files /tmp` would
//! always come back on /tmp. The file therefore also records the directory the
//! command line named (`launched`): the same command line again, or one
//! naming nothing, is this window reopening, and the saved place wins; a
//! command line naming another directory is a new request, and it wins.
//!
//! The last-visited directory file (`services::fs`, `cce-files-last-dir.txt`)
//! is unchanged and still the fallback: with no place to restore and no
//! directory on the command line, a launch opens there as it always has.
//!
//! `cce_state_dir()/files/state.json`, written whole to a temporary file and
//! renamed over, so a crash mid-write leaves the last one.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::pages::Page;

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
struct Saved {
    /// The page on screen, by label.
    page: String,
    dir: Option<PathBuf>,
    /// The selected row's path, in `dir`.
    selected: Option<PathBuf>,
    /// The Browse list's offset.
    scroll: f32,
    /// The directory the command line named when this was saved, if any.
    launched: Option<PathBuf>,
}

/// Where a launch opens.
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub page: Page,
    /// The directory, with the row to select in it and the list's offset.
    /// `None` (it is gone): the command line's directory, else the last one.
    pub dir: Option<PathBuf>,
    pub selected: Option<PathBuf>,
    pub scroll: f32,
}

/// The directory this run's command line named.
static LAUNCHED: OnceLock<Option<PathBuf>> = OnceLock::new();

fn path() -> PathBuf {
    cce_ui::config::cce_state_dir().join("files").join("state.json")
}

/// The place to open on, if this launch is the window reopening;
/// `launched` is the directory the command line named.
pub fn initial(launched: Option<&Path>) -> Option<Place> {
    let _ = LAUNCHED.set(launched.map(Path::to_path_buf));
    let saved = std::fs::read_to_string(path()).ok().and_then(|s| serde_json::from_str::<Saved>(&s).ok());
    choose(launched, saved, Path::is_dir)
}

fn choose(launched: Option<&Path>, saved: Option<Saved>, is_dir: impl Fn(&Path) -> bool) -> Option<Place> {
    let saved = saved.filter(|s| launched.is_none() || s.launched.as_deref() == launched)?;
    let page = Page::ALL.into_iter().find(|p| p.label() == saved.page).unwrap_or(Page::Browse);
    // A directory that is gone takes its row and offset with it.
    let Some(dir) = saved.dir.filter(|d| is_dir(d)) else {
        return Some(Place { page, dir: None, selected: None, scroll: 0.0 });
    };
    let scroll = if saved.scroll.is_finite() { saved.scroll.max(0.0) } else { 0.0 };
    Some(Place { page, dir: Some(dir), selected: saved.selected, scroll })
}

/// Keep where the window is for the next launch.
pub fn save(page: Page, dir: &Path, selected: Option<&Path>, scroll: f32) {
    let saved = Saved {
        page: page.label().to_string(),
        dir: Some(dir.to_path_buf()),
        selected: selected.map(Path::to_path_buf),
        scroll,
        launched: LAUNCHED.get().cloned().flatten(),
    };
    let path = path();
    let Some(parent) = path.parent() else { return };
    let tmp = path.with_extension("json.tmp");
    let written = std::fs::create_dir_all(parent)
        .and_then(|()| std::fs::write(&tmp, serde_json::to_vec(&saved).unwrap_or_default()))
        .and_then(|()| std::fs::rename(&tmp, &path));
    if let Err(e) = written {
        log::warn!("cannot save the window's place to {}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved(page: Page, dir: &str, launched: Option<&str>) -> Saved {
        Saved {
            page: page.label().into(),
            dir: Some(dir.into()),
            selected: Some(Path::new(dir).join("a.txt")),
            scroll: 120.0,
            launched: launched.map(Into::into),
        }
    }

    fn exists(p: &Path) -> bool {
        p != Path::new("/gone")
    }

    fn place(page: Page, dir: &str) -> Option<Place> {
        Some(Place { page, dir: Some(dir.into()), selected: Some(Path::new(dir).join("a.txt")), scroll: 120.0 })
    }

    #[test]
    fn the_same_command_line_or_none_reopens_where_it_was() {
        // Plain launch, plain relaunch.
        assert_eq!(choose(None, Some(saved(Page::Space, "/d", None)), exists), place(Page::Space, "/d"));
        // Launched on /tmp, moved on, restarted with the same argv: where it was, not /tmp.
        let tmp = Some(Path::new("/tmp"));
        assert_eq!(choose(tmp, Some(saved(Page::Network, "/d", Some("/tmp"))), exists), place(Page::Network, "/d"));
        // Launched on /tmp, reopened plainly.
        assert_eq!(choose(None, Some(saved(Page::Browse, "/d", Some("/tmp"))), exists), place(Page::Browse, "/d"));
    }

    #[test]
    fn a_command_line_naming_another_directory_wins() {
        let other = Some(Path::new("/other"));
        assert_eq!(choose(other, Some(saved(Page::Space, "/d", Some("/tmp"))), exists), None);
        assert_eq!(choose(other, Some(saved(Page::Space, "/d", None)), exists), None);
        // Nothing saved.
        assert_eq!(choose(None, None, exists), None);
        assert_eq!(choose(other, None, exists), None);
    }

    #[test]
    fn what_no_longer_exists_is_skipped() {
        // The directory is gone: the page comes back, the row and offset do not.
        let gone = Place { page: Page::Space, dir: None, selected: None, scroll: 0.0 };
        assert_eq!(choose(None, Some(saved(Page::Space, "/gone", None)), exists), Some(gone));
        // A page this build does not have: Browse.
        let mut odd = saved(Page::Space, "/d", None);
        odd.page = "Timeline".into();
        assert_eq!(choose(None, Some(odd), exists), place(Page::Browse, "/d"));
    }
}
