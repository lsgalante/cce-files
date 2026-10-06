//! Small formatting helpers shared across pages and services.

/// Format a byte count as a short human-readable size (e.g. `1.5 M`).
pub fn format_size(size: u64) -> String {
    if size < 1024 {
        format!("{} B", size)
    } else if size < 1024 * 1024 {
        format!("{:.1} K", size as f64 / 1024.0)
    } else if size < 1024 * 1024 * 1024 {
        format!("{:.1} M", size as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} G", size as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

/// Format a Unix mode into a 10-character `drwxr-xr-x`-style permission string.
pub fn format_permissions(mode: u32) -> String {
    let mut s = String::with_capacity(10);
    s.push(if mode & 0o40000 != 0 { 'd' } else { '-' });
    s.push(if mode & 0o400 != 0 { 'r' } else { '-' });
    s.push(if mode & 0o200 != 0 { 'w' } else { '-' });
    s.push(if mode & 0o100 != 0 { 'x' } else { '-' });
    s.push(if mode & 0o040 != 0 { 'r' } else { '-' });
    s.push(if mode & 0o020 != 0 { 'w' } else { '-' });
    s.push(if mode & 0o010 != 0 { 'x' } else { '-' });
    s.push(if mode & 0o004 != 0 { 'r' } else { '-' });
    s.push(if mode & 0o002 != 0 { 'w' } else { '-' });
    s.push(if mode & 0o001 != 0 { 'x' } else { '-' });
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_size_units() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1024), "1.0 K");
        assert_eq!(format_size(1048576), "1.0 M");
        assert_eq!(format_size(1073741824), "1.0 G");
    }

    #[test]
    fn format_permissions_string() {
        assert_eq!(format_permissions(0o40755), "drwxr-xr-x");
        assert_eq!(format_permissions(0o100644), "-rw-r--r--");
        assert_eq!(format_permissions(0o644), "-rw-r--r--");
    }
}

/// Truncate `s` to fit `avail` px, measured for real (resvg-backed, cached per
/// string+size — the handful of details strings re-measure only on selection
/// change). `head` replaces the front ("...ail/of/path"), else the back
/// ("name..."). Binary search on kept chars: ~7 probes for a long path.
pub fn truncate_px(s: &str, family: &str, size: f32, avail: f32, head: bool) -> String {
    if cce_ui::widget::display::measure_text_width(s, family, size) <= avail {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let build = |keep: usize| -> String {
        if head {
            let tail: String = chars[chars.len() - keep..].iter().collect();
            format!("...{tail}")
        } else {
            let kept: String = chars[..keep].iter().collect();
            format!("{kept}...")
        }
    };
    let (mut lo, mut hi) = (0usize, chars.len().saturating_sub(1));
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        if cce_ui::widget::display::measure_text_width(&build(mid), family, size) <= avail {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    build(lo)
}
