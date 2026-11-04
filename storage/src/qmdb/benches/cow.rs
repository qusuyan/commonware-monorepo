//! Isolated scratch directories with per-file CoW and a portable copy fallback.
#![allow(dead_code)]
use super::artifact_io;
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
#[cfg(target_os = "macos")]
use std::{ffi::CString, os::unix::ffi::OsStrExt};
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
#[derive(Clone, Debug)]
pub struct CloneReport {
    pub elapsed: Duration,
    pub method: &'static str,
}
fn clone_file(src: &Path, dst: &Path) -> io::Result<&'static str> {
    #[cfg(target_os = "macos")]
    {
        let a = CString::new(src.as_os_str().as_bytes())?;
        let b = CString::new(dst.as_os_str().as_bytes())?;
        // SAFETY: Both strings are NUL-terminated and remain valid for this call.
        if unsafe { libc::clonefile(a.as_ptr(), b.as_ptr(), 0) } == 0 {
            return Ok("clonefile");
        }
    }
    #[cfg(target_os = "linux")]
    {
        let source = fs::File::open(src)?;
        let target = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dst)?;
        // SAFETY: The file descriptors remain open and FICLONE takes the source descriptor.
        if unsafe {
            libc::ioctl(
                target.as_raw_fd(),
                0x40049409 as libc::c_ulong,
                source.as_raw_fd(),
            )
        } == 0
        {
            return Ok("reflink");
        }
        drop(target);
        fs::remove_file(dst)?;
    }
    fs::copy(src, dst)?;
    Ok("recursive-copy")
}
fn copy_into(src: &Path, dst: &Path, method: &mut Option<&'static str>) -> io::Result<()> {
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            fs::create_dir(&to)?;
            copy_into(&entry.path(), &to, method)?;
        } else if ty.is_file() {
            let used = clone_file(&entry.path(), &to)?;
            *method = Some(match *method {
                None => used,
                Some(old) if old == used => old,
                _ => "mixed-copy",
            });
        } else {
            return Err(artifact_io::invalid(
                "snapshot contains a symlink or special file",
            ));
        }
    }
    Ok(())
}
pub fn clone_directory(src: &Path, dst: &Path) -> io::Result<Duration> {
    let start = Instant::now();
    fs::create_dir_all(artifact_io::parent(dst))?;
    fs::create_dir(dst)?;
    if let Err(error) = copy_into(src, dst, &mut None) {
        let _ = fs::remove_dir_all(dst);
        return Err(error);
    }
    Ok(start.elapsed())
}
pub fn copy_dir_recursive(src: &Path, dst: &Path) -> io::Result<()> {
    clone_directory(src, dst).map(|_| ())
}
#[derive(Debug)]
pub struct ScratchDir {
    path: PathBuf,
    keep: bool,
    pub report: CloneReport,
}
impl ScratchDir {
    pub fn empty(base: &Path) -> io::Result<Self> {
        fs::create_dir_all(base)?;
        for _ in 0..32 {
            let path = artifact_io::unique(base, "qmdb-scratch");
            match fs::create_dir(&path) {
                Ok(()) => {
                    return Ok(Self {
                        path,
                        keep: false,
                        report: CloneReport {
                            elapsed: Duration::ZERO,
                            method: "fresh",
                        },
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "scratch allocation collisions",
        ))
    }
    pub fn new_from_clone(master: &Path, base: &Path) -> io::Result<Self> {
        let mut scratch = Self::empty(base)?;
        let start = Instant::now();
        let mut method = None;
        copy_into(master, scratch.path(), &mut method)?;
        scratch.report = CloneReport {
            elapsed: start.elapsed(),
            method: method.unwrap_or("empty"),
        };
        Ok(scratch)
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn keep(mut self) -> PathBuf {
        self.keep = true;
        self.path.clone()
    }
}
impl Drop for ScratchDir {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
