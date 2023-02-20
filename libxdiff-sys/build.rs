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

    // Tell cargo to look for shared libraries in the specified directory
    println!("cargo:rustc-link-search={}", libs_path.to_str().unwrap());

    println!("cargo:rustc-link-lib=static=xdiff");

    // Tell cargo to invalidate the built crate whenever the header changes.
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

    // The bindgen::Builder is the main entry point
    // to bindgen, and lets you build up options for
    // the resulting bindings.
    let bindings = bindgen::Builder::default()
        // The input header we would like to generate
        // bindings for.
        .header(header_path_str)
        // Tell cargo to invalidate the built crate whenever any of the
        // included header files changed.
        .parse_callbacks(Box::new(CargoCallbacks))
        // Finish the builder and generate the bindings.
        .generate()
        // Unwrap the Result and panic on failure.
        .expect("Unable to generate bindings");

    // Write the bindings to the $OUT_DIR/bindings.rs file.
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("bindings.rs");
    bindings
        .write_to_file(out_path)
        .expect("Couldn't write bindings!");
}
