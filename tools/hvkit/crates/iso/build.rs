// libisofs (GPL-2.0-or-later) is linked dynamically: install it from the distribution (pacman -S
// libisofs, apt install libisofs-dev). A binary built with it falls under the GPL; hvkit makes it an
// optional feature (iso) for that reason.
fn main() {
    println!("cargo:rerun-if-changed=src/shim.c");
    cc::Build::new()
        .file("src/shim.c")
        .warnings(true)
        .extra_warnings(true)
        .compile("isoshim");
    println!("cargo:rustc-link-lib=dylib=isofs");
}
