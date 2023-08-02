use core::{
    ffi::{c_int, c_long, c_ulong, c_void},
    mem::MaybeUninit,
    ptr::addr_of_mut,
};

use libxdiff_sys::{mmfile_t, xdl_free_mmfile, xdl_write_mmfile, xdl_mmfile_iscompact, xdl_mmfile_compact, XDL_MMF_ATOMIC, xdl_mmfile_size};

use crate::{init_mmfile, MMFile, ensure_init};


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
    /// Initialize an empty MMPatch
    pub(crate) fn new() -> Self {
        ensure_init();
        Self {
            inner: init_mmfile(0),
        }
    }

    /// Checks if the entire file is a single allocation.
    pub(crate) fn is_compact(&mut self) -> bool {
        unsafe { xdl_mmfile_iscompact(addr_of_mut!(self.inner)) != 0 }
    }

    /// Get size of stored data in bytes
    pub fn size(&mut self) -> usize {
        unsafe { xdl_mmfile_size(addr_of_mut!(self.inner)) as usize }
    }

    /// Convert this possibly-non-compact file to an MMFile
    pub(crate) fn to_mmfile(mut self) -> MMFile {
        if !self.is_compact() {
            let mut compacted: MaybeUninit<mmfile_t> = MaybeUninit::uninit();
            let compacted_ptr = compacted.as_mut_ptr();
            let bsize = self.size() as c_long;

            let compact_result = unsafe {
                xdl_mmfile_compact(addr_of_mut!(self.inner), compacted_ptr, bsize, XDL_MMF_ATOMIC as c_ulong)
            };
            if compact_result != 0 {
                panic!("compaction failed");
            }
            return MMFile {
                inner: unsafe { compacted.assume_init() },
            };
        }
        MMFile {
            inner: self.inner,
        }
    }

    /// Write a buffer of data to the end of this file
    pub(crate) fn write_buf(&mut self, buf: &[u8]) -> c_int {
        let write_result = unsafe { xdl_write_mmfile(
            addr_of_mut!(self.inner),
            buf.as_ptr() as *const c_void,
            buf.len() as c_long) };
        if write_result == buf.len() as c_long {
            0
        } else {
            -1
        }
    }
}
