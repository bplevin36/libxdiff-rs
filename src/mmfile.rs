use core::{
    ffi::{c_int, c_long, c_void},
    fmt::Debug,
    ptr::{addr_of, addr_of_mut},
    slice::from_raw_parts,
    str::from_utf8,
};

#[cfg(feature = "std")]
use std::panic::{catch_unwind, AssertUnwindSafe};

use libxdiff_sys::{
    mmbuffer_t, mmfile_t, xdemitcb_t, xdemitconf_t, xdl_diff, xdl_free_mmfile, xdl_merge3,
    xdl_mmfile_iscompact, xdl_mmfile_size, xdl_patch, xdl_write_mmfile, xdl_writem_mmfile,
    xpparam_t, XDL_PATCH_NORMAL,
};

use crate::{ensure_init, init_mmfile, MMBlocks};

pub type MMPatch = MMBlocks;

/// Type representing an owned, compact file in libxdiff
pub struct MMFile {
    // this mmfile is always compact
    pub(crate) inner: mmfile_t,
}

impl Drop for MMFile {
    fn drop(&mut self) {
        unsafe { xdl_free_mmfile(addr_of_mut!(self.inner)) };
    }
}

impl MMFile {
    /// Create a new empty MMFile
    pub fn new() -> MMFile {
        ensure_init();
        MMFile {
            inner: init_mmfile(0),
        }
    }
    /// Create a new MMFile initialized with contents
    pub fn from_bytes(bytes: &[u8]) -> MMFile {
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
        MMFile { inner }
    }

    /// Get size of stored data in bytes
    pub fn size(&mut self) -> usize {
        unsafe { xdl_mmfile_size(addr_of_mut!(self.inner)) as usize }
    }

    /// Checks if the entire file is a single allocation. In our library this
    /// is always true.
    pub fn is_compact(&self) -> bool {
        // SAFETY: one of the few places we lie about pointer mutability.
        // I have checked that libxdiff-0.23 doesn't mutate anything in this call.
        unsafe { xdl_mmfile_iscompact(addr_of!(self.inner) as *mut mmfile_t) != 0 }
    }

    /// Compute the patch to turn self into other
    pub fn compute_patch(&mut self, other: &mut Self) -> Result<MMPatch, String> {
        let mut patch = MMPatch::new();
        unsafe { self.diff_raw_nopanic(other, |buf| patch.write_buf(buf))? };
        Ok(patch)
    }

    /// Apply a patch to a file. If successful, return the new file. If
    /// unsuccessful, return (successfully patched part, rejected parts)
    pub fn apply_patch(&mut self, patch: &mut MMPatch) -> Result<MMFile, (MMFile, MMFile)> {
        patch.to_compact(); // patch must be compacted before use
        let mut patched = MMPatch::new();
        let mut rejected = MMPatch::new();

        extern "C" fn emit_cb(
            patch_ptr: *mut c_void,
            buffers: *mut mmbuffer_t,
            num: c_int,
        ) -> c_int {
            let ptr_to_patch = patch_ptr as *mut MMPatch;
            let patch_ref = unsafe { &mut *ptr_to_patch };

            let mut bytes_to_write = 0;
            for i in 0..num {
                let buffer = unsafe { buffers.add(i as usize) };
                bytes_to_write += unsafe { (*buffer).size };
            }
            let bytes_written =
                unsafe { xdl_writem_mmfile(addr_of_mut!(patch_ref.inner), buffers, num) };
            if bytes_to_write == bytes_written {
                0
            } else {
                -1
            }
        }
        let mut emit_struct = xdemitcb_t {
            priv_: addr_of_mut!(patched) as *mut c_void,
            outf: Some(emit_cb),
        };
        let mut reject_struct = xdemitcb_t {
            priv_: addr_of_mut!(rejected) as *mut c_void,
            outf: Some(emit_cb),
        };
        let patch_result = unsafe {
            xdl_patch(
                addr_of_mut!(self.inner),
                addr_of_mut!(patch.inner),
                XDL_PATCH_NORMAL as c_int,
                addr_of_mut!(emit_struct),
                addr_of_mut!(reject_struct),
            )
        };
        if patch_result == 0 && rejected.size() == 0 {
            Ok(patched.to_mmfile())
        } else {
            Err((patched.to_mmfile(), rejected.to_mmfile()))
        }
    }

