//! The freedesktop.org Trash spec (v1.0), home-trash only: items move into
//! `$XDG_DATA_HOME/Trash/files/` (else `~/.local/share/Trash/files/`) and each
//! carries an `info/<name>.trashinfo` recording its origin and deletion date —
//! so gio, trash-cli, and the KDE/GNOME file managers all see the same trash.
//!
//! All functions do blocking IO; they run on the FsService task like the rest
//! of `services::fs`.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// `$XDG_DATA_HOME/Trash`, else `~/.local/share/Trash`. None only when HOME
/// is unset.
pub fn trash_dir() -> Option<PathBuf> {
    if let Some(data) = std::env::var_os("XDG_DATA_HOME") {
        let data = PathBuf::from(data);
        if data.is_absolute() {
            return Some(data.join("Trash"));
        }
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share/Trash"))
}

/// The directory trashed items live in — what the browse page navigates to.
pub fn files_dir() -> Option<PathBuf> {
    trash_dir().map(|t| t.join("files"))
}

/// Whether `dir` IS the trash files directory (the browse page's gate for
/// swapping Delete → Restore/Delete Permanently in the row menu).
pub fn is_trash_files_dir(dir: &Path) -> bool {
    files_dir().is_some_and(|f| f == dir)
}

/// Move `path` into the trash: reserve a unique name by exclusively creating
/// its .trashinfo, then rename (copy+delete across filesystems).
pub fn move_to_trash(path: &Path) -> io::Result<()> {
    let base = trash_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no HOME — trash unavailable"))?;
    move_to_trash_in(&base, path)
}

/// Restore a trashed item (a path under `files/`) to the origin its
/// .trashinfo records. Refuses to overwrite an existing file at the origin.
/// Returns the restored path.
pub fn restore(trashed: &Path) -> io::Result<PathBuf> {
    let base = trash_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no HOME — trash unavailable"))?;
    restore_in(&base, trashed)
}

/// Delete everything in the trash permanently. Returns how many items went.
pub fn empty() -> io::Result<usize> {
    let base = trash_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no HOME — trash unavailable"))?;
    empty_in(&base)
}

fn move_to_trash_in(base: &Path, path: &Path) -> io::Result<()> {
    let files = base.join("files");
    let info = base.join("info");
    fs::create_dir_all(&files)?;
    fs::create_dir_all(&info)?;

    let orig = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let name = orig
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?
        .to_string_lossy()
        .to_string();

    // Spec: DeletionDate is local time, no zone suffix.
    let deleted_at = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string();
    let body = format!(
        "[Trash Info]\nPath={}\nDeletionDate={}\n",
        percent_encode(&orig.to_string_lossy()),
        deleted_at
    );

    // Reserve the trash name by O_EXCL-creating the info file: `name`, then
    // `name.2`, `name.3`, … — the create-race is the spec's uniqueness lock.
    let mut n = 1u32;
    loop {
        let candidate = if n == 1 { name.clone() } else { format!("{name}.{n}") };
        let info_path = info.join(format!("{candidate}.trashinfo"));
        match fs::OpenOptions::new().write(true).create_new(true).open(&info_path) {
            Ok(mut f) => {
                f.write_all(body.as_bytes())?;
                let target = files.join(&candidate);
                if let Err(e) = rename_or_copy(&orig, &target) {
                    let _ = fs::remove_file(&info_path);
                    return Err(e);
                }
                return Ok(());
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                n += 1;
                if n > 10_000 {
                    return Err(io::Error::new(io::ErrorKind::AlreadyExists, "no free trash name"));
                }
            }
            Err(e) => return Err(e),
        }
    }
}

/// The origin a trashed item (a path under `files/`) restores to, from its
/// .trashinfo — the trash listing shows this. None when the sidecar is
/// missing or malformed.
pub fn origin_of(trashed: &Path) -> Option<PathBuf> {
    let base = trash_dir()?;
    let name = trashed.file_name()?.to_string_lossy().to_string();
    read_origin(&base.join("info").join(format!("{name}.trashinfo"))).ok()
}

fn read_origin(info_path: &Path) -> io::Result<PathBuf> {
    let body = fs::read_to_string(info_path)?;
    let encoded = body
        .lines()
        .find_map(|l| l.strip_prefix("Path="))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "trashinfo has no Path"))?;
    Ok(PathBuf::from(percent_decode(encoded)))
}

fn restore_in(base: &Path, trashed: &Path) -> io::Result<PathBuf> {
    let name = trashed
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?
        .to_string_lossy()
        .to_string();
    let info_path = base.join("info").join(format!("{name}.trashinfo"));
    let origin = read_origin(&info_path)?;

    if origin.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} already exists", origin.display()),
        ));
    }
    if let Some(parent) = origin.parent() {
        fs::create_dir_all(parent)?;
    }
    rename_or_copy(trashed, &origin)?;
    fs::remove_file(&info_path)?;
    Ok(origin)
}

