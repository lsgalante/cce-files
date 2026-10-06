//! Recursive directory scan feeding the Space (treemap) view.
//!
//! Unlike `read_directory_internal`, which reads one level and reports each
//! entry's own size, this walks the whole subtree and gives every directory
//! the sum of what it contains — the number a treemap's area encodes. It runs
//! on the `FsService` thread like every other request; the app sees it only as
//! progress messages followed by a completed tree.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
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
    /// Mount points the walk may descend into although they sit on another
    /// device than their parent — see [`crossable`]. Any other change of
    /// device is skipped, so scanning `/` does not wander into `/proc`, `/sys`,
    /// a tmpfs, or a backup drive.
    crossable: HashSet<PathBuf>,
    /// Every directory entered, by (device, inode). Directories have no hard
    /// links, so a second sighting is a bind mount: skipped, it is counted once
    /// and cannot loop the walk back into itself.
    seen: HashSet<(u64, u64)>,
    cancel: &'a AtomicBool,
    files: u64,
    dirs: u64,
    bytes: u64,
    on_progress: &'a mut dyn FnMut(u64, u64),
    last_report: Instant,
}

impl Walker<'_> {
    /// `dev` is the device `dir` lies on; a child on any other is a mount.
    fn walk(&mut self, dir: &Path, name: String, depth: u32, dev: u64) -> TreeNode {
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
            if ft.is_symlink() {
                continue;
            }

            let child_name = entry.file_name().to_string_lossy().into_owned();
            if ft.is_dir() {
                let path = entry.path();
                if meta.dev() != dev && !self.crossable.contains(&path) {
                    continue;
                }
                if !self.seen.insert((meta.dev(), meta.ino())) {
                    continue;
                }
                self.dirs += 1;
                let child = self.walk(&path, child_name, depth + 1, meta.dev());
                node.size += child.size;
                node.children.push(child);
            } else if ft.is_file() && meta.dev() == dev {
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
            // are dropped entirely, as is a file bind-mounted from elsewhere.
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

/// One line of `/proc/self/mountinfo`, as much as the walk needs.
#[derive(Debug, Clone, PartialEq)]
struct Mount {
    point: PathBuf,
    /// What is mounted: a device path for a disk filesystem, a bare word
    /// (`tmpfs`, `proc`) or `host:/path` for anything else.
    source: String,
}

/// Parse mountinfo: `id parent maj:min root POINT opts [optional...] - fstype SOURCE superopts`.
/// The optional fields vary in number, so the source is found after the ` - `.
fn parse_mountinfo(text: &str) -> Vec<Mount> {
    text.lines()
        .filter_map(|line| {
            let (left, right) = line.split_once(" - ")?;
            let point = left.split(' ').nth(4)?;
            let source = right.split(' ').nth(1)?;
            Some(Mount { point: unescape_mount(point), source: unescape_mount(source).to_string_lossy().into_owned() })
        })
        .collect()
}

/// mountinfo writes a space, tab, newline or backslash in a path as a
/// three-digit octal escape (`\040`).
fn unescape_mount(s: &str) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
            out.push((b[i + 1] - b'0') * 64 + (b[i + 2] - b'0') * 8 + (b[i + 3] - b'0'));
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    PathBuf::from(std::ffi::OsString::from_vec(out))
}

/// The disk a mount source lives on: the device itself, a partition's
/// parent, and through device-mapper (LUKS, LVM) the disk underneath.
/// `None` for anything that is not a block device — tmpfs, proc, a share.
fn disk_of(source: &str) -> Option<String> {
    if !source.starts_with("/dev/") {
        return None;
    }
    let dev = std::fs::canonicalize(source).ok()?;
    disk_of_block(dev.file_name()?.to_str()?, 0)
}

fn disk_of_block(name: &str, depth: u32) -> Option<String> {
    let sys = Path::new("/sys/class/block").join(name);
    if !sys.exists() {
        return None;
    }
    // A mapped device (dm-N) names what it is built on under `slaves/`.
    if depth < 8 {
        let slave = std::fs::read_dir(sys.join("slaves")).ok().and_then(|mut rd| rd.next()).and_then(|e| e.ok());
        if let Some(slave) = slave {
            return disk_of_block(&slave.file_name().to_string_lossy(), depth + 1);
        }
    }
    // A partition's sysfs node sits inside its disk's.
    if sys.join("partition").exists() {
        let real = std::fs::canonicalize(&sys).ok()?;
        return Some(real.parent()?.file_name()?.to_string_lossy().into_owned());
    }
    Some(name.to_string())
}

/// The mount points a scan of `root` may enter: every one whose topmost
/// mount is on the same disk as the mount `root` lies in.
///
/// Comparing devices alone stopped at every mount, and on a btrfs layout
/// (`@` at `/`, `@home` at `/home`) each subvolume is its own device: a scan
/// of `/` left out `/home`, most of the disk, and a separate `/home`
/// partition went the same way. Comparing disks keeps those and still keeps
/// out what the device check was for — `/proc`, `/sys`, tmpfs, network
/// shares, a backup drive. Nested btrfs subvolumes that are not mounted
/// (snapshots, container layers) have a device of their own and no entry
/// here, so they stay out, and a snapshot is not counted as a second copy.
fn crossable(mounts: &[Mount], root: &Path, disk_of: impl Fn(&str) -> Option<String>) -> HashSet<PathBuf> {
    // The last mount at a point is the one on top, the one the walk sees.
    let mut top: HashMap<&Path, &str> = HashMap::new();
    for m in mounts {
        top.insert(&m.point, &m.source);
    }
    let Some((home, home_src)) = top
        .iter()
        .filter(|(p, _)| root.starts_with(p))
        .max_by_key(|(p, _)| p.components().count())
    else {
        return HashSet::new();
    };
    let Some(disk) = disk_of(home_src) else {
        return HashSet::new();
    };
    let mut disks: HashMap<&str, Option<String>> = HashMap::new();
    top.iter()
        .filter(|(p, _)| p != &home)
        .filter(|(_, src)| disks.entry(src).or_insert_with(|| disk_of(src)).as_deref() == Some(disk.as_str()))
        .map(|(p, _)| p.to_path_buf())
        .collect()
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
    // Mount points are canonical, so the walk's paths must be too.
    let real = std::fs::canonicalize(root).ok()?;
    let mountinfo = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();

    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.to_string_lossy().into_owned());

    let mut walker = Walker {
        crossable: crossable(&parse_mountinfo(&mountinfo), &real, disk_of),
        seen: HashSet::from([(meta.dev(), meta.ino())]),
        cancel,
        files: 0,
        dirs: 0,
        bytes: 0,
        on_progress,
        last_report: Instant::now(),
    };
    let tree = walker.walk(&real, name, 0, meta.dev());

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
    fn mountinfo_parses_past_optional_fields_and_escapes() {
        let text = "\
29 1 0:31 /@ / rw,relatime shared:1 - btrfs /dev/nvme0n1p2 rw,ssd
40 29 0:50 /@home /home rw,relatime shared:2 master:7 - btrfs /dev/nvme0n1p2 rw
41 29 259:1 / /mnt/my\\040drive rw - ext4 /dev/sdb1 rw
";
        let m = parse_mountinfo(text);
        assert_eq!(m.len(), 3);
        assert_eq!(m[1], Mount { point: "/home".into(), source: "/dev/nvme0n1p2".into() });
        assert_eq!(m[2].point, PathBuf::from("/mnt/my drive"));
    }

    #[test]
    fn mounts_on_the_scanned_disk_are_crossed_and_nothing_else() {
        let mount = |p: &str, s: &str| Mount { point: p.into(), source: s.into() };
        let mounts = vec![
            mount("/", "/dev/nvme0n1p2"),
            mount("/home", "/dev/nvme0n1p2"),        // btrfs subvolume
            mount("/boot", "/dev/nvme0n1p1"),        // another partition, same disk
            mount("/mnt/backup", "/dev/sdb1"),       // another disk
            mount("/tmp", "tmpfs"),
            mount("/proc", "proc"),
            mount("/srv/share", "nas:/export"),
            mount("/home/me/cache", "/dev/nvme0n1p2"),
            mount("/home/me/cache", "tmpfs"),        // stacked on top: the walk sees this
        ];
        let disk = |src: &str| match src {
            "/dev/nvme0n1p1" | "/dev/nvme0n1p2" => Some("nvme0n1".to_string()),
            "/dev/sdb1" => Some("sdb".to_string()),
            _ => None,
        };

        let got = crossable(&mounts, Path::new("/"), disk);
        let want: HashSet<PathBuf> = ["/home", "/boot"].iter().map(PathBuf::from).collect();
        assert_eq!(got, want);

        // Scanning inside the backup drive crosses nothing on the system disk.
        assert!(crossable(&mounts, Path::new("/mnt/backup/x"), disk).is_empty());
        // Nor does a scan rooted on a tmpfs.
        assert!(crossable(&mounts, Path::new("/tmp/x"), disk).is_empty());
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