    #[cfg(feature = "std")]
    /// Compute the diff to turn self into other, returning diff through a
    /// callback one line at a time. Returns `Err` if callback panics.
    pub fn diff_raw<CB>(&mut self, other: &mut MMFile, callback: CB) -> Result<(), String>
    where
        CB: FnMut(&[u8]),
    {
        let xpparam = xpparam_t { flags: 0 };
        let conf = xdemitconf_t { ctxlen: 3 };
        let mut boxed_cb: Box<dyn FnMut(&[u8])> = Box::new(callback);
        let ptr_to_box = addr_of_mut!(boxed_cb);
        let cb_ptr = ptr_to_box as *mut c_void;
        extern "C" fn emit_cb(cb_ptr: *mut c_void, buffers: *mut mmbuffer_t, num: c_int) -> c_int {
            let ptr_to_box = cb_ptr as *mut Box<dyn FnMut(&[u8])>;

            for i in 0..num {
                let buffer = unsafe { buffers.add(i as usize) };
                let slice =
                    unsafe { from_raw_parts((*buffer).ptr as *const u8, (*buffer).size as usize) };
                // This is unwind safe because our closure only closes over some pointers, no owned objects.
                // After we return an error, the boxed closure will not be called any more,
                // so any broken invariants in its closed-over variables won't be witnessed.
                match catch_unwind(AssertUnwindSafe(|| unsafe { (*ptr_to_box)(slice) })) {
                    Ok(_) => (),
                    Err(_) => {
                        // TODO: store the panic info somewhere
                        return -1;
                    }
                }
            }
            0
        }
        let mut emit_struct = xdemitcb_t {
            priv_: cb_ptr,
            outf: Some(emit_cb),
        };
        let err = unsafe {
            xdl_diff(
                addr_of_mut!(self.inner),
                addr_of_mut!(other.inner),
                addr_of!(xpparam),
                addr_of!(conf),
                addr_of_mut!(emit_struct),
            )
        };
        if err != 0 {
            Err(format!("diff failed with err: {}", err))
        } else {
            Ok(())
        }
    }

    /// Compute the diff to turn self into other, returning diff through a
    /// callback one line at a time. Callback should return 0 on success and -1
    /// on failure.
    ///
    ///
    /// # Safety
    /// The provided callback must not panic
    pub unsafe fn diff_raw_nopanic<CB>(
        &mut self,
        other: &mut MMFile,
        callback: CB,
    ) -> Result<(), String>
    where
        CB: FnMut(&[u8]) -> c_int,
    {
        let xpparam = xpparam_t { flags: 0 };
        let conf = xdemitconf_t { ctxlen: 3 };
        let mut boxed_cb: Box<dyn FnMut(&[u8]) -> c_int> = Box::new(callback);
        let ptr_to_box = addr_of_mut!(boxed_cb);
        let cb_ptr = ptr_to_box as *mut c_void;
        extern "C" fn emit_cb(cb_ptr: *mut c_void, buffers: *mut mmbuffer_t, num: c_int) -> c_int {
            let ptr_to_box = cb_ptr as *mut Box<dyn FnMut(&[u8]) -> c_int>;

            for i in 0..num {
                let buffer = unsafe { buffers.add(i as usize) };
                let slice =
                    unsafe { from_raw_parts((*buffer).ptr as *const u8, (*buffer).size as usize) };
                let cb_result = unsafe { (*ptr_to_box)(slice) };
                if cb_result < 0 {
                    return cb_result as c_int;
                }
            }
            0
        }
        let mut emit_struct = xdemitcb_t {
            priv_: cb_ptr,
            outf: Some(emit_cb),
        };
        let err = unsafe {
            xdl_diff(
                addr_of_mut!(self.inner),
                addr_of_mut!(other.inner),
                addr_of!(xpparam),
                addr_of!(conf),
                addr_of_mut!(emit_struct),
            )
        };
        if err != 0 {
            Err(format!("diff failed with errno: {}", err))
        } else {
            Ok(())
        }
    }

