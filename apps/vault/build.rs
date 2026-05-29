fn main() {
    // 当启用 tee_sdf 特性时，链接 libsdf.so
    if cfg!(feature = "tee_sdf") {
        println!("cargo:rustc-link-search=native=/usr/local/sdf/lib");
        println!("cargo:rustc-link-lib=dylib=sdf");

        // 运行时库搜索路径
        println!("cargo:rustc-rpath=/usr/local/sdf/lib");
    }

    // 输出构建信息
    println!("cargo:rerun-if-changed=build.rs");
}