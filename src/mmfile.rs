use core::{
    ffi::{c_int, c_long, c_void},
    ptr::{addr_of, addr_of_mut},
    slice::from_raw_parts,
};
use std::panic::{catch_unwind, AssertUnwindSafe};

use libxdiff_sys::{
    mmfile_t, xdl_write_mmfile,
    xdl_free_mmfile, xdl_mmfile_cmp, xdl_mmfile_first, xdl_mmfile_size,
    xdl_mmfile_iscompact, xdl_diff, xpparam_t, xdemitconf_t, mmbuffer_t,
    xdemitcb_t, xdl_merge3, xdl_patch, XDL_PATCH_NORMAL,
};

use crate::{MMBlocks, init_mmfile, ensure_init};

type MMPatch = MMBlocks;

/// Type representing an owned memory file in libxdiff.
#[derive(Debug)]
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
        MMFile {
            inner: init_mmfile(0)
        }
    }
    /// Create a new MMFile initialized with contents
    pub fn from_bytes(bytes: &[u8]) -> MMFile {
        let mut inner = init_mmfile(bytes.len());
        ensure_init();

        let bytes_written = unsafe {
            xdl_write_mmfile(addr_of_mut!(inner), bytes.as_ptr() as *const c_void, bytes.len() as c_long) };
        if bytes_written != bytes.len() as i64 {
            panic!("mmfile write only wrote {} bytes when {} were requested", bytes_written, bytes.len());
        }
        MMFile {
            inner,
        }
    }

    /// Get size of stored data in bytes
    pub fn size(&mut self) -> usize {
        unsafe { xdl_mmfile_size(addr_of_mut!(self.inner)) as usize }
    }

    /// Checks if the entire file is a single allocation. In our library this
    /// is always true.
    pub fn is_compact(&mut self) -> bool {
        unsafe { xdl_mmfile_iscompact(addr_of_mut!(self.inner)) != 0 }
    }

    /// Compute the patch to turn self into other
    pub fn compute_patch(&mut self, other: &mut Self) -> Result<MMPatch, String> {
        let mut patch = MMPatch::new();
        unsafe {
            self.diff_raw_nopanic(other, |buf| {
                patch.write_buf(buf)
            })?
        };
        Ok(patch)
    }

    /// Apply a patch to a file. If successful, return the new file. If
    /// unsuccessful, return (successfully patched part, rejected parts)
    pub fn apply_patch(&mut self, patch: &mut MMPatch) -> Result<MMFile, (MMFile, MMFile)> {
        let mut patched = MMPatch::new();
        let mut rejected = MMPatch::new();

        extern "C" fn emit_cb(patch_ptr: *mut c_void, buffers: *mut mmbuffer_t, num: c_int) -> c_int {
            let ptr_to_patch = patch_ptr as *mut MMPatch;
            let patch_ref = unsafe { &mut *ptr_to_patch };

            for i in 0..num {
                let buffer = unsafe { buffers.add(i as usize) };
                let slice = unsafe { from_raw_parts((*buffer).ptr as *const u8, (*buffer).size as usize) };

                let patch_result = patch_ref.write_buf(slice);
                if patch_result < 0 {
                    return patch_result as c_int;
                }
            }
            0
        }
        let mut emit_struct = xdemitcb_t {
            priv_: addr_of_mut!(patched) as *mut c_void,
            outf: Some(emit_cb)
        };
        let mut reject_struct = xdemitcb_t {
            priv_: addr_of_mut!(rejected) as *mut c_void,
            outf: Some(emit_cb)
        };
        let patch_result = unsafe { xdl_patch(
            addr_of_mut!(self.inner),
            addr_of_mut!(patch.inner),
            XDL_PATCH_NORMAL as c_int,
            addr_of_mut!(emit_struct),
            addr_of_mut!(reject_struct),
        )};
        if patch_result == 0 {
            Ok(patched.to_mmfile())
        } else {
            Err((patched.to_mmfile(), rejected.to_mmfile()))
        }
    }

    /// Compute the diff to turn self into other, returning diff through a
    /// callback one line at a time. Returns `Err` if callback panics, but users
    /// should avoid panicking when possible.
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

    /// Compute the diff to turn self into other, returning diff through a
    /// callback one line at a time. Callback should return 0 on success and -1
    /// on failure.
    ///
    /// SAFETY: callback must not panic
    pub unsafe fn diff_raw_nopanic<CB>(&mut self, other: &mut MMFile, callback: CB) -> Result<(), String>
        where CB: FnMut(&[u8]) -> c_int
    {
        let xpparam = xpparam_t{ flags: 0 };
        let conf = xdemitconf_t{ ctxlen: 3 };
        let mut boxed_cb: Box<dyn FnMut(&[u8]) -> c_int> = Box::new(callback);
        let ptr_to_box = addr_of_mut!(boxed_cb);
        let cb_ptr = ptr_to_box as *mut c_void;
        extern "C" fn emit_cb(cb_ptr: *mut c_void, buffers: *mut mmbuffer_t, num: c_int) -> c_int {
            let ptr_to_box = cb_ptr as *mut Box<dyn FnMut(&[u8]) -> c_int>;

            for i in 0..num {
                let buffer = unsafe { buffers.add(i as usize) };
                let slice = unsafe { from_raw_parts((*buffer).ptr as *const u8, (*buffer).size as usize) };
                let cb_result = unsafe { (*ptr_to_box)(slice) };
                if cb_result < 0 {
                    return cb_result as c_int;
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
            Err(format!("diff failed with errno: {}", err))
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
                // This is unwind safe because our closure only closes over pointers, no owned objects that would run destructors.
                // After we return an error, the boxed closure will not be called any more,
                // so any broken invariants in its closed-over variables won't be witnessed.
                match catch_unwind(AssertUnwindSafe(|| {
                    unsafe { (*ptr_to_box)(slice) }
                })) {
                    Ok(_) => (),
                    Err(_) => {
                        // TODO: maybe store the panic info somewhere?
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
    /// Compare contents of 2 files for equality. The underlying structs track
    /// their own iterator state, so comparison requires mutable access.
    pub fn eq(&mut self, other: &mut Self) -> bool {
        unsafe {
            xdl_mmfile_cmp(
                addr_of_mut!(self.inner),
                addr_of_mut!(other.inner)) == 0}
    }

    /// Get a view of the `MMFile`'s data as a slice
    pub fn as_slice(&mut self) -> &[u8] {
        assert!(self.is_compact());
        let mut len: c_long = 0;
        let block_ptr = unsafe {
            xdl_mmfile_first(addr_of_mut!(self.inner),
            addr_of_mut!(len)) as *mut u8
        };
        if block_ptr.is_null() || len <= 0 {
            &[]
        } else {
            unsafe {
                core::slice::from_raw_parts(block_ptr, len as usize)
            }
        }
    }

    /// Get a mutable view of the `MMFile`'s data as a slice
    pub fn as_slice_mut(&mut self) -> &mut [u8] {
        assert!(self.is_compact());
        let mut len: c_long = 0;
        let block_ptr = unsafe {
            xdl_mmfile_first(addr_of_mut!(self.inner),
            addr_of_mut!(len)) as *mut u8
        };
        if block_ptr.is_null() || len <= 0 {
            &mut []
        } else {
            unsafe {
                core::slice::from_raw_parts_mut(block_ptr, len as usize)
            }
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
    fn eq() {
        let mut data = Vec::new();
        data.extend((0..240).cycle().take(15_000));
        let mut f = MMFile::from_bytes(data.as_slice());
        let mut f2 = MMFile::from_bytes(data.as_slice());
        assert!(f.eq(&mut f2));
    }

    #[test]
    fn as_slice() {
        assert_eq!(MMFile::new().as_slice(), &[]);

        let mut data = Vec::new();
        data.extend((0..240).cycle().take(15_000));
        let mut f = MMFile::from_bytes(data.as_slice());
        assert_eq!(f.as_slice(), &data);
    }

    #[test]
    fn as_slice_mut() {
        assert_eq!(MMFile::new().as_slice_mut(), &mut []);

        let mut data = Vec::new();
        data.extend((0..240).cycle().take(15_000));
        let mut f = MMFile::from_bytes(data.as_slice());
        assert_eq!(f.as_slice()[0], data[0]);

        f.as_slice_mut()[0] += 1;
        assert_eq!(f.as_slice()[0], data[0] + 1);
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
    fn diff_panic() {
        let data = b"hello world\n";
        let mut f = MMFile::from_bytes(data);
        let data2 = b"hello world!\n";
        let mut f2 = MMFile::from_bytes(data2);

        let mut lines = Vec::<Vec<u8>>::new();
        let diff_result = f.diff_raw(&mut f2, |line: &[u8]| {
            if lines.len() > 1 {
                panic!("too many lines!");
            }
            lines.push(line.to_owned());
        });
        assert_eq!(diff_result, Result::Err("diff failed with err: -1".to_owned()));
    }

    #[test]
    fn diff_with_mutation() {
        // do the simple diff first
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
        );
        // now change a letter and run the diff again
        f2.as_slice_mut()[0] = "j".as_bytes()[0];
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
                "+", "jello world!\n",  // first letter is now different
            ],
        );
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

    #[test]
    fn merge3_conflicts() {
        let data = b"header\nline2\nline3\nline4\nhello world\n";
        let mut f = MMFile::from_bytes(data);
        let data2 = b"header\nline2\nline3\nline4\nhello world changed\n";
        let mut f2 = MMFile::from_bytes(data2);
        let data3 = b"header\nline2\nline3\nline4\nhello world also changed\n";
        let mut f3 = MMFile::from_bytes(data3);

        let mut lines = Vec::<Vec<u8>>::new();
        let mut lines_rejected = Vec::<Vec<u8>>::new();
        MMFile::merge3_raw(&mut f, &mut f2, &mut f3,
            |line: &[u8]| {
                lines.push(line.to_owned());
            },
            |rej_line: &[u8]| {
                lines_rejected.push(rej_line.to_owned());
            }
        ).unwrap();

        let str_lines: Vec<String> = lines.iter().map(|l| String::from_utf8_lossy(l).into_owned()).collect();
        let str_rejected_lines: Vec<String> = lines_rejected.iter().map(|l| String::from_utf8_lossy(l).into_owned()).collect();
        eprintln!("{:?}", str_lines);
        eprintln!("{:?}", str_rejected_lines);
        assert_eq!(
            str_lines,
            vec![
                "header\n",
                "line2\n",
                "line3\n",
                "line4\n",
                "hello world changed\n"
            ],
        );
        assert_eq!(
            str_rejected_lines,
            vec![
                "@@ -2,4 +2,4 @@\n",
                " line2\n",
                " line3\n",
                " line4\n",
                "-hello world\n", "+hello world also changed\n",
            ],
        );
    }

    #[test]
    fn merge3_panic_emit() {
        let data = b"header\nline2\nline3\nline4\nhello world\n";
        let mut f = MMFile::from_bytes(data);
        let data2 = b"header\nline2\nline3\nline4\nhello world changed\n";
        let mut f2 = MMFile::from_bytes(data2);
        let data3 = b"header_changed\nline2\nline3\nline4\nhello world\n";
        let mut f3 = MMFile::from_bytes(data3);

        let mut lines = Vec::<Vec<u8>>::new();
        let mut lines_rejected = Vec::<Vec<u8>>::new();
        let merge_result = MMFile::merge3_raw(&mut f, &mut f2, &mut f3, |line: &[u8]| {
            if lines.len() > 2 {
                panic!("too many lines!");
            }
            lines.push(line.to_owned());
        }, |rej_line: &[u8]| {
            lines_rejected.push(rej_line.to_owned());
        });

        assert_eq!(merge_result, Result::Err("merge failed with err: -1".to_owned()));
    }

    #[test]
    fn merge3_panic_reject() {
        let data = b"header\nline2\nline3\nline4\nhello world\n";
        let mut f = MMFile::from_bytes(data);
        let data2 = b"header\nline2\nline3\nline4\nhello world changed\n";
        let mut f2 = MMFile::from_bytes(data2);
        let data3 = b"header\nline2\nline3\nline4\nhello world changed\n";
        let mut f3 = MMFile::from_bytes(data3);

        let mut lines = Vec::<Vec<u8>>::new();
        let mut lines_rejected = Vec::<Vec<u8>>::new();
        let merge_result = MMFile::merge3_raw(&mut f, &mut f2, &mut f3, |line: &[u8]| {
            lines.push(line.to_owned());
        }, |rej_line: &[u8]| {
            if lines_rejected.len() > 2 {
                panic!("too many lines!");
            }
            lines_rejected.push(rej_line.to_owned());
        });

        assert_eq!(merge_result, Result::Err("merge failed with err: -1".to_owned()));
    }

    #[test]
    fn patch_simple() {
        let data = b"header\nline2\nline3\nline4\nhello world\n";
        let mut f = MMFile::from_bytes(data);
        let data2 = b"header\nline2\nline3\nline4\nhello world changed\n";
        let mut f2 = MMFile::from_bytes(data2);
        let data3 = b"header\nline2\nline3\nline4\nhello world changed\n";
        let mut f3 = MMFile::from_bytes(data3);

        let mut patch = f.compute_patch(&mut f2).unwrap();
        eprintln!("patch: {:?}", patch);

        let mut patch_result = f.apply_patch(&mut patch).unwrap();

        assert!(patch_result.eq(&mut f3));

    }
}
