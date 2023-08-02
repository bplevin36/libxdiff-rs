use crate::{MMBlocks, MMFile};

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
fn clone() {
    let mut data = Vec::new();
    data.extend((0..240).cycle().take(15_000));
    let f = MMFile::from_bytes(data.as_slice());
    let f2 = f.clone();
    assert!(f == f2);
}

#[test]
fn clone_blocks() {
    let mut data = Vec::new();
    data.extend((0..240).cycle().take(15_000));
    let mut f = MMBlocks::from_bytes(data.as_slice());
    let mut f2 = f.clone();
    assert!(f.eq(&mut f2));
}

#[test]
fn eq() {
    let mut data = Vec::new();
    data.extend((0..240).cycle().take(15_000));
    let f = MMFile::from_bytes(data.as_slice());
    let mut f2 = MMFile::from_bytes(data.as_slice());
    assert!(f.eq(&mut f2));
}

#[test]
fn as_slice() {
    assert_eq!(MMFile::new().as_slice(), &[]);

    let mut data = Vec::new();
    data.extend((0..240).cycle().take(15_000));
    let f = MMFile::from_bytes(data.as_slice());
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
    })
    .unwrap();

    let str_lines: Vec<String> = lines
        .iter()
        .map(|l| String::from_utf8_lossy(l).into_owned())
        .collect();
    assert_eq!(
        str_lines,
        vec![
            "@@ -1,1 +1,1 @@\n",
            "-",
            "hello world\n",
            "+",
            "hello world!\n",
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
    assert_eq!(
        diff_result,
        Result::Err("diff failed with err: -1".to_owned())
    );
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
    })
    .unwrap();

    let str_lines: Vec<String> = lines
        .iter()
        .map(|l| String::from_utf8_lossy(l).into_owned())
        .collect();
    assert_eq!(
        str_lines,
        vec![
            "@@ -1,1 +1,1 @@\n",
            "-",
            "hello world\n",
            "+",
            "hello world!\n",
        ],
    );
    // now change a letter and run the diff again
    f2.as_slice_mut()[0] = "j".as_bytes()[0];
    let mut lines = Vec::<Vec<u8>>::new();
    f.diff_raw(&mut f2, |line: &[u8]| {
        lines.push(line.to_owned());
    })
    .unwrap();

    let str_lines: Vec<String> = lines
        .iter()
        .map(|l| String::from_utf8_lossy(l).into_owned())
        .collect();
    assert_eq!(
        str_lines,
        vec![
            "@@ -1,1 +1,1 @@\n",
            "-",
            "hello world\n",
            "+",
            "jello world!\n", // first letter is now different
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
    MMFile::merge3_raw(
        &mut f,
        &mut f2,
        &mut f3,
        |line: &[u8]| {
            lines.push(line.to_owned());
        },
        |rej_line: &[u8]| {
            lines_rejected.push(rej_line.to_owned());
        },
    )
    .unwrap();

    let str_lines: Vec<String> = lines
        .iter()
        .map(|l| String::from_utf8_lossy(l).into_owned())
        .collect();
    let str_rejected_lines: Vec<String> = lines_rejected
        .iter()
        .map(|l| String::from_utf8_lossy(l).into_owned())
        .collect();
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
    MMFile::merge3_raw(
        &mut f,
        &mut f2,
        &mut f3,
        |line: &[u8]| {
            lines.push(line.to_owned());
        },
        |rej_line: &[u8]| {
            lines_rejected.push(rej_line.to_owned());
        },
    )
    .unwrap();

    let str_lines: Vec<String> = lines
        .iter()
        .map(|l| String::from_utf8_lossy(l).into_owned())
        .collect();
    let str_rejected_lines: Vec<String> = lines_rejected
        .iter()
        .map(|l| String::from_utf8_lossy(l).into_owned())
        .collect();
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
            "-hello world\n",
            "+hello world also changed\n",
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
    let merge_result = MMFile::merge3_raw(
        &mut f,
        &mut f2,
        &mut f3,
        |line: &[u8]| {
            if lines.len() > 2 {
                panic!("too many lines!");
            }
            lines.push(line.to_owned());
        },
        |rej_line: &[u8]| {
            lines_rejected.push(rej_line.to_owned());
        },
    );

    assert_eq!(
        merge_result,
        Result::Err("merge failed with err: -1".to_owned())
    );
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
    let merge_result = MMFile::merge3_raw(
        &mut f,
        &mut f2,
        &mut f3,
        |line: &[u8]| {
            lines.push(line.to_owned());
        },
        |rej_line: &[u8]| {
            if lines_rejected.len() > 2 {
                panic!("too many lines!");
            }
            lines_rejected.push(rej_line.to_owned());
        },
    );

    assert_eq!(
        merge_result,
        Result::Err("merge failed with err: -1".to_owned())
    );
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

    let patch_result = f.apply_patch(&mut patch).unwrap();

    assert!(patch_result.eq(&mut f3));
}

#[test]
fn patch_reject() {
    let data = b"header\nline2\nline3\nline4\nhello world\n";
    let mut f = MMFile::from_bytes(data);
    let data2 = b"header changed\nline2\nline3\nline4\nhello world changed\n";
    let mut f2 = MMFile::from_bytes(data2);

    let mut patch = f.compute_patch(&mut f2).unwrap();

    eprintln!("patch: {:?}", patch.clone().to_mmfile());
    // modify base file to break patch assumptions
    f.as_slice_mut()[0] = "b".as_bytes()[0];

    let patch_result = f.apply_patch(&mut patch);

    // when patch fails, original file is returned alongside failed patch segments
    assert_eq!(patch_result, Err((f.clone(), patch.clone().to_mmfile())));
}
