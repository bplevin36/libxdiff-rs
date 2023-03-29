//! # libxdiff bindings for Rust
//!
//! This library contains bindings to the [libxdiff][1] C library. Currently,
//! only a limited subset of the library's functionality is provided.
//!
//! [1]: http://www.xmailserver.org/xdiff-lib.html

use core::{
    ptr::{null_mut, addr_of, addr_of_mut},
    mem::MaybeUninit,
    slice::from_raw_parts,
    sync::atomic::{AtomicBool, Ordering}
};
use std::panic::{catch_unwind, AssertUnwindSafe};

use libc::{free, malloc, realloc, c_void, c_uint, size_t, c_ulong, c_long, c_int};
use libxdiff_sys::{
    memallocator_t, xdl_set_allocator, mmfile_t, xdl_init_mmfile,
    XDL_MMF_ATOMIC, xdl_write_mmfile, xdl_free_mmfile, xdl_mmfile_size,
    xdl_mmfile_iscompact, xdl_diff, xpparam_t, xdemitconf_t, mmbuffer_t,
    xdemitcb_t, xdl_merge3,
};

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
/// This is called automatically when `MMFile`s are created. From
/// that point on, the existence of the object means the library has been
/// initialized already.
pub fn ensure_init() {
    if let Ok(_) = INITIALIZED.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed) {
        unsafe { init() }
    }
}

/// Type representing an owned memory file in libxdiff.
#[derive(Debug)]
pub struct MMFile {
    inner: mmfile_t,
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
    /// Create a new MMFile initialized with contents
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

    /// Get size of stored data in bytes
    pub fn size(&mut self) -> usize {
        unsafe { xdl_mmfile_size(addr_of_mut!(self.inner)) as usize }
    }

    /// Checks if the entire file is a single allocation. Mostly a performance
    /// optimization, but also required for some underlying operations.
    pub fn is_compact(&mut self) -> bool {
        unsafe { xdl_mmfile_iscompact(addr_of_mut!(self.inner)) != 0 }
    }

    /// Compute the diff to turn self into other, returning diff through a
    /// callback one line at a time. Returns `Err` if callback panics, but this
    /// should be avoided wherever possible.
    pub fn diff_raw<CB>(&mut self, other: &mut MMFile, callback: CB) -> Result<(), String>
        where CB: FnMut(&[u8])
    {
        let xpparam = xpparam_t{ flags: 0 };
        let conf = xdemitconf_t{ ctxlen: 3 };
        let mut boxed_cb: Box<dyn FnMut(&[u8])> = Box::new(callback);
        let ptr_to_box = addr_of_mut!(boxed_cb);
        let cb_ptr = ptr_to_box as *mut c_void;
        extern "C" fn emit_cb(cb_ptr: *mut c_void, buffers: *mut mmbuffer_t, num: c_int) -> c_int {
            let ptr_to_box = cb_ptr as *mut Box<dyn FnMut(&[u8])>;

            for i in 0..num {
                let buffer = unsafe { buffers.add(i as usize) };
                let slice = unsafe { from_raw_parts((*buffer).ptr as *const u8, (*buffer).size as usize) };
                // This is unwind safe because our closure only closes over some pointers, no owned objects.
                // After we return an error, the boxed closure will not be called any more,
                // so any broken invariants in its closed-over variables won't be witnessed.
                match catch_unwind(AssertUnwindSafe(|| {
                    unsafe { (*ptr_to_box)(slice) }
                })) {
                    Ok(_) => (),
                    Err(_) => {
                        // TODO: store the panic info somewhere
                        return -1;
                    },
                }
            }
            0
        }
        let mut emit_struct = xdemitcb_t {
            priv_: cb_ptr,
            outf: Some(emit_cb)
        };
        let err = unsafe { xdl_diff(
            addr_of_mut!(self.inner), addr_of_mut!(other.inner),
            addr_of!(xpparam), addr_of!(conf), addr_of_mut!(emit_struct)) };
        if err != 0 {
            Err(format!("diff failed with err: {}", err))
        } else {
            Ok(())
        }
    }

