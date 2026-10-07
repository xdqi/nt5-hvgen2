// hivex (LGPL-2.1) is linked dynamically: install it from the distribution (e.g. pacman -S hivex,
// apt install libhivex0 libhivex-dev). HIVEX_LIB_DIR adds a directory to search. Windows builds use
// offreg.dll instead, which is loaded at run time.
fn main() {
    println!("cargo:rerun-if-env-changed=HIVEX_LIB_DIR");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        return;
    }
    if let Ok(dir) = std::env::var("HIVEX_LIB_DIR") {
        println!("cargo:rustc-link-search=native={dir}");
    }
    println!("cargo:rustc-link-lib=dylib=hivex");
}
