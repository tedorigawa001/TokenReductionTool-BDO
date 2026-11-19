//! Bounded reads of small text files that a repository controls.
//!
//! A repository can ship a link to `/dev/zero` or to a file outside it, and a
//! working tree can hold a FIFO. A plain `read_to_string` then never returns,
//! fills memory, or copies the outside file into bdo's output. These reads
//! open without blocking, accept only regular files of at most
//! [`MAX_TEXT_BYTES`], and skip oversized files in full rather than reading a
//! truncated prefix.

use std::fs::{Metadata, OpenOptions};
use std::io::Read;
use std::path::Path;

/// Manifests, settings and source files are small; anything larger is skipped.
pub const MAX_TEXT_BYTES: u64 = 1024 * 1024;

/// Whether the last path component may be a link.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Links {
    /// Follow it: for the user's own configuration, which may be linked.
    Follow,
    /// Refuse it: for files a repository ships.
    Refuse,
}

/// UTF-8 text of a regular file of at most [`MAX_TEXT_BYTES`], or `None`.
pub fn read_text(path: &Path, links: Links) -> Option<String> {
    if links == Links::Refuse && !scannable(&std::fs::symlink_metadata(path).ok()?, links) {
        return None;
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Opening a FIFO must not wait for a writer; a link swapped in after
        // the check above must not be followed. The handle is checked again.
        let nofollow = if links == Links::Refuse {
            libc::O_NOFOLLOW
        } else {
            0
        };
        options.custom_flags(libc::O_NONBLOCK | nofollow);
    }
    #[cfg(windows)]
    if links == Links::Refuse {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    let file = options.open(path).ok()?;
    if !scannable(&file.metadata().ok()?, links) {
        return None;
    }
    read_limited_text(file)
}

fn scannable(metadata: &Metadata, links: Links) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if links == Links::Refuse && metadata.file_attributes() & 0x400 != 0 {
            return false; // FILE_ATTRIBUTE_REPARSE_POINT
        }
    }
    #[cfg(not(windows))]
    let _ = links;
    metadata.is_file() && metadata.len() <= MAX_TEXT_BYTES
}

/// Read at most one byte past the cap, so a file that grew after its size
/// check is still refused.
pub(crate) fn read_limited_text(input: impl Read) -> Option<String> {
    let mut content = String::new();
    input
        .take(MAX_TEXT_BYTES + 1)
        .read_to_string(&mut content)
        .ok()?;
    (content.len() as u64 <= MAX_TEXT_BYTES).then_some(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_size_boundary_and_invalid_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file");
        for links in [Links::Follow, Links::Refuse] {
            std::fs::write(&path, vec![b'a'; MAX_TEXT_BYTES as usize]).unwrap();
            assert_eq!(
                read_text(&path, links).unwrap().len(),
                MAX_TEXT_BYTES as usize
            );
            std::fs::write(&path, vec![b'a'; MAX_TEXT_BYTES as usize + 1]).unwrap();
            assert!(read_text(&path, links).is_none());
            std::fs::write(&path, [0xff]).unwrap();
            assert!(read_text(&path, links).is_none());
            assert!(read_text(&dir.path().join("missing"), links).is_none());
            assert!(read_text(dir.path(), links).is_none());
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_links_are_followed_only_when_asked() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        std::fs::write(&target, "linked text").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(
            read_text(&link, Links::Follow).as_deref(),
            Some("linked text")
        );
        assert!(read_text(&link, Links::Refuse).is_none());

        let device = dir.path().join("zero");
        std::os::unix::fs::symlink("/dev/zero", &device).unwrap();
        assert!(read_text(&device, Links::Follow).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn test_fifos_are_skipped_without_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("fifo");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(status.success());
        for links in [Links::Follow, Links::Refuse] {
            assert!(read_text(&fifo, links).is_none());
        }
    }

    #[test]
    fn test_limited_read_enforces_cap_independent_of_metadata() {
        // Represents a stream which has grown beyond its earlier size check.
        let bytes = vec![b'x'; MAX_TEXT_BYTES as usize + 4096];
        let mut input = std::io::Cursor::new(bytes);
        assert!(read_limited_text(&mut input).is_none());
        assert_eq!(input.position(), MAX_TEXT_BYTES + 1);
        assert_eq!(
            read_limited_text(&b"valid text\n"[..]).as_deref(),
            Some("valid text\n")
        );
    }
}