    /// Compute the file that results from merging two sets of changes to the
    /// base file. The resulting file is passed line-by-line to the
    /// `accept_callback`, any conflicting changes are passed to the
    /// `reject_callback`. Returns `Err` if any callback panics; panicking
    /// should be avoided wherever possible.
    pub fn merge3_raw<CBA, CBR>(
        base: &mut MMFile, f1: &mut MMFile, f2: &mut MMFile,
        accept_callback: CBA, reject_callback: CBR,
    ) -> Result<(), String>
        where CBA: FnMut(&[u8]), CBR: FnMut(&[u8])
    {
        // prepare callback for emitting accepted lines
        let mut boxed_acc_cb: Box<dyn FnMut(&[u8])> = Box::new(accept_callback);
        let box_acc_ptr = addr_of_mut!(boxed_acc_cb);
        let void_acc_ptr = box_acc_ptr as *mut c_void;
        extern "C" fn emit_cb(cb_ptr: *mut c_void, buffers: *mut mmbuffer_t, num: c_int) -> c_int {
            let ptr_to_box = cb_ptr as *mut Box<dyn FnMut(&[u8])>;

            for i in 0..num {
                let buffer = unsafe { buffers.add(i as usize) };
                let slice = unsafe { from_raw_parts((*buffer).ptr as *const u8, (*buffer).size as usize) };
                // This is unwind safe because our closure only closes over some pointers, no owned objects.
                // After we return an error, the boxed closure will not be called any more,
                // so any broken invariants in its closed-over variables won't be witnessed.
                match catch_unwind(AssertUnwindSafe(|| {
                    unsafe { (*ptr_to_box)(slice) }
                })) {
                    Ok(_) => (),
                    Err(_) => {
                        // TODO: store the panic info somewhere
                        return -1;
                    },
                }
            }
            0
        }
        let mut emit_struct = xdemitcb_t {
            priv_: void_acc_ptr,
            outf: Some(emit_cb)
        };

        // prepare callback for emitting rejected lines
        let mut boxed_rej_cb: Box<dyn FnMut(&[u8])> = Box::new(reject_callback);
        let box_rej_ptr = addr_of_mut!(boxed_rej_cb);
        let void_rej_ptr = box_rej_ptr as *mut c_void;
        extern "C" fn reject_cb(cb_ptr: *mut c_void, buffers: *mut mmbuffer_t, num: c_int) -> c_int {
            let ptr_to_box = cb_ptr as *mut Box<dyn FnMut(&[u8])>;

            for i in 0..num {
                let buffer = unsafe { buffers.add(i as usize) };
                let slice = unsafe { from_raw_parts((*buffer).ptr as *const u8, (*buffer).size as usize) };
                // This is unwind safe because our closure only closes over some pointers, no owned objects.
                // After we return an error, the boxed closure will not be called any more,
                // so any broken invariants in its closed-over variables won't be witnessed.
                match catch_unwind(AssertUnwindSafe(|| {
                    unsafe { (*ptr_to_box)(slice) }
                })) {
                    Ok(_) => (),
                    Err(_) => {
                        // TODO: store the panic info somewhere
                        return -1;
                    },
                }
            }
            0
        }
        let mut reject_struct = xdemitcb_t {
            priv_: void_rej_ptr,
            outf: Some(reject_cb)
        };
        let err = unsafe { xdl_merge3(
            addr_of_mut!(base.inner), addr_of_mut!(f1.inner), addr_of_mut!(f2.inner),
            addr_of_mut!(emit_struct), addr_of_mut!(reject_struct)) };
        if err != 0 {
            Err(format!("merge failed with err: {}", err))
        } else {
            Ok(())
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

    #[test]
    fn diff_simple() {
        let data = b"hello world\n";
        let mut f = MMFile::from_bytes(data);
        let data2 = b"hello world!\n";
        let mut f2 = MMFile::from_bytes(data2);

        let mut lines = Vec::<Vec<u8>>::new();
        f.diff_raw(&mut f2, |line: &[u8]| {
            lines.push(line.to_owned());
        }).unwrap();

        let str_lines: Vec<String> = lines.iter().map(|l| String::from_utf8_lossy(l).into_owned()).collect();
        assert_eq!(
            str_lines,
            vec![
                "@@ -1,1 +1,1 @@\n",
                "-", "hello world\n",
                "+", "hello world!\n",
            ],
        )
    }

    #[test]
    fn merge3_simple() {
        let data = b"header\nline2\nline3\nline4\nhello world\n";
        let mut f = MMFile::from_bytes(data);
        let data2 = b"header\nline2\nline3\nline4\nhello world changed\n";
        let mut f2 = MMFile::from_bytes(data2);
        let data3 = b"header_changed\nline2\nline3\nline4\nhello world\n";
        let mut f3 = MMFile::from_bytes(data3);

        let mut lines = Vec::<Vec<u8>>::new();
        let mut lines_rejected = Vec::<Vec<u8>>::new();
        MMFile::merge3_raw(&mut f, &mut f2, &mut f3, |line: &[u8]| {
            lines.push(line.to_owned());
        }, |rej_line: &[u8]| {
            lines_rejected.push(rej_line.to_owned());
        }).unwrap();

        let str_lines: Vec<String> = lines.iter().map(|l| String::from_utf8_lossy(l).into_owned()).collect();
        let str_rejected_lines: Vec<String> = lines_rejected.iter().map(|l| String::from_utf8_lossy(l).into_owned()).collect();
        eprintln!("{:?}", str_lines);
        eprintln!("{:?}", str_rejected_lines);
        assert_eq!(
            str_lines,
            vec![
                "header_changed\n",
                "line2\n",
                "line3\n",
                "line4\n",
                "hello world changed\n"
            ],
        );
        assert_eq!(str_rejected_lines, Vec::<String>::new());
    }
}