    #[cfg(feature = "std")]
    /// Compute the file that results from merging two sets of changes to the
    /// base file. The resulting file is passed line-by-line to the
    /// `accept_callback`, any conflicting changes are passed to the
    /// `reject_callback`. Returns `Err` if any callback panics; panicking
    /// should be avoided wherever possible.
    pub fn merge3_raw<CBA, CBR>(
        base: &mut MMFile,
        f1: &mut MMFile,
        f2: &mut MMFile,
        accept_callback: CBA,
        reject_callback: CBR,
    ) -> Result<(), String>
    where
        CBA: FnMut(&[u8]),
        CBR: FnMut(&[u8]),
    {
        // prepare callback for emitting accepted lines
        let mut boxed_acc_cb: Box<dyn FnMut(&[u8])> = Box::new(accept_callback);
        let box_acc_ptr = addr_of_mut!(boxed_acc_cb);
        let void_acc_ptr = box_acc_ptr as *mut c_void;
        extern "C" fn emit_cb(cb_ptr: *mut c_void, buffers: *mut mmbuffer_t, num: c_int) -> c_int {
            let ptr_to_box = cb_ptr as *mut Box<dyn FnMut(&[u8])>;

            for i in 0..num {
                let buffer = unsafe { buffers.add(i as usize) };
                let slice =
                    unsafe { from_raw_parts((*buffer).ptr as *const u8, (*buffer).size as usize) };
                // This is unwind safe because our closure only closes over some pointers, no owned objects.
                // After we return an error, the boxed closure will not be called any more,
                // so any broken invariants in its closed-over variables won't be witnessed.
                match catch_unwind(AssertUnwindSafe(|| unsafe { (*ptr_to_box)(slice) })) {
                    Ok(_) => (),
                    Err(_) => {
                        // TODO: store the panic info somewhere
                        return -1;
                    }
                }
            }
            0
        }
        let mut emit_struct = xdemitcb_t {
            priv_: void_acc_ptr,
            outf: Some(emit_cb),
        };

        // prepare callback for emitting rejected lines
        let mut boxed_rej_cb: Box<dyn FnMut(&[u8])> = Box::new(reject_callback);
        let box_rej_ptr = addr_of_mut!(boxed_rej_cb);
        let void_rej_ptr = box_rej_ptr as *mut c_void;
        extern "C" fn reject_cb(
            cb_ptr: *mut c_void,
            buffers: *mut mmbuffer_t,
            num: c_int,
        ) -> c_int {
            let ptr_to_box = cb_ptr as *mut Box<dyn FnMut(&[u8])>;

            for i in 0..num {
                let buffer = unsafe { buffers.add(i as usize) };
                let slice =
                    unsafe { from_raw_parts((*buffer).ptr as *const u8, (*buffer).size as usize) };
                // This is unwind safe because our closure only closes over pointers, no owned objects that would run destructors.
                // After we return an error, the boxed closure will not be called any more,
                // so any broken invariants in its closed-over variables won't be witnessed.
                match catch_unwind(AssertUnwindSafe(|| unsafe { (*ptr_to_box)(slice) })) {
                    Ok(_) => (),
                    Err(_) => {
                        // TODO: maybe store the panic info somewhere?
                        return -1;
                    }
                }
            }
            0
        }
        let mut reject_struct = xdemitcb_t {
            priv_: void_rej_ptr,
            outf: Some(reject_cb),
        };
        let err = unsafe {
            xdl_merge3(
                addr_of_mut!(base.inner),
                addr_of_mut!(f1.inner),
                addr_of_mut!(f2.inner),
                addr_of_mut!(emit_struct),
                addr_of_mut!(reject_struct),
            )
        };
        if err != 0 {
            Err(format!("merge failed with err: {}", err))
        } else {
            Ok(())
        }
    }

    /// Get a view of the `MMFile`'s data as a slice
    pub fn as_slice(&self) -> &[u8] {
        assert!(self.is_compact());
        let head_block = self.inner.head;
        if head_block.is_null() {
            return &[];
        }
        let len = unsafe { (*head_block).size };
        if len <= 0 {
            return &[];
        }

        let char_ptr = unsafe { (*head_block).ptr as *const u8 };
        if char_ptr.is_null() {
            &[]
        } else {
            unsafe { core::slice::from_raw_parts(char_ptr, len as usize) }
        }
    }

    /// Get a mutable view of the `MMFile`'s data as a slice
    pub fn as_slice_mut(&mut self) -> &mut [u8] {
        assert!(self.is_compact());
        let head_block = self.inner.head;
        if head_block.is_null() {
            return &mut [];
        }
        let len = unsafe { (*head_block).size };
        if len <= 0 {
            return &mut [];
        }

        let char_ptr = unsafe { (*head_block).ptr as *mut u8 };
        if char_ptr.is_null() {
            &mut []
        } else {
            unsafe { core::slice::from_raw_parts_mut(char_ptr, len as usize) }
        }
    }
}

impl Clone for MMFile {
    fn clone(&self) -> Self {
        Self::from_bytes(self.as_slice())
    }
}

impl PartialEq for MMFile {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl Debug for MMFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match from_utf8(self.as_slice()) {
            Ok(s) => f.write_fmt(format_args!("MMFile UTF:\n\"{}\"", s)),
            Err(_) => {
                // just use debug impl if not utf
                Debug::fmt(&self, f)
            }
        }
    }
}
