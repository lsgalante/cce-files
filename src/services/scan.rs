//! Recursive directory scan feeding the Space (treemap) view.
//!
//! Unlike `read_directory_internal`, which reads one level and reports each
//! entry's own size, this walks the whole subtree and gives every directory
//! the sum of what it contains — the number a treemap's area encodes. It runs
//! on the `FsService` thread like every other request; the app sees it only as
//! progress messages followed by a completed tree.

use std::path::Path;
use std::os::unix::fs::MetadataExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Deepest nesting the walk will descend. Ordinary trees are nowhere near
/// this; the cap exists so a pathological one cannot overflow the recursion
/// stack.
const MAX_DEPTH: u32 = 64;

/// How often a scan in flight reports what it has counted so far. Short
/// enough that the progress line moves, long enough that a fast tree is not
/// mostly channel traffic.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(150);

/// One node of a scanned tree. `size` is the recursive total for a directory
/// and the apparent size for a file.
///
/// Nodes deliberately carry no `PathBuf` — a large tree is millions of nodes,
/// and only the few thousand tiles that survive layout culling ever need a
/// path. The Space page rebuilds those from the names along the way down.
#[derive(Debug, Clone, Default)]
pub struct TreeNode {
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
    /// Sorted descending by `size` — the order the squarified layout wants,
    /// established once here rather than per frame.
    pub children: Vec<TreeNode>,
}

#[derive(Debug, Clone, Default)]
pub struct ScanResult {
    pub tree: TreeNode,
    pub files: u64,
    pub dirs: u64,
    /// True when the scan stopped early because its cancel flag was raised —
    /// the tree is a partial one and should be discarded, not drawn.
    pub cancelled: bool,
}

struct Walker<'a> {
    /// Device of the scan root. Entries on any other device are skipped, so
    /// scanning `/` does not wander into `/proc`, `/sys`, or a mounted backup
    /// drive — and cannot loop through a bind mount pointing back inside.
    dev: u64,
    cancel: &'a AtomicBool,
    files: u64,
    dirs: u64,
    bytes: u64,
    on_progress: &'a mut dyn FnMut(u64, u64),
    last_report: Instant,
}

impl Walker<'_> {
    fn walk(&mut self, dir: &Path, name: String, depth: u32) -> TreeNode {
        let mut node = TreeNode { name, size: 0, is_dir: true, children: Vec::new() };
        if depth >= MAX_DEPTH || self.cancel.load(Ordering::Relaxed) {
            return node;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            // Unreadable directory (permissions, races) contributes nothing
            // rather than aborting the scan around it.
            return node;
        };

        for entry in rd.filter_map(|e| e.ok()) {
            if self.cancel.load(Ordering::Relaxed) {
                break;
            }
            // `DirEntry::metadata` does not traverse symlinks, which is what we
            // want twice over: a link cannot pull its target's bytes into this
            // subtree's total, and cannot loop the walk back into itself.
            let Ok(meta) = entry.metadata() else { continue };
            let ft = meta.file_type();
            if ft.is_symlink() || meta.dev() != self.dev {
                continue;
            }

            let child_name = entry.file_name().to_string_lossy().into_owned();
            if ft.is_dir() {
                self.dirs += 1;
                let child = self.walk(&entry.path(), child_name, depth + 1);
                node.size += child.size;
                node.children.push(child);
            } else if ft.is_file() {
                let size = meta.len();
                self.files += 1;
                self.bytes += size;
                node.size += size;
                node.children.push(TreeNode {
                    name: child_name,
                    size,
                    is_dir: false,
                    children: Vec::new(),
                });
            }
            // Sockets, fifos, and device nodes occupy no meaningful space and
            // are dropped entirely.
            self.maybe_report();
        }

        node.children.sort_unstable_by(|a, b| b.size.cmp(&a.size));
        node
    }

    fn maybe_report(&mut self) {
        if self.last_report.elapsed() >= PROGRESS_INTERVAL {
            self.last_report = Instant::now();
            (self.on_progress)(self.files, self.bytes);
        }
    }
}

