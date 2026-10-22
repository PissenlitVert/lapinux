//! Mémoire partagée Wayland (memfd + mmap) pour les tampons du menu.

use crate::{Error, Result};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

pub struct Mapping {
    pub fd: OwnedFd,
    pub ptr: *mut u8,
    pub len: usize,
}

impl Mapping {
    /// Crée un memfd de `len` octets et le mappe.
    pub fn new(len: usize) -> Result<Self> {
        unsafe {
            let name = c"ludus-overlay";
            let fd = libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC);
            if fd < 0 {
                return Err(Error::Other(format!(
                    "memfd_create : {}",
                    std::io::Error::last_os_error()
                )));
            }
            let fd = OwnedFd::from_raw_fd(fd);
            if libc::ftruncate(fd.as_raw_fd(), len as libc::off_t) != 0 {
                return Err(Error::Other(format!(
                    "ftruncate : {}",
                    std::io::Error::last_os_error()
                )));
            }
            let ptr = libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            );
            if ptr == libc::MAP_FAILED {
                return Err(Error::Other(format!(
                    "mmap : {}",
                    std::io::Error::last_os_error()
                )));
            }
            Ok(Self {
                fd,
                ptr: ptr as *mut u8,
                len,
            })
        }
    }

    pub fn write(&mut self, data: &[u8]) {
        let count = data.len().min(self.len);
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), self.ptr, count);
        }
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.ptr as *mut libc::c_void, self.len);
        }
    }
}

unsafe impl Send for Mapping {}
