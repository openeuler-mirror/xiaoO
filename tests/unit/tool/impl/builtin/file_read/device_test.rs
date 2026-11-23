use super::*;

/// `environ` / `cmdline` / `mem` / `maps` leak the daemon's secrets and
/// memory, so every owner spelling must be rejected: self, a numeric pid,
/// thread-self, and the per-thread `task/<tid>` form.
#[test]
fn process_scoped_files_are_blocked_for_every_owner() {
    for path in [
        "/proc/self/environ",
        "/proc/self/cmdline",
        "/proc/self/mem",
        "/proc/self/maps",
        "/proc/1/environ",
        "/proc/thread-self/environ",
        "/proc/self/task/123/environ",
        "/proc/42/task/7/maps",
        // Same leak class: smaps is a superset of maps, numa_maps carries
        // per-node layout, auxv carries the stack canary and AT_RANDOM.
        "/proc/self/smaps",
        "/proc/self/smaps_rollup",
        "/proc/self/numa_maps",
        "/proc/self/auxv",
        "/proc/1/task/2/smaps",
    ] {
        assert!(
            is_blocked_device_path(path),
            "{path} leaks process state and must be blocked"
        );
    }
}

/// Ordinary files and harmless `/proc` entries stay readable — the guard must
/// not turn into a blanket `/proc` ban.
#[test]
fn harmless_paths_remain_allowed() {
    for path in [
        "/etc/passwd",
        "/dev/null",
        "/proc/self/stat",
        "/proc/self/status",
        "/proc/meminfo",
        "/proc/cpuinfo",
        "/proc/self/environment",
    ] {
        assert!(!is_blocked_device_path(path), "{path} should stay readable");
    }
}

/// The device-file and std-fd guards added earlier keep working.
#[test]
fn existing_device_and_std_fd_guards_still_apply() {
    for path in ["/dev/zero", "/dev/urandom", "/dev/stdin", "/proc/self/fd/0"] {
        assert!(is_blocked_device_path(path), "{path} must stay blocked");
    }
}