fn empty_in(base: &Path) -> io::Result<usize> {
    let mut count = 0usize;
    let files = base.join("files");
    if let Ok(entries) = fs::read_dir(&files) {
        for entry in entries.flatten() {
            let p = entry.path();
            let removed = if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                fs::remove_dir_all(&p)
            } else {
                fs::remove_file(&p)
            };
            if removed.is_ok() {
                count += 1;
            }
        }
    }
    if let Ok(entries) = fs::read_dir(base.join("info")) {
        for entry in entries.flatten() {
            let _ = fs::remove_file(entry.path());
        }
    }
    Ok(count)
}

/// Rename, falling back to a recursive copy + delete when source and trash
/// live on different filesystems (EXDEV).
fn rename_or_copy(from: &Path, to: &Path) -> io::Result<()> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(libc_exdev()) => {
            copy_recursive(from, to)?;
            if from.is_dir() {
                fs::remove_dir_all(from)
            } else {
                fs::remove_file(from)
            }
        }
        Err(e) => Err(e),
    }
}

const fn libc_exdev() -> i32 {
    18 // EXDEV on Linux
}

fn copy_recursive(from: &Path, to: &Path) -> io::Result<()> {
    if from.is_dir() {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(from, to).map(|_| ())
    }
}

/// Percent-encode a path for a trashinfo `Path=` line: unreserved characters
/// and `/` pass through, everything else (spaces, non-ASCII bytes, …) encodes.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let (Some(h), Some(l)) = (
                bytes.get(i + 1).and_then(|b| (*b as char).to_digit(16)),
                bytes.get(i + 2).and_then(|b| (*b as char).to_digit(16)),
            ) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_base(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("cce-files-trash-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn test_percent_roundtrip() {
        let s = "/home/user/My Files/café ünï.txt";
        let enc = percent_encode(s);
        assert!(!enc.contains(' '));
        assert_eq!(percent_decode(&enc), s);
    }

    #[test]
    fn test_trash_restore_roundtrip() {
        let base = temp_base("roundtrip");
        let trash = base.join("Trash");
        let victim = base.join("doomed file.txt");
        fs::write(&victim, b"contents").unwrap();

        move_to_trash_in(&trash, &victim).unwrap();
        assert!(!victim.exists());
        let trashed = trash.join("files/doomed file.txt");
        assert!(trashed.exists());
        let info = fs::read_to_string(trash.join("info/doomed file.txt.trashinfo")).unwrap();
        assert!(info.starts_with("[Trash Info]\n"));
        assert!(info.contains("DeletionDate="));

        let restored = restore_in(&trash, &trashed).unwrap();
        assert_eq!(restored, victim.canonicalize().unwrap_or(victim.clone()));
        assert_eq!(fs::read(&victim).unwrap(), b"contents");
        assert!(!trashed.exists());
        assert!(!trash.join("info/doomed file.txt.trashinfo").exists());

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_name_collision_and_empty() {
        let base = temp_base("collision");
        let trash = base.join("Trash");
        for _ in 0..3 {
            let victim = base.join("dup.txt");
            fs::write(&victim, b"x").unwrap();
            move_to_trash_in(&trash, &victim).unwrap();
        }
        assert!(trash.join("files/dup.txt").exists());
        assert!(trash.join("files/dup.txt.2").exists());
        assert!(trash.join("files/dup.txt.3").exists());

        // A directory victim exercises the recursive branch of empty.
        let dir_victim = base.join("nested");
        fs::create_dir_all(dir_victim.join("inner")).unwrap();
        fs::write(dir_victim.join("inner/f.txt"), b"y").unwrap();
        move_to_trash_in(&trash, &dir_victim).unwrap();

        assert_eq!(empty_in(&trash).unwrap(), 4);
        assert_eq!(fs::read_dir(trash.join("files")).unwrap().count(), 0);
        assert_eq!(fs::read_dir(trash.join("info")).unwrap().count(), 0);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_restore_refuses_overwrite() {
        let base = temp_base("overwrite");
        let trash = base.join("Trash");
        let victim = base.join("clash.txt");
        fs::write(&victim, b"old").unwrap();
        move_to_trash_in(&trash, &victim).unwrap();
        fs::write(&victim, b"new").unwrap();

        let err = restore_in(&trash, &trash.join("files/clash.txt")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&victim).unwrap(), b"new");

        let _ = fs::remove_dir_all(&base);
    }
}
