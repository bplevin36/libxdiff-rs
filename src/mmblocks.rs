use core::{
    ffi::{c_int, c_long, c_ulong, c_void},
    mem::{forget, swap, MaybeUninit},
    ptr::{addr_of, addr_of_mut},
};

use libxdiff_sys::{
    mmfile_t, xdl_free_mmfile, xdl_mmfile_cmp, xdl_mmfile_compact, xdl_mmfile_iscompact,
    xdl_mmfile_size, xdl_write_mmfile, XDL_MMF_ATOMIC,
};

use crate::{ensure_init, init_mmfile, MMFile};

/// An MMFile that does not have compactness as an invariant
#[derive(Debug)]
pub struct MMBlocks {
    pub(crate) inner: mmfile_t,
}

impl Drop for MMBlocks {
    fn drop(&mut self) {
        unsafe { xdl_free_mmfile(addr_of_mut!(self.inner)) };
    }
}

impl MMBlocks {
    /// Initialize an empty MMBlocks
    pub fn new() -> Self {
        ensure_init();
        Self {
            inner: init_mmfile(0),
        }
    }

    /// Create a new MMBlocks initialized with contents
    pub fn from_bytes(bytes: &[u8]) -> Self {
        ensure_init();
        let mut inner = init_mmfile(bytes.len());
        let bytes_written = unsafe {
            xdl_write_mmfile(
                addr_of_mut!(inner),
                bytes.as_ptr() as *const c_void,
                bytes.len() as c_long,
            )
        };
        if bytes_written != bytes.len() as i64 {
            panic!(
                "mmfile write only wrote {} bytes when {} were requested",
                bytes_written,
                bytes.len()
            );
        }
        Self { inner }
    }

    /// Checks if the entire file is a single allocation.
    pub fn is_compact(&self) -> bool {
        // SAFETY: one of the few places we lie about pointer mutability.
        // I have personally checked that libxdiff does not mutate anything here.
        unsafe { xdl_mmfile_iscompact(addr_of!(self.inner) as *mut mmfile_t) != 0 }
    }

    /// Ensure this blocks is compact; reallocates if it isn't.
    pub fn to_compact(&mut self) {
        if self.is_compact() {
            return;
        }
        let mut compacted: MaybeUninit<mmfile_t> = MaybeUninit::uninit();
        let compacted_ptr = compacted.as_mut_ptr();
        let bsize = self.size() as c_long;

        let compact_result = unsafe {
            xdl_mmfile_compact(
                addr_of_mut!(self.inner),
                compacted_ptr,
                bsize,
                XDL_MMF_ATOMIC as c_ulong,
            )
        };
        if compact_result != 0 {
            panic!("compaction failed");
        }
        let mut new_blocks = MMBlocks {
            inner: unsafe { compacted.assume_init() },
        };
        swap(self, &mut new_blocks); // swap new one in, old one is dropped
    }

    /// Get size of stored data in bytes
    pub fn size(&mut self) -> usize {
        unsafe { xdl_mmfile_size(addr_of_mut!(self.inner)) as usize }
    }

    /// Convert this possibly-non-compact file to an MMFile
    pub fn to_mmfile(mut self) -> MMFile {
        self.to_compact();
        let inner_mmfile = self.inner;
        // forget the original blocks so inner obj is not freed
        forget(self);
        MMFile {
            inner: inner_mmfile,
        }
    }

    /// Write a buffer of data to the end of this file
    pub fn write_buf(&mut self, buf: &[u8]) -> c_int {
        let write_result = unsafe {
            xdl_write_mmfile(
                addr_of_mut!(self.inner),
                buf.as_ptr() as *const c_void,
                buf.len() as c_long,
            )
        };
        if write_result == buf.len() as c_long {
            0
        } else {
            -1
        }
    }

    /// Create a copy
    pub fn clone(&mut self) -> Self {
        let mut compacted: MaybeUninit<mmfile_t> = MaybeUninit::uninit();
        let compacted_ptr = compacted.as_mut_ptr();
        let bsize = self.size() as c_long;

        let compact_result = unsafe {
            xdl_mmfile_compact(
                addr_of_mut!(self.inner),
                compacted_ptr,
                bsize,
                XDL_MMF_ATOMIC as c_ulong,
            )
        };
        if compact_result != 0 {
            panic!("compaction failed");
        }
        return MMBlocks {
            inner: unsafe { compacted.assume_init() },
        };
    }

    /// Compare contents of 2 files for equality. The underlying structs track
    /// their own iterator state, so comparison requires mutable access.
    pub fn eq(&mut self, other: &mut Self) -> bool {
        unsafe { xdl_mmfile_cmp(addr_of_mut!(self.inner), addr_of_mut!(other.inner)) == 0 }
    }
}
