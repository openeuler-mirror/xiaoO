fn main() {
    println!("cargo:rerun-if-env-changed=XIAOO_RELEASE_VERSION");
    println!("cargo:rerun-if-env-changed=XIAOO_BUILD_COMMIT");
    let version = std::env::var("XIAOO_RELEASE_VERSION").ok();
    let commit = std::env::var("XIAOO_BUILD_COMMIT").ok();
    let (version, commit) = match (version, commit) {
        (Some(version), Some(commit)) => {
            semver::Version::parse(&version)
                .expect("XIAOO_RELEASE_VERSION must be a semantic version");
            assert!(
                commit.len() == 40 && commit.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "XIAOO_BUILD_COMMIT must be a full Git SHA"
            );
            (version, commit.to_ascii_lowercase())
        }
        (None, None) => (env!("CARGO_PKG_VERSION").to_string(), "unknown".to_string()),
        _ => panic!("XIAOO_RELEASE_VERSION and XIAOO_BUILD_COMMIT must be set together"),
    };
    println!("cargo:rustc-env=XIAOO_VERSION={version}");
    println!("cargo:rustc-env=XIAOO_COMMIT={commit}");
}