/// Walk `root`, returning its tree with directory sizes summed.
///
/// Returns `None` when `root` is not a readable directory. `cancel` is polled
/// per entry, so a superseded scan stops within a directory rather than
/// running to completion unwatched.
pub fn scan(
    root: &Path,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(u64, u64),
) -> Option<ScanResult> {
    // The root is stat'd through symlinks — the user may well have navigated
    // to one — while everything beneath it is not.
    let meta = std::fs::metadata(root).ok()?;
    if !meta.is_dir() {
        return None;
    }

    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.to_string_lossy().into_owned());

    let mut walker = Walker {
        dev: meta.dev(),
        cancel,
        files: 0,
        dirs: 0,
        bytes: 0,
        on_progress,
        last_report: Instant::now(),
    };
    let tree = walker.walk(root, name, 0);

    Some(ScanResult {
        tree,
        files: walker.files,
        dirs: walker.dirs,
        cancelled: cancel.load(Ordering::Relaxed),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A scratch tree: `<tmp>/cce_scan_test_<nanos>/`.
    fn scratch(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("cce_scan_test_{tag}_{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn sums_sizes_recursively_and_sorts_descending() {
        let root = scratch("sum");
        fs::write(root.join("small.txt"), vec![b'a'; 10]).unwrap();
        let sub = root.join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("big.bin"), vec![b'b'; 5000]).unwrap();
        fs::write(sub.join("mid.bin"), vec![b'c'; 500]).unwrap();

        let cancel = AtomicBool::new(false);
        let res = scan(&root, &cancel, &mut |_, _| {}).unwrap();

        assert_eq!(res.files, 3);
        assert_eq!(res.dirs, 1);
        assert!(!res.cancelled);
        // The directory carries what it contains, not its own inode size.
        assert_eq!(res.tree.size, 5510);

        // Children sorted descending: sub (5500) before small.txt (10).
        assert_eq!(res.tree.children.len(), 2);
        assert_eq!(res.tree.children[0].name, "sub");
        assert_eq!(res.tree.children[0].size, 5500);
        assert!(res.tree.children[0].is_dir);
        assert_eq!(res.tree.children[1].name, "small.txt");

        // ...and so are the grandchildren.
        let sub_node = &res.tree.children[0];
        assert_eq!(sub_node.children[0].name, "big.bin");
        assert_eq!(sub_node.children[1].name, "mid.bin");

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn symlinks_are_skipped_not_followed() {
        let root = scratch("link");
        let real = root.join("real");
        fs::create_dir(&real).unwrap();
        fs::write(real.join("data.bin"), vec![b'x'; 1000]).unwrap();
        // A link back to the root would loop the walk if it were followed, and
        // a link to the sibling directory would double-count its bytes.
        std::os::unix::fs::symlink(&root, root.join("loop")).unwrap();
        std::os::unix::fs::symlink(&real, root.join("alias")).unwrap();

        let cancel = AtomicBool::new(false);
        let res = scan(&root, &cancel, &mut |_, _| {}).unwrap();

        assert_eq!(res.files, 1);
        assert_eq!(res.tree.size, 1000);
        assert_eq!(res.tree.children.len(), 1, "only `real` — both links dropped");

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn cancel_flag_stops_the_walk() {
        let root = scratch("cancel");
        for i in 0..50 {
            fs::write(root.join(format!("f{i}")), vec![b'z'; 100]).unwrap();
        }

        // Already-raised flag: the walk bails before reading any entry.
        let cancel = AtomicBool::new(true);
        let res = scan(&root, &cancel, &mut |_, _| {}).unwrap();

        assert!(res.cancelled);
        assert_eq!(res.files, 0);
        assert!(res.tree.children.is_empty());

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn non_directory_root_is_rejected() {
        let root = scratch("file");
        let file = root.join("plain.txt");
        fs::write(&file, b"hello").unwrap();

        let cancel = AtomicBool::new(false);
        assert!(scan(&file, &cancel, &mut |_, _| {}).is_none());

        fs::remove_dir_all(&root).unwrap();
    }
}
