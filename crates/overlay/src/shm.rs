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
            let name = c"lapinux-overlay";
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

    /// Copie un Pixmap tiny-skia (RGBA prémultiplié) vers un tampon
    /// `wl_shm::Format::Argb8888`. Ce format est natif : sur little-endian,
    /// la mémoire attend BGRA. Écrire les octets RGBA tels quels inversait le
    /// rouge et le bleu de tout l'overlay.
    pub fn write_rgba(&mut self, data: &[u8]) {
        let count = data.len().min(self.len);
        let pixels = count / 4 * 4;
        unsafe {
            for offset in (0..pixels).step_by(4) {
                #[cfg(target_endian = "little")]
                {
                    *self.ptr.add(offset) = data[offset + 2];
                    *self.ptr.add(offset + 1) = data[offset + 1];
                    *self.ptr.add(offset + 2) = data[offset];
                    *self.ptr.add(offset + 3) = data[offset + 3];
                }
                #[cfg(target_endian = "big")]
                {
                    *self.ptr.add(offset) = data[offset + 3];
                    *self.ptr.add(offset + 1) = data[offset];
                    *self.ptr.add(offset + 2) = data[offset + 1];
                    *self.ptr.add(offset + 3) = data[offset + 2];
                }
            }
            if count > pixels {
                std::ptr::copy_nonoverlapping(
                    data[pixels..].as_ptr(),
                    self.ptr.add(pixels),
                    count - pixels,
                );
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_is_written_in_native_argb8888_order() {
        let mut mapping = Mapping::new(8).expect("mapping");
        mapping.write_rgba(&[10, 20, 30, 255, 1, 2, 3, 4]);
        let bytes = unsafe { std::slice::from_raw_parts(mapping.ptr, 8) };
        #[cfg(target_endian = "little")]
        assert_eq!(bytes, &[30, 20, 10, 255, 3, 2, 1, 4]);
        #[cfg(target_endian = "big")]
        assert_eq!(bytes, &[255, 10, 20, 30, 4, 1, 2, 3]);
    }
}
