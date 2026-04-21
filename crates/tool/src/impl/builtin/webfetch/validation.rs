use std::net::IpAddr;
use std::net::SocketAddr;

use super::constants::max_timeout_ms;
use super::input::WebFetchInput;

pub mod error_code {
    pub const URL_EMPTY: u32 = 1;
    pub const URL_INVALID_SCHEME: u32 = 2;
    pub const TIMEOUT_INVALID: u32 = 3;
    pub const TIMEOUT_EXCEEDS_MAX: u32 = 4;
    pub const URL_BLOCKED_IP: u32 = 5;
}

#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub result: bool,
    pub message: Option<String>,
    pub error_code: Option<u32>,
}

impl ValidationResult {
    pub fn ok() -> Self {
        Self {
            result: true,
            message: None,
            error_code: None,
        }
    }

    pub fn error(message: impl Into<String>, error_code: u32) -> Self {
        Self {
            result: false,
            message: Some(message.into()),
            error_code: Some(error_code),
        }
    }
}

/// Check if an IP address is blocked for SSRF protection.
///
/// Blocks:
/// - Loopback (127.0.0.0/8, ::1)
/// - RFC 1918 private (10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16)
/// - RFC 4193 unique local (fc00::/7)
/// - Link-local (169.254.0.0/16, fe80::/10)
/// - Unspecified (0.0.0.0, ::)
/// - IPv4-mapped IPv6 private addresses (::ffff:0:0/96 that map to private ranges)
/// - AWS/GCP metadata endpoints
fn is_ip_blocked(ip: IpAddr) -> bool {
    if ip.is_loopback() {
        return true;
    }
    match ip {
        IpAddr::V4(v4) => {
            if v4.is_private() {
                return true;
            }
            if v4.is_link_local() {
                return true;
            }
        }
        IpAddr::V6(v6) => {
            // RFC 4193 unique local (fc00::/7)
            if (v6.segments()[0] & 0xfe00) == 0xfc00 {
                return true;
            }
            // IPv6 link-local (fe80::/10)
            if (v6.segments()[0] & 0xffc0) == 0xfe80 {
                return true;
            }
        }
    }

    if ip.is_unspecified() {
        return true;
    }

    // Check IPv4-mapped IPv6 addresses (::ffff:x.x.x.x)
    if let IpAddr::V6(ipv6) = ip {
        if let Some(ipv4) = ipv6.to_ipv4_mapped() {
            return is_ip_blocked(IpAddr::V4(ipv4));
        }
    }

    false
}

/// Parse host from URL and check if it's a literal blocked IP.
/// This catches direct IP URLs like http://127.0.0.1:8080/path
fn validate_url_host_not_blocked_ip(url_str: &str) -> ValidationResult {
    // Extract host from URL manually to avoid adding url crate dependency
    // Format after stripping scheme: [user:pass@]host[:port][/][?][#]
    let host = extract_host(url_str);

    // Try parsing as IP address directly (IPv4 or bracketed IPv6)
    if let Some(ip) = parse_host_as_ip(&host) {
        if is_ip_blocked(ip) {
            return ValidationResult::error(
                format!(
                    "URL resolves to a blocked internal IP address: {} \
                     (loopback/private/link-local addresses are not allowed)",
                    ip
                ),
                error_code::URL_BLOCKED_IP,
            );
        }
    }

    ValidationResult::ok()
}

/// Extract the host portion from a URL string.
/// Handles http(s):// scheme, port, and IPv6 brackets.
fn extract_host(url_str: &str) -> String {
    let after_scheme = url_str
        .strip_prefix("http://")
        .or_else(|| url_str.strip_prefix("https://"))
        .unwrap_or(url_str);

    let host_port = after_scheme
        .find(&['/', '?', '#'][..])
        .map(|i| &after_scheme[..i])
        .unwrap_or(after_scheme);

    // Strip userinfo (user:pass@) — not part of host for IP check
    let host_port = host_port
        .rfind('@')
        .map(|i| &host_port[i + 1..])
        .unwrap_or(host_port);

    let host = if let Some(colon_pos) = host_port.rfind(':') {
        if host_port.starts_with('[') {
            if let Some(bracket_end) = host_port.find(']') {
                &host_port[..=bracket_end]
            } else {
                host_port
            }
        } else {
            &host_port[..colon_pos]
        }
    } else {
        host_port
    };

    host.to_string()
}

/// Try to parse a host string as an IP address.
/// Returns None if it's a hostname (not an IP).
fn parse_host_as_ip(host: &str) -> Option<IpAddr> {
    let trimmed = host.trim_matches(|c| c == '[' || c == ']');
    trimmed.parse::<IpAddr>().ok()
}

/// Validate that all resolved socket addresses for a host are allowed.
/// Used post-DNS resolution in executor to prevent DNS rebinding attacks.
pub fn validate_resolved_addrs(addrs: &[SocketAddr]) -> ValidationResult {
    for addr in addrs {
        if is_ip_blocked(addr.ip()) {
            return ValidationResult::error(
                format!(
                    "URL resolved to a blocked internal IP address: {} \
                     (SSRF protection: loopback/private/link-local addresses are not allowed)",
                    addr.ip()
                ),
                error_code::URL_BLOCKED_IP,
            );
        }
    }

    ValidationResult::ok()
}

fn validate_url(input: &WebFetchInput) -> ValidationResult {
    if input.url.trim().is_empty() {
        return ValidationResult::error("URL cannot be empty", error_code::URL_EMPTY);
    }

    if !input.url.starts_with("http://") && !input.url.starts_with("https://") {
        return ValidationResult::error(
            format!(
                "URL must start with http:// or https://, got: {}",
                input.url
            ),
            error_code::URL_INVALID_SCHEME,
        );
    }

    // SSRF protection: block literal internal IPs in URL host
    let host_check = validate_url_host_not_blocked_ip(&input.url);
    if !host_check.result {
        return host_check;
    }

    ValidationResult::ok()
}

fn validate_timeout(input: &WebFetchInput) -> ValidationResult {
    let Some(timeout) = input.timeout else {
        return ValidationResult::ok();
    };

    if timeout == 0 {
        return ValidationResult::error(
            "Timeout must be greater than 0 milliseconds",
            error_code::TIMEOUT_INVALID,
        );
    }

    let max_timeout = max_timeout_ms();
    if timeout > max_timeout {
        return ValidationResult::error(
            format!(
                "Timeout {}ms exceeds maximum allowed {}ms",
                timeout, max_timeout
            ),
            error_code::TIMEOUT_EXCEEDS_MAX,
        );
    }

    ValidationResult::ok()
}

pub fn validate_input(input: &WebFetchInput) -> ValidationResult {
    let url_result = validate_url(input);
    if !url_result.result {
        return url_result;
    }

    let timeout_result = validate_timeout(input);
    if !timeout_result.result {
        return timeout_result;
    }

    ValidationResult::ok()
}

#[cfg(test)]
mod tests {
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
}
