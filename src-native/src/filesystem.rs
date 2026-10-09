//! Capability-checked guest filesystem. Guest paths never map directly to host paths.
use std::collections::{BTreeMap, BTreeSet};

pub const BADF: i32 = 8;
pub const EXIST: i32 = 20;
pub const INVAL: i32 = 28;
pub const ISDIR: i32 = 31;
pub const NOTDIR: i32 = 54;
pub const NOTEMPTY: i32 = 55;
pub const NOENT: i32 = 44;
pub const NOTCAPABLE: i32 = 76;
pub const LIMIT: usize = 1024 * 1024;
// WASI Preview 1 rights. libc intersects file-open rights with the preopen's
// inheriting mask; an empty mask silently produces unwritable file handles.
pub const FILE_RIGHTS: u64 = 2 | 4 | 8 | 64;
pub const DIRECTORY_RIGHTS: u64 = 512 | 1024 | 8192 | 262144 | 33554432 | 67108864;

pub struct Handle {
    pub path: String,
    pub offset: usize,
    pub rights: u64,
    pub flags: u16,
}
pub struct VirtualFs {
    pub files: BTreeMap<String, Vec<u8>>,
    pub handles: BTreeMap<u32, Handle>,
    pub directories: BTreeSet<String>,
    next: u32,
    #[cfg(all(windows, any(test, not(feature = "diagnostics"))))]
    pub profile: Option<std::sync::Arc<crate::profile::Profile>>,
}

impl Default for VirtualFs {
    fn default() -> Self {
        Self {
            files: BTreeMap::new(),
            handles: BTreeMap::new(),
            directories: BTreeSet::from(["/".into()]),
            next: 64,
            #[cfg(all(windows, any(test, not(feature = "diagnostics"))))]
            profile: None,
        }
    }
}

impl VirtualFs {
    pub fn flush(&self) -> anyhow::Result<()> {
        #[cfg(all(windows, any(test, not(feature = "diagnostics"))))]
        if let Some(profile) = &self.profile {
            profile.save(self)?;
        }
        Ok(())
    }

    pub fn remove_directory(&mut self, path: &[u8]) -> Result<(), i32> {
        let path = Self::path(path)?;
        if path == "/" {
            return Err(10);
        } // WASI BUSY: the mounted synthetic root.
        if self.files.contains_key(&path) {
            return Err(NOTDIR);
        }
        if !self.directories.contains(&path) {
            return Err(NOENT);
        }
        let prefix = format!("{path}/");
        if self.files.keys().any(|p| p.starts_with(&prefix))
            || self.directories.iter().any(|p| p.starts_with(&prefix))
        {
            return Err(NOTEMPTY);
        }
        self.directories.remove(&path);
        Ok(())
    }
    pub fn unlink(&mut self, path: &[u8]) -> Result<(), i32> {
        let path = Self::path(path)?;
        if self.directories.contains(&path) {
            return Err(ISDIR);
        }
        if !self.files.contains_key(&path) {
            return Err(NOENT);
        }
        // Preserve open descriptors after removing the name. NUL-containing
        // internal names are never addressable by a guest path, so recreating
        // the original name produces a different file with independent bytes.
        let opened = self.handles.values().any(|h| h.path == path);
        let orphan = if opened {
            let id = self.next;
            self.next = self.next.checked_add(1).ok_or(INVAL)?;
            Some(format!("\0unlinked:{id}"))
        } else {
            None
        };
        let bytes = self.files.remove(&path).ok_or(NOENT)?;
        if let Some(orphan) = orphan {
            for h in self.handles.values_mut().filter(|h| h.path == path) {
                h.path = orphan.clone();
            }
            self.files.insert(orphan, bytes);
        }
        Ok(())
    }

    pub fn close(&mut self, fd: u32) -> bool {
        let Some(handle) = self.handles.remove(&fd) else {
            return false;
        };
        if handle.path.starts_with('\0') && !self.handles.values().any(|h| h.path == handle.path) {
            self.files.remove(&handle.path);
        }
        true
    }
    pub fn path(path: &[u8]) -> Result<String, i32> {
        let path = std::str::from_utf8(path).map_err(|_| INVAL)?;
        if path.contains('\0') || path.contains('\\') {
            return Err(INVAL);
        }
        let mut parts = Vec::new();
        for p in path.split('/') {
            match p {
                "" | "." => (),
                ".." => return Err(NOTCAPABLE),
                _ => parts.push(p),
            }
        }
        Ok(format!("/{}", parts.join("/")))
    }

    pub fn open(&mut self, path: &[u8], oflags: u32, rights: u64, flags: u16) -> Result<u32, i32> {
        let path = Self::path(path)?;
        if flags & !1 != 0 || oflags & !15 != 0 {
            return Err(INVAL);
        }
        // Creation/truncation is authorized by the parent directory's path
        // capabilities, not FD_WRITE on the newly opened descriptor. In
        // particular, a guest may create its lock file with a read-only fd.
        if oflags & 2 != 0 || self.directories.contains(&path) {
            return Err(INVAL);
        }
        if self.files.contains_key(&path) {
            if oflags & 5 == 5 {
                return Err(EXIST);
            }
        } else if oflags & 1 != 0 {
            let parent = path
                .rsplit_once('/')
                .map(|(p, _)| if p.is_empty() { "/" } else { p })
                .ok_or(INVAL)?;
            if !self.directories.contains(parent) {
                return Err(NOENT);
            }
            if self.files.len() >= 32 {
                return Err(INVAL);
            }
            self.files.insert(path.clone(), Vec::new());
        } else {
            return Err(NOENT);
        }
        if oflags & 8 != 0 {
            self.files.get_mut(&path).unwrap().clear();
        }
        if self.handles.len() >= 64 {
            return Err(INVAL);
        }
        let fd = self.next;
        self.next = self.next.checked_add(1).ok_or(INVAL)?;
        self.handles.insert(
            fd,
            Handle {
                path,
                offset: 0,
                rights,
                flags,
            },
        );
        Ok(fd)
    }

