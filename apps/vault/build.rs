fn main() {
    // Link against libsdf.so only when the tee_sdf feature is enabled.
    if cfg!(feature = "tee_sdf") {
        println!("cargo:rustc-link-search=native=/usr/local/sdf/lib");
        println!("cargo:rustc-link-lib=dylib=sdf");

        println!("cargo:rustc-rpath=/usr/local/sdf/lib");
    }

    println!("cargo:rerun-if-changed=build.rs");
}
