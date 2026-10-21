use super::{tls_verification_disabled, tls_verification_disabled_from};

#[test]
fn tls_verification_is_disabled_only_by_an_explicit_zero() {
    assert!(tls_verification_disabled_from(Some("0")));
    assert!(tls_verification_disabled_from(Some(" 0 ")), "允许两侧空白");
    assert!(!tls_verification_disabled_from(Some("1")));
    assert!(!tls_verification_disabled_from(Some("")), "空串不等于 0");
    assert!(
        !tls_verification_disabled_from(None),
        "未设置时必须保持严格校验"
    );
}

/// 上面钉的是判据本身，这里钉"判据读的是哪个环境变量"：变量名写错、或者
/// `tls_verification_disabled()` 哪天改成读别的开关，都必须在这里失败。
/// （`temp_env` 会串行化它自己的调用；只有读这个开关的构造路径会受影响。）
#[test]
fn tls_verification_disabled_reads_the_process_env() {
    temp_env::with_var("NODE_TLS_REJECT_UNAUTHORIZED", Some("0"), || {
        assert!(tls_verification_disabled(), "=0 应当关闭校验");
    });
    temp_env::with_var("NODE_TLS_REJECT_UNAUTHORIZED", Some("1"), || {
        assert!(!tls_verification_disabled(), "=1 必须保持严格校验");
    });
    temp_env::with_var("NODE_TLS_REJECT_UNAUTHORIZED", None::<&str>, || {
        assert!(!tls_verification_disabled(), "未设置必须保持严格校验");
    });
}
