use std::env;
use std::fs;
use std::io;
use std::path::Path;

fn main() {
    compile_conch_proto();
    install_guardian_skill();
}

fn compile_conch_proto() {
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("find vendored protoc");
    std::env::set_var("PROTOC", protoc);
    tonic_build::configure()
        .build_server(false)
        .compile_protos(
            &["src/gateway/backend/conch/proto/agent.proto"],
            &["src/gateway/backend/conch/proto"],
        )
        .expect("compile conch agent proto");
}

fn install_guardian_skill() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let src = Path::new(&manifest_dir)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("plugins/skills/xiaoo-guardian");
    println!("cargo:rerun-if-changed={}", src.display());

    let Ok(home) = env::var("HOME") else {
        return;
    };
    let dst = Path::new(&home).join(".xiaoo/skills/xiaoo-guardian");

    if !src.exists() {
        return;
    }

    let Some(skills_dir) = dst.parent() else {
        return;
    };

    if fs::create_dir_all(skills_dir).is_ok() {
        let _ = copy_dir_recursive(&src, &dst);
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> io::Result<()> {
    if !dst.exists() {
        fs::create_dir_all(dst)?;
    }

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let file_name = entry.file_name();
        let src_path = src.join(&file_name);
        let dst_path = dst.join(&file_name);

        if ty.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }

    Ok(())
}
