// hivex (LGPL-2.1) is linked dynamically: install it from the distribution (e.g. pacman -S hivex,
// apt install libhivex0 libhivex-dev). HIVEX_LIB_DIR adds a directory to search.
fn main() {
    println!("cargo:rerun-if-env-changed=HIVEX_LIB_DIR");
    if let Ok(dir) = std::env::var("HIVEX_LIB_DIR") {
        println!("cargo:rustc-link-search=native={dir}");
    }
    println!("cargo:rustc-link-lib=dylib=hivex");
}
