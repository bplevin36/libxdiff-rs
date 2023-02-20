
use core::{
    ptr::{null_mut, addr_of, addr_of_mut},
    mem::MaybeUninit,
    slice::from_raw_parts,
    sync::atomic::{AtomicBool, Ordering}
};

use libc::{free, malloc, realloc, c_void, c_uint, size_t, c_ulong, c_long, c_int};
use libxdiff_sys::{
    memallocator_t, xdl_set_allocator, mmfile_t, xdl_init_mmfile,
    XDL_MMF_ATOMIC, xdl_write_mmfile, xdl_free_mmfile, xdl_mmfile_size, xdl_mmfile_iscompact, xdl_diff, xpparam_t, xdemitconf_t, mmbuffer_t, xdemitcb_t, xdl_merge3
};


pub unsafe extern "C" fn wrap_malloc(_obj: *mut c_void, size: c_uint) -> *mut c_void {
    malloc(size as size_t)
}

pub unsafe extern "C" fn wrap_free(_obj: *mut c_void, ptr: *mut c_void) {
    free(ptr)
}

pub unsafe extern "C" fn wrap_realloc(_obj: *mut c_void, ptr: *mut c_void, size: c_uint) -> *mut c_void {
    realloc(ptr, size as size_t)
}

// must call before using xdl functions and must only call once
pub unsafe fn init() {
    let alloc_struct = memallocator_t{
        priv_: null_mut(),
        malloc: Some(wrap_malloc),
        free: Some(wrap_free),
        realloc: Some(wrap_realloc),
    };
    unsafe { xdl_set_allocator(addr_of!(alloc_struct)) };
}

static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Safely ensure libxdiff has been initialized before proceeding
/// We will get away with only calling this when constructing `MMFile`. From
/// that point on, the existence of the object means the library has been
/// initialized already.
pub fn ensure_init() {
    if let Ok(_) = INITIALIZED.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed) {
        unsafe { init() }
    }
}

#[derive(Debug)]
pub struct MMFile {
    inner: mmfile_t,
}

impl Drop for MMFile {
    fn drop(&mut self) {
        unsafe { xdl_free_mmfile(addr_of_mut!(self.inner)) };
    }
}

/// Type representing an owned memory file in libxdiff.
impl MMFile {
    /// Create a new MMFile with block size zero
    pub fn new() -> MMFile {
        ensure_init();
        let mut inner: MaybeUninit<mmfile_t> = MaybeUninit::uninit();
        let inner_ptr = inner.as_mut_ptr();
        let err = unsafe { xdl_init_mmfile(inner_ptr, 0, XDL_MMF_ATOMIC as c_ulong) };
        if err != 0 {
            panic!("mmfile initialization failed");
        }
        MMFile {
            inner: unsafe { inner.assume_init() },
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> MMFile {
        ensure_init();
        let mut inner: MaybeUninit<mmfile_t> = MaybeUninit::uninit();
        let inner_ptr = inner.as_mut_ptr();
        let err = unsafe { xdl_init_mmfile(inner_ptr, bytes.len() as c_long, XDL_MMF_ATOMIC as c_ulong) };
        if err != 0 {
            panic!("mmfile initialization failed");
        }

        let bytes_written = unsafe { xdl_write_mmfile(inner_ptr, bytes.as_ptr() as *const c_void, bytes.len() as c_long) };
        if bytes_written != bytes.len() as i64 {
            panic!("mmfile write only wrote {} bytes when {} were requested", bytes_written, bytes.len());
        }
        MMFile {
            inner: unsafe { inner.assume_init() },
        }
    }

    pub fn size(&mut self) -> usize {
        unsafe { xdl_mmfile_size(addr_of_mut!(self.inner)) as usize }
    }

    pub fn is_compact(&mut self) -> bool {
        unsafe { xdl_mmfile_iscompact(addr_of_mut!(self.inner)) != 0 }
    }

    pub fn diff(&mut self, other: &mut MMFile) {
        let xpparam = xpparam_t{ flags: 0 };
        let conf = xdemitconf_t{ ctxlen: 3 };
        extern "C" fn emit_cb(_obj: *mut c_void, buffers: *mut mmbuffer_t, num: c_int) -> c_int {
            println!("Emitting {} buffers", num);
            for i in 0..num {
                let buffer = unsafe { buffers.add(i as usize) };
                let slice = unsafe { from_raw_parts((*buffer).ptr as *const u8, (*buffer).size as usize) };
                println!("line: {}", slice.escape_ascii());
            }
            0
        }
        let mut emit_struct = xdemitcb_t { priv_: null_mut(), outf: Some(emit_cb) };
        let err = unsafe { xdl_diff(
            addr_of_mut!(self.inner), addr_of_mut!(other.inner),
            addr_of!(xpparam), addr_of!(conf), addr_of_mut!(emit_struct)) };
        if err != 0 {
            panic!("diff failed with err: {}", err);
        }
    }

    pub fn merge(base: &mut MMFile, f1: &mut MMFile, f2: &mut MMFile) {
        extern "C" fn emit_cb(_obj: *mut c_void, buffers: *mut mmbuffer_t, num: c_int) -> c_int {
            println!("Emitting {} buffers", num);
            for i in 0..num {
                let buffer = unsafe { buffers.add(i as usize) };
                let slice = unsafe { from_raw_parts((*buffer).ptr as *const u8, (*buffer).size as usize) };
                println!("line: {}", slice.escape_ascii());
            }
            0
        }
        let mut emit_struct = xdemitcb_t { priv_: null_mut(), outf: Some(emit_cb) };
        extern "C" fn reject_cb(_obj: *mut c_void, buffers: *mut mmbuffer_t, num: c_int) -> c_int {
            println!("Rejecting {} buffers", num);
            for i in 0..num {
                let buffer = unsafe { buffers.add(i as usize) };
                let slice = unsafe { from_raw_parts((*buffer).ptr as *const u8, (*buffer).size as usize) };
                println!("line: {}", slice.escape_ascii());
            }
            0
        }
        let mut reject_struct = xdemitcb_t { priv_: null_mut(), outf: Some(reject_cb) };

        let err = unsafe { xdl_merge3(
            addr_of_mut!(base.inner), addr_of_mut!(f1.inner), addr_of_mut!(f2.inner),
            addr_of_mut!(emit_struct), addr_of_mut!(reject_struct)) };
        if err != 0 {
            panic!("Merge failed with error: {}", err);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::MMFile;

    #[test]
    fn new_empty() {
        let mut f = MMFile::new();
        assert_eq!(f.size(), 0);
        assert!(f.is_compact());
    }

    #[test]
    fn new_from_bytes() {
        let data = b"hello world";
        let mut f = MMFile::from_bytes(data);
        assert_eq!(f.size(), data.len());
        assert!(f.is_compact());
    }

    #[test]
    fn large_from_bytes() {
        let mut data = Vec::new();
        data.extend((0..240).cycle().take(15_000));
        let mut f = MMFile::from_bytes(data.as_slice());
        assert_eq!(f.size(), data.len());
        assert!(f.is_compact());
    }
}
