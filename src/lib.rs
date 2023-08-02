//! # libxdiff bindings for Rust
//!
//! This library contains bindings to the [libxdiff][1] C library. The C library
//! defines "MMFiles" using chains of non-contiguous buffers
//! to minimize reallocations when appending and mutating.
//! This wrapper simplifies the API by only dealing with "atomic" files
//! backed by a single buffer. The underlying library tracks iteration over
//! buffers internally, so many operations that conceptually are read-only end
//! up requiring `&mut` arguments to be safe.
//!
//! libxdiff is small and has no dependencies, so this crate links it statically.
//!
//! [1]: http://www.xmailserver.org/xdiff-lib.html

use core::{
    ffi::{c_long, c_uint, c_ulong, c_void},
    mem::MaybeUninit,
    ptr::{addr_of, null_mut},
    sync::atomic::{AtomicBool, Ordering},
};

use libc::{free, malloc, realloc, size_t};
use libxdiff_sys::{mmfile_t, xdl_init_mmfile, XDL_MMF_ATOMIC, memallocator_t, xdl_set_allocator};

mod mmfile;
pub use mmfile::*;

mod mmblocks;
pub use mmblocks::*;


unsafe extern "C" fn wrap_malloc(_obj: *mut c_void, size: c_uint) -> *mut c_void {
    malloc(size as size_t)
}

unsafe extern "C" fn wrap_free(_obj: *mut c_void, ptr: *mut c_void) {
    free(ptr)
}

unsafe extern "C" fn wrap_realloc(_obj: *mut c_void, ptr: *mut c_void, size: c_uint) -> *mut c_void {
    realloc(ptr, size as size_t)
}

// must call before using any xdl functions and must only call once
unsafe fn init() {
    let alloc_struct = memallocator_t{
        priv_: null_mut(),
        malloc: Some(wrap_malloc),
        free: Some(wrap_free),
        realloc: Some(wrap_realloc),
    };
    unsafe { xdl_set_allocator(addr_of!(alloc_struct)) };
}

static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Safely ensure libxdiff has been initialized before proceeding.
/// This is called automatically when [`MMFile`]s are created. From
/// that point on, the existence of the object means the library has been
/// initialized already.
pub fn ensure_init() {
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
