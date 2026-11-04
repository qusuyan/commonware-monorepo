//! Exclusive creation and durable publication of benchmark artifacts.
#![allow(dead_code)]
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
pub fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
pub fn unique(parent: &Path, prefix: &str) -> PathBuf {
    parent.join(format!(
        ".{prefix}-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}
pub fn sync_dir(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}
pub fn parent(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}
pub struct CreationLock(PathBuf);
impl CreationLock {
    pub fn acquire(path: &Path) -> io::Result<Self> {
        fs::create_dir_all(parent(path))?;
        let mut name = path.as_os_str().to_os_string();
        name.push(".lock");
        let path = PathBuf::from(name);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let guard = Self(path);
        writeln!(file, "{}", std::process::id())?;
        Ok(guard)
    }
}
impl Drop for CreationLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = unique(parent(path), "publish");
    let result = (|| {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)?;
        sync_dir(parent(path))
    })();
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}
