//! # libxdiff bindings for Rust
//!
//! This library contains bindings to the [libxdiff][1] C library. The
//! underlying library defines "MMFiles" using chains of non-contiguous buffers
//! to minimize reallocations when appending and mutating.
//! This wrapper structures the API by defining all [`MMFile`]s to be compact
//! (backed by a single buffer). The non-compact form is [`MMBlocks`].
//!
//! libxdiff tracks iteration over buffers internally, so some operations that
//! conceptually are read-only end up requiring `&mut` arguments in order to be
//! safe.
//!
//! # Example
//!
//! ```
//! use core::str::from_utf8;
//! use libxdiff::MMFile;
//!
//! let mut f1 = MMFile::from_bytes(b"hello world\n");
//! let mut f2 = MMFile::from_bytes(b"hello world!\n");
//! let mut diff_lines = Vec::<String>::new();
//!     f1.diff_raw(&mut f2, |line: &[u8]| {
//!         diff_lines.push(from_utf8(line).unwrap().to_owned());
//!     })
//!     .unwrap();
//!
//! assert_eq!(
//!     diff_lines,
//!     vec![
//!         "@@ -1,1 +1,1 @@\n",
//!         "-", "hello world\n",
//!         "+", "hello world!\n",
//!     ],
//! );
//! ```
//!
//! [1]: http://www.xmailserver.org/xdiff-lib.html

#[cfg_attr(not(feature = "std"), no_std)]
use core::{
    ffi::{c_long, c_uint, c_ulong, c_void},
    mem::MaybeUninit,
    ptr::{addr_of, null_mut},
    sync::atomic::{AtomicBool, Ordering},
};

use libc::{free, malloc, realloc, size_t};
use libxdiff_sys::{memallocator_t, mmfile_t, xdl_init_mmfile, xdl_set_allocator, XDL_MMF_ATOMIC};

mod mmfile;
pub use mmfile::*;

mod mmblocks;
pub use mmblocks::*;

#[cfg(test)]
mod tests;

unsafe extern "C" fn wrap_malloc(_obj: *mut c_void, size: c_uint) -> *mut c_void {
    malloc(size as size_t)
}

unsafe extern "C" fn wrap_free(_obj: *mut c_void, ptr: *mut c_void) {
    free(ptr)
}

unsafe extern "C" fn wrap_realloc(
    _obj: *mut c_void,
    ptr: *mut c_void,
    size: c_uint,
) -> *mut c_void {
    realloc(ptr, size as size_t)
}

// must call before using any xdl functions and must only call once
unsafe fn init() {
    let alloc_struct = memallocator_t {
        priv_: null_mut(),
        malloc: Some(wrap_malloc),
        free: Some(wrap_free),
        realloc: Some(wrap_realloc),
    };
    unsafe { xdl_set_allocator(addr_of!(alloc_struct)) };
}

static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Safely ensure libxdiff has been initialized before proceeding. U
/// This is called automatically when [`MMFile`]s are created. From
/// that point on, the existence of the object means the library has been
/// initialized already.
pub(crate) fn ensure_init() {
    if let Ok(_) = INITIALIZED.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed) {
        unsafe { init() }
    }
}

/// Initialize a new mmfile_t
pub(crate) fn init_mmfile(len: usize) -> mmfile_t {
    ensure_init();
    let mut inner: MaybeUninit<mmfile_t> = MaybeUninit::uninit();
    let inner_ptr = inner.as_mut_ptr();
    let err = unsafe { xdl_init_mmfile(inner_ptr, len as c_long, XDL_MMF_ATOMIC as c_ulong) };
    if err != 0 {
        panic!("mmfile initialization failed");
    }
    unsafe { inner.assume_init() }
}
