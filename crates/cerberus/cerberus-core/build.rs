//! Build script for compiling eBPF programs.
//!
//! This build script compiles the eBPF audit program when the `ebpf` feature
//! is enabled. The compiled eBPF object is embedded in the library for
//! runtime loading.

#[cfg(feature = "ebpf")]
fn main() -> anyhow::Result<()> {
    use anyhow::{anyhow, Context as _};
    use aya_build::Toolchain;
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").context("CARGO_MANIFEST_DIR not set")?;
    let manifest_path = std::path::Path::new(&manifest_dir)
        .join("bpf")
        .join("Cargo.toml");

    if !manifest_path.is_file() {
        return Err(anyhow!(
            "audit-bpf manifest not found at {}",
            manifest_path.display()
        ));
    }

    let bpf_root = manifest_path
        .parent()
        .ok_or_else(|| anyhow!("no parent for {}", manifest_path.display()))?;

    let bpf_root = bpf_root
        .to_str()
        .ok_or_else(|| anyhow!("invalid UTF-8 path: {}", bpf_root.display()))?;

    let ebpf_package = aya_build::Package {
        name: "audit-bpf",
        root_dir: bpf_root,
        features: &["eBPF"],
        ..Default::default()
    };

    aya_build::build_ebpf([ebpf_package], Toolchain::default())
}

#[cfg(not(feature = "ebpf"))]
fn main() {}
