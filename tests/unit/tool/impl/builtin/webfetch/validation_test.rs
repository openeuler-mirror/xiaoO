use super::*;
use crate::r#impl::builtin::webfetch::input::WebFetchFormat;

/// SSRF guard: only public addresses may be fetched. Loopback (v4/v6),
/// RFC1918 private ranges, link-local (incl. the cloud metadata endpoint),
/// unspecified, and IPv4-mapped IPv6 addresses must all be blocked.
#[test]
fn is_ip_blocks_non_public_addresses() {
    // (address, expected)
    let cases = [
        // loopback v4
        ("127.0.0.1", true),
        ("127.0.0.2", true),
        ("127.255.255.255", true),
        // loopback v6
        ("::1", true),
        // RFC1918 private (172.32.0.1 lies just outside the 172.16/12 range)
        ("10.0.0.1", true),
        ("10.255.255.255", true),
        ("172.16.0.1", true),
        ("172.31.255.255", true),
        ("172.32.0.1", false),
        ("192.168.0.1", true),
        ("192.168.255.255", true),
        // link-local, incl. the cloud metadata endpoint
        ("169.254.169.254", true),
        ("169.254.1.1", true),
        ("fe80::1", true),
        // unspecified
        ("0.0.0.0", true),
        ("::", true),
        // IPv4-mapped IPv6 must not bypass the v4 checks
        ("::ffff:127.0.0.1", true),
        ("::ffff:10.0.0.1", true),
        // public addresses stay reachable
        ("8.8.8.8", false),
        ("1.1.1.1", false),
        ("2001:4860:4860::8888", false),
    ];
    for (address, blocked) in cases {
        let ip: IpAddr = address.parse().unwrap();
        assert_eq!(is_ip_blocked(ip), blocked, "{address}");
    }
}

/// extract_host: scheme, port, and userinfo are stripped; IPv4 and bracketed
/// IPv6 literals are kept verbatim.
#[test]
fn extract_host_normalizes_urls() {
    // (url, expected host)
    let cases = [
        // scheme and path stripped
        ("https://example.com/path", "example.com"),
        ("http://example.com", "example.com"),
        // port stripped
        ("https://example.com:443/path", "example.com"),
        ("http://example.com:8080", "example.com"),
        // IPv4 literals kept
        ("http://127.0.0.1:8080/path", "127.0.0.1"),
        ("http://192.168.1.1/api", "192.168.1.1"),
        // bracketed IPv6 literals kept
        ("http://[::1]:8080/", "[::1]"),
        ("http://[::ffff:127.0.0.1]/path", "[::ffff:127.0.0.1]"),
        // userinfo stripped
        ("http://user:pass@example.com/path", "example.com"),
    ];
    for (url, expected) in cases {
        assert_eq!(extract_host(url), expected, "{url}");
    }
}

/// URLs pointing at non-public endpoints (loopback, cloud metadata, private
/// ranges) must be rejected with URL_BLOCKED_IP.
#[test]
fn validate_url_blocks_non_public_endpoints() {
    for url in [
        "http://127.0.0.1:8080/admin",
        // cloud metadata endpoint
        "http://169.254.169.254/latest/meta-data/",
        "http://[::1]/admin",
        "http://192.168.1.1/secret",
    ] {
        let input = WebFetchInput {
            url: url.to_string(),
            format: WebFetchFormat::Text,
            timeout: None,
        };
        let result = validate_url(&input);
        assert!(!result.result, "{url}");
        assert_eq!(result.error_code, Some(error_code::URL_BLOCKED_IP), "{url}");
    }
}

/// Public URLs and plain hostnames (resolved later, not at validation) pass.
#[test]
fn validate_url_allows_public_urls_and_hostnames() {
    for url in [
        "https://example.com/page",
        "https://8.8.8.8",
        "https://localhost/admin",
    ] {
        let input = WebFetchInput {
            url: url.to_string(),
            format: WebFetchFormat::Text,
            timeout: None,
        };
        assert!(validate_url(&input).result, "{url}");
    }
}
