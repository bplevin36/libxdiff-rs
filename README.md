# libxdiff
Safe, idiomatic Rust bindings for the `libxdiff` C library.

[![Crates.io](https://img.shields.io/crates/v/libxdiff)](https://crates.io/crates/libxdiff)
[![docs](https://docs.rs/libxdiff/badge.svg)](https://docs.rs/libxdiff)

## Usage
Add this to your `Cargo.toml`:

```toml
[dependencies]
libxdiff = "0.2"
```

## Example
```rust
use core::str::from_utf8;
use libxdiff::MMFile;

let mut f1 = MMFile::from_bytes(b"hello world\n");
let mut f2 = MMFile::from_bytes(b"hello world!\n");
let mut diff_lines = Vec::<String>::new();
    f1.diff_raw(&mut f2, |line: &[u8]| {
        diff_lines.push(from_utf8(line).unwrap().to_owned());
    })
    .unwrap();
assert_eq!(
    diff_lines,
    vec![
        "@@ -1,1 +1,1 @@\n",
        "-", "hello world\n",
        "+", "hello world!\n",
    ],
);
```

## Linkage
Upstream `libxdiff` is small and has no dependencies, so this crate links it statically.
