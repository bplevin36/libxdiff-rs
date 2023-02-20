extern crate bindgen;

use std::env;
use std::path::PathBuf;

use bindgen::CargoCallbacks;

const VENDORED: &'static str = "./libxdiff-0.23";

fn main() {
    let libxdiff_path = PathBuf::from(VENDORED)
        .canonicalize()
        .expect("cannot canonicalize path");

    let xdiff_path = libxdiff_path.join("xdiff");
    let header_path = xdiff_path.join("xdiff.h");
    let header_path_str = header_path.to_str()
        .expect("Path is not a valid string");

    let libs_path = xdiff_path.join(".libs");

    println!("cargo:rustc-link-search={}", libs_path.to_str().unwrap());
    println!("cargo:rustc-link-lib=static=xdiff");
    println!("cargo:rerun-if-changed={}", header_path_str);

    let configure_path = libxdiff_path.join("configure");
    match std::process::Command::new(configure_path)
        .current_dir(&libxdiff_path)
        .arg("--enable-shared=no")
        .arg("--enable-static=yes")
        .output()
    {
        Ok(_) => (),
        Err(e) => {
            eprintln!("{}", e);
            panic!("could not configure");
        },
    };

    match std::process::Command::new("make")
        .current_dir(&libxdiff_path)
        .output()
    {
        Ok(_) => (),
        Err(e) => {
            eprintln!("{}", e);
            panic!("could not make");
        },
    }

    let bindings = bindgen::Builder::default()
        .header(header_path_str)
        .parse_callbacks(Box::new(CargoCallbacks))
        .generate()
        .expect("Unable to generate bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("bindings.rs");
    bindings
        .write_to_file(out_path)
        .expect("Couldn't write bindings!");
}
