use std::env;
use std::fs;
use std::path::Path;

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let src = Path::new(&manifest_dir)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("plugin/security/xiaoo-guardian");

    let home = env::var("HOME").expect("HOME not set");
    let dst = Path::new(&home).join(".xiaoo/skills/xiaoo-guardian");

    if !src.exists() {
        println!(
            "cargo:warning=Source skill directory not found: {}",
            src.display()
        );
        return;
    }

    fs::create_dir_all(dst.parent().unwrap()).expect("Failed to create skills directory");
    copy_dir_recursive(&src, &dst);
    println!(
        "cargo:warning=Installed xiaoo-guardian skill to {}",
        dst.display()
    );
}

fn copy_dir_recursive(src: &Path, dst: &Path) {
    if !dst.exists() {
        fs::create_dir_all(dst).expect("Failed to create directory");
    }

    for entry in fs::read_dir(src).expect("Failed to read directory") {
        let entry = entry.expect("Failed to read entry");
        let ty = entry.file_type().expect("Failed to get file type");
        let file_name = entry.file_name();
        let src_path = src.join(&file_name);
        let dst_path = dst.join(&file_name);

        if ty.is_dir() {
            copy_dir_recursive(&src_path, &dst_path);
        } else {
            fs::copy(&src_path, &dst_path).expect("Failed to copy file");
        }
    }
}
