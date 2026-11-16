use super::{
    http1_forced, http1_forced_from, tls_verification_disabled, tls_verification_disabled_from,
};

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

/// 与 TLS 开关对称：钉"锁回 HTTP/1.1"的判据本身。语义与
/// `XIAOO_DISABLE_OPENROUTER_LOOKUP` 一致——置为 `0`/`false`/空串 视为未开启，
/// 其余任意值（含空白包裹）开启。
#[test]
fn http1_is_forced_by_anything_except_explicit_opt_outs() {
    assert!(http1_forced_from(Some("1")));
    assert!(http1_forced_from(Some("true")));
    assert!(http1_forced_from(Some(" 1 ")), "允许两侧空白");
    assert!(!http1_forced_from(Some("0")));
    assert!(!http1_forced_from(Some("false")));
    assert!(!http1_forced_from(Some("")), "空串视为未开启");
    assert!(
        !http1_forced_from(None),
        "未设置时保持默认：允许 ALPN 协商出 h2"
    );
}

/// 上面钉的是判据本身，这里钉"判据读的是哪个环境变量"：变量名写错、或者
/// `http1_forced()` 哪天改成读别的开关，都必须在这里失败。
#[test]
fn http1_forced_reads_the_process_env() {
    temp_env::with_var("XIAOO_LLM_FORCE_HTTP1", Some("1"), || {
        assert!(http1_forced(), "=1 应当锁回 HTTP/1.1");
    });
    temp_env::with_var("XIAOO_LLM_FORCE_HTTP1", Some("0"), || {
        assert!(!http1_forced(), "=0 必须保持默认协议协商");
    });
    temp_env::with_var("XIAOO_LLM_FORCE_HTTP1", None::<&str>, || {
        assert!(!http1_forced(), "未设置必须保持默认协议协商");
    });
}
