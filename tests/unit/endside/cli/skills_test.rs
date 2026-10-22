use super::*;
use std::fs;
use tempfile::tempdir;

#[test]
fn copy_dir_rejects_destination_inside_source() {
    let temp = tempdir().unwrap();
    let src = temp.path().join("skills");
    fs::create_dir(&src).unwrap();
    fs::write(src.join("SKILL.md"), "test").unwrap();

    let err = copy_dir_recursive(&src, &src.join("nested")).unwrap_err();

    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
}

#[cfg(unix)]
#[test]
fn copied_executable_runs_during_concurrent_process_creation() {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    let temp = tempdir().unwrap();
    let src = temp.path().join("src");
    fs::create_dir(&src).unwrap();
    {
        let _permit = xiaoo_shared::process_support::write_permit_blocking();
        fs::write(src.join("product-executable.sh"), "#!/bin/sh\nexit 0\n").unwrap();
    }
    fs::set_permissions(
        src.join("product-executable.sh"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let spawns = std::thread::spawn({
        let stop = Arc::clone(&stop);
        move || {
            for _ in 0..512 {
                if stop.load(Ordering::Acquire) {
                    break;
                }
                xiaoo_shared::process_support::output_blocking(&mut std::process::Command::new(
                    "/bin/true",
                ))
                .unwrap();
            }
        }
    });
    let dest = temp.path().join("dest");
    let copied = copy_dir_recursive(&src, &dest);
    let result = copied
        .and_then(|_| {
            xiaoo_shared::process_support::spawn_std_blocking(&mut std::process::Command::new(
                dest.join("product-executable.sh"),
            ))
        })
        .and_then(|mut child| child.wait());
    stop.store(true, Ordering::Release);
    spawns.join().unwrap();
    assert!(result.unwrap().success());
}
