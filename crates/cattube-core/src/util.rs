//! Small helpers: random ids, URL encoding, grid movement.

use crate::input::Action;
use std::io::{self, Read};

pub fn random_hex(n: usize) -> io::Result<String> {
    let mut f = std::fs::File::open("/dev/urandom")?;
    let mut buf = vec![0u8; n];
    f.read_exact(&mut buf)?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

/// Write a file only its owner can read (it holds a sign-in or a pairing),
/// in one step: a reader never sees half a file.
pub fn write_private(path: &std::path::Path, text: &str) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(text.as_bytes())?;
    }
    std::fs::rename(tmp, path)
}

/// Make an existing file private (files written before
/// [`write_private`] was used).
pub fn make_private(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(m) = std::fs::metadata(path)
        && m.permissions().mode() & 0o077 != 0
    {
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
}

/// Percent-encode everything but unreserved characters (RFC 3986).
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Move inside a grid of `n` items, `cols` wide. `None`: Up from the first
/// row (leave the grid).
pub fn grid_move(i: usize, action: Action, cols: usize, n: usize) -> Option<usize> {
    if n == 0 {
        return if action == Action::Up { None } else { Some(0) };
    }
    Some(match action {
        Action::Left if !i.is_multiple_of(cols) => i - 1,
        Action::Right if i % cols < cols - 1 && i + 1 < n => i + 1,
        Action::Up if i >= cols => i - cols,
        Action::Up => return None,
        Action::Down if i + cols < n => i + cols,
        // Last partial row: Down from above moves to the last item.
        Action::Down if i / cols < (n - 1) / cols => n - 1,
        _ => i,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn private_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("cattube-private-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("remote.conf");
        std::fs::write(&p, "old").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        super::make_private(&p);
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        super::write_private(&p, "screen = x\n").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "screen = x\n");
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    use super::*;

    #[test]
    fn encoding_and_grid() {
        assert_eq!(percent_encode("a b/é"), "a%20b%2F%C3%A9");
        assert_eq!(grid_move(0, Action::Right, 3, 5), Some(1));
        assert_eq!(grid_move(1, Action::Down, 3, 5), Some(4));
        assert_eq!(grid_move(2, Action::Down, 3, 5), Some(4));
        assert_eq!(grid_move(1, Action::Up, 3, 5), None);
        assert_eq!(random_hex(4).unwrap().len(), 8);
    }
}
