use super::*;
use crate::r#impl::builtin::webfetch::input::WebFetchFormat;

#[test]
fn test_is_ip_blocks_loopback_ipv4() {
    assert!(is_ip_blocked(IpAddr::V4("127.0.0.1".parse().unwrap())));
    assert!(is_ip_blocked(IpAddr::V4("127.0.0.2".parse().unwrap())));
    assert!(is_ip_blocked(IpAddr::V4(
        "127.255.255.255".parse().unwrap()
    )));
}

#[test]
fn test_is_ip_blocks_loopback_ipv6() {
    assert!(is_ip_blocked(IpAddr::V6("::1".parse().unwrap())));
}

#[test]
fn test_is_ip_blocks_rfc1918_private() {
    assert!(is_ip_blocked(IpAddr::V4("10.0.0.1".parse().unwrap())));
    assert!(is_ip_blocked(IpAddr::V4("10.255.255.255".parse().unwrap())));

    assert!(is_ip_blocked(IpAddr::V4("172.16.0.1".parse().unwrap())));
    assert!(is_ip_blocked(IpAddr::V4("172.31.255.255".parse().unwrap())));
    assert!(!is_ip_blocked(IpAddr::V4("172.32.0.1".parse().unwrap())));
    assert!(is_ip_blocked(IpAddr::V4("192.168.0.1".parse().unwrap())));
    assert!(is_ip_blocked(IpAddr::V4(
        "192.168.255.255".parse().unwrap()
    )));
}

#[test]
fn test_is_ip_blocks_link_local() {
    assert!(is_ip_blocked(IpAddr::V4(
        "169.254.169.254".parse().unwrap()
    )));
    assert!(is_ip_blocked(IpAddr::V4("169.254.1.1".parse().unwrap())));

    assert!(is_ip_blocked(IpAddr::V6("fe80::1".parse().unwrap())));
}

#[test]
fn test_is_ip_blocks_unspecified() {
    assert!(is_ip_blocked(IpAddr::V4("0.0.0.0".parse().unwrap())));
    assert!(is_ip_blocked(IpAddr::V6("::".parse().unwrap())));
}

#[test]
fn test_is_ip_blocks_ipv4_mapped_ipv6() {
    assert!(is_ip_blocked(IpAddr::V6(
        "::ffff:127.0.0.1".parse().unwrap()
    )));
    assert!(is_ip_blocked(IpAddr::V6(
        "::ffff:10.0.0.1".parse().unwrap()
    )));
}

#[test]
fn test_is_ip_allows_public_ips() {
    assert!(!is_ip_blocked(IpAddr::V4("8.8.8.8".parse().unwrap())));
    assert!(!is_ip_blocked(IpAddr::V4("1.1.1.1".parse().unwrap())));
    assert!(!is_ip_blocked(IpAddr::V6(
        "2001:4860:4860::8888".parse().unwrap()
    )));
}

#[test]
fn test_extract_host_basic() {
    assert_eq!(extract_host("https://example.com/path"), "example.com");
    assert_eq!(extract_host("http://example.com"), "example.com");
}

#[test]
fn test_extract_host_with_port() {
    assert_eq!(extract_host("https://example.com:443/path"), "example.com");
    assert_eq!(extract_host("http://example.com:8080"), "example.com");
}

#[test]
fn test_extract_host_ipv4() {
    assert_eq!(extract_host("http://127.0.0.1:8080/path"), "127.0.0.1");
    assert_eq!(extract_host("http://192.168.1.1/api"), "192.168.1.1");
}

#[test]
fn test_extract_host_ipv6() {
    assert_eq!(extract_host("http://[::1]:8080/"), "[::1]");
    assert_eq!(
        extract_host("http://[::ffff:127.0.0.1]/path"),
        "[::ffff:127.0.0.1]"
    );
}

#[test]
fn test_extract_host_with_userinfo() {
    assert_eq!(
        extract_host("http://user:pass@example.com/path"),
        "example.com"
    );
}

#[test]
fn test_validate_url_blocks_loopback() {
    let input = WebFetchInput {
        url: "http://127.0.0.1:8080/admin".to_string(),
        format: WebFetchFormat::Text,
        timeout: None,
    };
    let result = validate_url(&input);
    assert!(!result.result);
    assert_eq!(result.error_code, Some(error_code::URL_BLOCKED_IP));
}

#[test]
fn test_validate_url_blocks_aws_metadata() {
    let input = WebFetchInput {
        url: "http://169.254.169.254/latest/meta-data/".to_string(),
        format: WebFetchFormat::Text,
        timeout: None,
    };
    let result = validate_url(&input);
    assert!(!result.result);
    assert_eq!(result.error_code, Some(error_code::URL_BLOCKED_IP));
}

#[test]
fn test_validate_url_blocks_ipv6_loopback() {
    let input = WebFetchInput {
        url: "http://[::1]/admin".to_string(),
        format: WebFetchFormat::Text,
        timeout: None,
    };
    let result = validate_url(&input);
    assert!(!result.result);
    assert_eq!(result.error_code, Some(error_code::URL_BLOCKED_IP));
}

#[test]
fn test_validate_url_blocks_rfc1918() {
    let input = WebFetchInput {
        url: "http://192.168.1.1/secret".to_string(),
        format: WebFetchFormat::Text,
        timeout: None,
    };
    let result = validate_url(&input);
    assert!(!result.result);
    assert_eq!(result.error_code, Some(error_code::URL_BLOCKED_IP));
}

#[test]
fn test_validate_url_allows_public_urls() {
    let input = WebFetchInput {
        url: "https://example.com/page".to_string(),
        format: WebFetchFormat::Text,
        timeout: None,
    };
    let result = validate_url(&input);
    assert!(result.result);
}

#[test]
fn test_validate_url_allows_public_ip() {
    let input = WebFetchInput {
        url: "https://8.8.8.8".to_string(),
        format: WebFetchFormat::Text,
        timeout: None,
    };
    let result = validate_url(&input);
    assert!(result.result);
}

#[test]
fn test_validate_hostname_passes_validation() {
    let input = WebFetchInput {
        url: "https://localhost/admin".to_string(),
        format: WebFetchFormat::Text,
        timeout: None,
    };
    assert!(validate_url(&input).result);
}