    pub fn read(&mut self, fd: u32, len: usize) -> Result<Vec<u8>, i32> {
        if len > LIMIT {
            return Err(INVAL);
        }
        let h = self.handles.get_mut(&fd).ok_or(BADF)?;
        if h.rights & 2 == 0 {
            return Err(NOTCAPABLE);
        }
        let data = self.files.get(&h.path).ok_or(NOENT)?;
        let start = h.offset.min(data.len());
        let end = start.saturating_add(len).min(data.len());
        h.offset = end;
        Ok(data[start..end].to_vec())
    }

    pub fn write(&mut self, fd: u32, bytes: &[u8]) -> Result<(), i32> {
        let h = self.handles.get_mut(&fd).ok_or(BADF)?;
        if h.rights & 64 == 0 {
            return Err(NOTCAPABLE);
        }
        let data = self.files.get_mut(&h.path).ok_or(NOENT)?;
        if h.flags & 1 != 0 {
            h.offset = data.len();
        }
        let end = h.offset.checked_add(bytes.len()).ok_or(INVAL)?;
        if end > LIMIT {
            return Err(INVAL);
        }
        data.resize(data.len().max(end), 0);
        data[h.offset..end].copy_from_slice(bytes);
        h.offset = end;
        Ok(())
    }

    pub fn seek(&mut self, fd: u32, offset: i64, whence: i32) -> Result<u64, i32> {
        let h = self.handles.get_mut(&fd).ok_or(BADF)?;
        if h.rights & 4 == 0 {
            return Err(NOTCAPABLE);
        }
        let base = match whence {
            0 => 0,
            1 => h.offset,
            2 => self.files.get(&h.path).ok_or(NOENT)?.len(),
            _ => return Err(INVAL),
        };
        let new = (base as i64).checked_add(offset).ok_or(INVAL)?;
        if !(0..=LIMIT as i64).contains(&new) {
            return Err(INVAL);
        }
        h.offset = new as usize;
        Ok(new as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn directory_removal_checks_type_children_and_the_mounted_root() {
        let mut fs = VirtualFs::default();
        fs.directories.insert("/dir".into());
        assert_eq!(fs.remove_directory(b"missing"), Err(NOENT));
        assert_eq!(fs.remove_directory(b"/"), Err(10));
        let fd = fs.open(b"dir/file", 1, 2, 0).unwrap();
        assert_eq!(fs.remove_directory(b"dir/file"), Err(NOTDIR));
        assert_eq!(fs.remove_directory(b"dir"), Err(NOTEMPTY));
        fs.close(fd);
        fs.unlink(b"dir/file").unwrap();
        fs.remove_directory(b"dir").unwrap();
        assert!(!fs.directories.contains("/dir"));
        assert_eq!(fs.remove_directory(b"../outside"), Err(NOTCAPABLE));
    }
    #[test]
    fn unlink_removes_name_but_preserves_open_descriptors() {
        let mut fs = VirtualFs::default();
        let old = fs.open(b"test", 1, 2 | 4 | 64, 0).unwrap();
        fs.write(old, b"old").unwrap();
        fs.unlink(b"test").unwrap();
        assert_eq!(fs.open(b"test", 0, 2, 0), Err(NOENT));
        let new = fs.open(b"test", 1, 2 | 4 | 64, 0).unwrap();
        fs.write(new, b"new").unwrap();
        fs.seek(old, 0, 0).unwrap();
        assert_eq!(fs.read(old, 3).unwrap(), b"old");
        assert_eq!(fs.files.len(), 2);
        assert!(fs.close(old));
        assert_eq!(fs.files.len(), 1);
        assert_eq!(fs.unlink(b"missing"), Err(NOENT));
        assert_eq!(fs.unlink(b"../host"), Err(NOTCAPABLE));
        assert_eq!(fs.unlink(b"/"), Err(ISDIR));
    }
    #[test]
    fn virtual_files_are_ephemeral_bounded_and_capability_checked() {
        let mut fs = VirtualFs::default();
        assert_eq!(fs.open(b"profile.json", 0, 2, 0), Err(NOENT));
        assert_eq!(fs.open(b"../host-file", 1, 64, 0), Err(NOTCAPABLE));
        assert!(fs.open(b"lock", 1, 2, 0).is_ok());
        let fd = fs.open(b"test", 1, 2 | 4 | 64, 0).unwrap();
        fs.write(fd, b"example").unwrap();
        fs.seek(fd, 0, 0).unwrap();
        assert_eq!(fs.read(fd, 20).unwrap(), b"example");
        assert_eq!(fs.read(fd, 20).unwrap(), b"");
        assert!(fs.write(fd, &vec![0; LIMIT]).is_err());
        let readonly = fs.open(b"test", 0, 2, 0).unwrap();
        assert_eq!(fs.write(readonly, b"x"), Err(NOTCAPABLE));
        assert!(VirtualFs::default().files.is_empty());
    }
}
