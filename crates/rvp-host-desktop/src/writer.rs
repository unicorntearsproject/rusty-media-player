//! Replacing a library file on disk (the tag editor): written to a temporary file beside it, flushed to disk, given the old file's
//! permissions, then renamed over it. The rename is atomic, so the file is either the old one or the whole new one, never half of
//! either; on any error the temporary file is removed and the original is untouched.
use crate::walk::root_path;
use rvp_host::FileWriter;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

/// The desktop's [`FileWriter`]: it writes at once, and the answer is waiting for the next poll.
#[derive(Default)]
pub struct DesktopWriter {
    answers: HashMap<u32, Result<(), String>>,
    next: u32,
}

/// `root` + `path` as a file path, refusing anything that could leave the root.
fn resolve(root: &str, path: &str) -> Result<PathBuf, String> {
    let base = root_path(root).ok_or("this folder is not one the app can write to")?;
    let rel = Path::new(path);
    if path.is_empty() || rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err("that path is not inside the folder".into());
    }
    Ok(base.join(rel))
}

/// Replace `target` with `data`, atomically.
pub fn replace_file(target: &Path, data: &[u8]) -> Result<(), String> {
    let meta = std::fs::metadata(target).map_err(|e| format!("the file cannot be found ({e})"))?;
    if !meta.is_file() {
        return Err("it is not a file".into());
    }
    let dir = target.parent().ok_or("the file has no folder")?;
    let name = target.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let tmp = dir.join(format!(".{name}.rvp-{}.tmp", std::process::id()));
    let result = (|| -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        drop(f);
        std::fs::set_permissions(&tmp, meta.permissions())?;
        std::fs::rename(&tmp, target)
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(match e.kind() {
            std::io::ErrorKind::PermissionDenied => "the folder or the file is read-only".to_string(),
            _ => format!("writing failed ({e})"),
        });
    }
    // Keep the change on disk before saying it is done (the folder entry of the rename).
    #[cfg(unix)]
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

impl FileWriter for DesktopWriter {
    fn can_write(&mut self, root: &str) -> Result<(), String> {
        let dir = root_path(root).ok_or("this folder is not one the app can write to")?;
        match std::fs::metadata(&dir) {
            Ok(m) if m.is_dir() => Ok(()),
            _ => Err("the folder is not there right now".into()),
        }
    }

    fn write(&mut self, root: &str, path: &str, data: Vec<u8>) -> u32 {
        let answer = resolve(root, path).and_then(|p| replace_file(&p, &data));
        self.next += 1;
        self.answers.insert(self.next, answer);
        self.next
    }

    fn poll_write(&mut self, ticket: u32) -> Option<Result<(), String>> {
        self.answers.remove(&ticket)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rvp-writer-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(d.join("sub")).unwrap();
        d
    }

    #[test]
    fn replaces_a_file_whole_keeps_its_permissions_and_leaves_no_temporary_file() {
        let d = tmpdir("ok");
        let f = d.join("sub/a.mp3");
        std::fs::write(&f, b"old").unwrap();
        let mut w = DesktopWriter::default();
        let root = crate::walk::root_id(&d);
        assert!(w.can_write(&root).is_ok());
        let t = w.write(&root, "sub/a.mp3", b"new bytes".to_vec());
        assert_eq!(w.poll_write(t), Some(Ok(())));
        assert_eq!(w.poll_write(t), None, "an answer is taken once");
        assert_eq!(std::fs::read(&f).unwrap(), b"new bytes");
        let left: Vec<_> =
            std::fs::read_dir(d.join("sub")).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left, ["a.mp3"], "no temporary file is left");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o640)).unwrap();
            let t = w.write(&root, "sub/a.mp3", b"again".to_vec());
            assert_eq!(w.poll_write(t), Some(Ok(())));
            assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o777, 0o640);
        }
    }

    #[test]
    fn refuses_paths_that_leave_the_folder_and_files_that_are_not_there() {
        let d = tmpdir("bad");
        std::fs::write(d.join("keep.txt"), b"keep").unwrap();
        let mut w = DesktopWriter::default();
        let root = crate::walk::root_id(&d.join("sub"));
        for bad in ["../keep.txt", "/etc/passwd", "", "a/../../keep.txt", "."] {
            let t = w.write(&root, bad, b"x".to_vec());
            assert!(w.poll_write(t).unwrap().is_err(), "{bad:?}");
        }
        assert_eq!(std::fs::read(d.join("keep.txt")).unwrap(), b"keep");
        // A file that does not exist is not created.
        let t = w.write(&root, "new.mp3", b"x".to_vec());
        assert!(w.poll_write(t).unwrap().is_err());
        assert!(!d.join("sub/new.mp3").exists());
        // A folder id that is not ours.
        assert!(w.can_write("root-123").is_err());
        let t = w.write("root-123", "a.mp3", vec![]);
        assert!(w.poll_write(t).unwrap().is_err());
        assert!(w.can_write(&crate::walk::root_id(&d.join("missing"))).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_folder_fails_cleanly_and_the_original_is_untouched() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmpdir("ro");
        let f = d.join("sub/a.mp3");
        std::fs::write(&f, b"old").unwrap();
        std::fs::set_permissions(d.join("sub"), std::fs::Permissions::from_mode(0o555)).unwrap();
        // Running as root ignores the permission; then there is nothing to test.
        if std::fs::write(d.join("sub/probe"), b"").is_ok() {
            return;
        }
        let mut w = DesktopWriter::default();
        let t = w.write(&crate::walk::root_id(&d), "sub/a.mp3", b"new".to_vec());
        let e = w.poll_write(t).unwrap().unwrap_err();
        assert!(e.contains("read-only"), "{e}");
        assert_eq!(std::fs::read(&f).unwrap(), b"old");
        std::fs::set_permissions(d.join("sub"), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}
