use super::{compute_local_host_identity, local_host_identity, local_hostname, trimmed_non_empty};

/// 取值优先级:machine-id 可读且非空时它就是标识;否则落到 hostname
/// 回退链。两条分支在任何机器上都可判定,不依赖测试机具体形态。
#[test]
fn machine_id_takes_priority_over_hostname_fallback() {
    let machine_id = super::read_trimmed("/etc/machine-id");
    let identity = compute_local_host_identity();
    match machine_id {
        Some(machine_id) => {
            assert_eq!(identity.as_deref(), Some(machine_id.as_str()));
        }
        None => assert_eq!(identity, local_hostname()),
    }
}

/// 两次调用返回同一值:标识经进程内缓存,漂移会让 /remote 比对误判。
#[test]
fn repeated_calls_return_the_same_identity() {
    assert_eq!(local_host_identity(), local_host_identity());
}

#[test]
fn trimmed_non_empty_drops_blank_and_trims() {
    assert_eq!(trimmed_non_empty("  \n\t ".to_string()), None);
    assert_eq!(
        trimmed_non_empty(" host-a \n".to_string()),
        Some("host-a".to_string())
    );
}
