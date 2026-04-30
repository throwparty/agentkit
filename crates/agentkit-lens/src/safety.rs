use std::net::IpAddr;

/// Errors that can occur during URL validation.
#[derive(Debug, thiserror::Error)]
pub enum SafetyError {
    #[error("Invalid URL format: {0}")]
    InvalidUrl(String),

    #[error("Bare IP blocked: {ip} ({reason})")]
    LanIpBlocked { ip: String, reason: String },
}

/// Validate a URL for safe fetching.
///
/// This function checks:
/// 1. The URL format is parseable
/// 2. The scheme is `http` or `https` only
/// 3. A host is present
/// 4. If the host is a **bare IP address** (not a domain name), it is checked
///    against blocked ranges.  Domain names are trusted — DNS resolution and
///    LAN blocking happen at connection time via a custom `reqwest` connector
///    (see `make_client()` in `mcp.rs`).
pub fn validate_url(url: &str) -> Result<String, SafetyError> {
    let parsed = url
        .parse::<reqwest::Url>()
        .map_err(|e| SafetyError::InvalidUrl(format!("Failed to parse URL: {e}")))?;

    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(SafetyError::InvalidUrl(format!(
            "Scheme '{scheme}' is not allowed (only http/https)"
        )));
    }

    let host = parsed
        .host_str()
        .ok_or_else(|| SafetyError::InvalidUrl("URL has no host".to_string()))?;

    if host.is_empty() {
        return Err(SafetyError::InvalidUrl("URL has an empty host".to_string()));
    }

    // If the host is already a bare IP (IPv4 or IPv6), validate it immediately.
    // Domain names are deferred to the HTTP client's connection-time connector.
    // Strip brackets for IPv6 literals (e.g. `[::1]` → `::1`).
    let host_clean = host.strip_prefix('[').unwrap_or(host).strip_suffix(']').unwrap_or(host);
    if let Ok(ip) = host_clean.parse::<IpAddr>()
        && let Some(reason) = is_blocked_ip(ip)
    {
        return Err(SafetyError::LanIpBlocked {
            ip: ip.to_string(),
            reason,
        });
    }

    Ok(url.to_string())
}

fn is_blocked_ip(ip: IpAddr) -> Option<String> {
    match ip {
        IpAddr::V4(v4) => is_blocked_ipv4(v4),
        IpAddr::V6(v6) => is_blocked_ipv6(v6),
    }
}

fn is_blocked_ipv4(ip: std::net::Ipv4Addr) -> Option<String> {
    let octets = ip.octets();
    let first = octets[0];
    let second = octets[1];

    if first == 127 {
        return Some("loopback address (localhost)".to_string());
    }
    if first == 10 {
        return Some("private network (RFC 1918 Class A)".to_string());
    }
    if first == 172 && (16..=31).contains(&second) {
        return Some("private network (RFC 1918 Class B)".to_string());
    }
    if first == 192 && second == 168 {
        return Some("private network (RFC 1918 Class C)".to_string());
    }
    if first == 169 && second == 254 {
        return Some("link-local address".to_string());
    }
    if first == 0 {
        return Some("unspecified address".to_string());
    }
    None
}

fn is_blocked_ipv6(ip: std::net::Ipv6Addr) -> Option<String> {
    if ip.is_loopback() {
        return Some("loopback address (::1)".to_string());
    }

    let bytes = ip.octets();
    let first_byte = bytes[0];
    if (first_byte & 0xFE) == 0xFC {
        return Some("unique local address (IPv6 private)".to_string());
    }
    if ip.is_unicast_link_local() {
        return Some("link-local address (IPv6)".to_string());
    }
    if first_byte == 0xFF {
        return Some("multicast address (not routable)".to_string());
    }
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return is_blocked_ipv4(mapped);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invalid_scheme() {
        let result = validate_url("ftp://example.com/file");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), SafetyError::InvalidUrl(_)));
    }

    #[test]
    fn test_invalid_url_format() {
        let result = validate_url("not a url");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), SafetyError::InvalidUrl(_)));
    }

    // -------------------------------------------------------------------------
    // Bare IP validation (no DNS)
    // -------------------------------------------------------------------------

    #[test]
    fn test_bare_ipv4_blocked_class_a_10() {
        assert!(matches!(
            validate_url("http://10.0.0.1/page").unwrap_err(),
            SafetyError::LanIpBlocked { .. }
        ));
    }

    #[test]
    fn test_bare_ipv4_blocked_class_b_172() {
        assert!(matches!(
            validate_url("http://172.16.0.1/page").unwrap_err(),
            SafetyError::LanIpBlocked { .. }
        ));
    }

    #[test]
    fn test_bare_ipv4_blocked_class_c_192() {
        assert!(matches!(
            validate_url("http://192.168.1.1/page").unwrap_err(),
            SafetyError::LanIpBlocked { .. }
        ));
    }

    #[test]
    fn test_bare_ipv4_blocked_loopback() {
        assert!(matches!(
            validate_url("http://127.0.0.1/page").unwrap_err(),
            SafetyError::LanIpBlocked { .. }
        ));
    }

    #[test]
    fn test_bare_ipv4_allowed_public() {
        assert!(validate_url("http://8.8.8.8/page").is_ok());
        assert!(validate_url("https://1.1.1.1/page").is_ok());
    }

    #[test]
    fn test_bare_ipv6_blocked_loopback() {
        assert!(matches!(
            validate_url("http://[::1]/page").unwrap_err(),
            SafetyError::LanIpBlocked { .. }
        ));
    }

    #[test]
    fn test_bare_ipv6_blocked_private() {
        assert!(matches!(
            validate_url("http://[fd00::1]/page").unwrap_err(),
            SafetyError::LanIpBlocked { .. }
        ));
    }

    // -------------------------------------------------------------------------
    // Domain names pass through (no DNS)
    // -------------------------------------------------------------------------

    #[test]
    fn test_domain_passes_through() {
        assert!(validate_url("https://example.com/page").is_ok());
        assert!(validate_url("https://rust-lang.org").is_ok());
        assert!(validate_url("http://example.com/page").is_ok());
    }

    // -------------------------------------------------------------------------
    // Internal helper tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_ipv4_blocked_class_a_10() {
        assert!(is_blocked_ipv4(std::net::Ipv4Addr::new(10, 0, 0, 1)).is_some());
        assert!(is_blocked_ipv4(std::net::Ipv4Addr::new(10, 255, 255, 255)).is_some());
    }

    #[test]
    fn test_ipv4_blocked_class_b_172() {
        assert!(is_blocked_ipv4(std::net::Ipv4Addr::new(172, 16, 0, 1)).is_some());
        assert!(is_blocked_ipv4(std::net::Ipv4Addr::new(172, 31, 255, 255)).is_some());
    }

    #[test]
    fn test_ipv4_not_blocked_172_32() {
        assert!(is_blocked_ipv4(std::net::Ipv4Addr::new(172, 32, 0, 1)).is_none());
    }

    #[test]
    fn test_ipv4_not_blocked_172_15() {
        assert!(is_blocked_ipv4(std::net::Ipv4Addr::new(172, 15, 0, 1)).is_none());
    }

    #[test]
    fn test_ipv4_blocked_class_c_192() {
        assert!(is_blocked_ipv4(std::net::Ipv4Addr::new(192, 168, 0, 1)).is_some());
        assert!(is_blocked_ipv4(std::net::Ipv4Addr::new(192, 168, 255, 255)).is_some());
    }

    #[test]
    fn test_ipv4_blocked_loopback_127() {
        assert!(is_blocked_ipv4(std::net::Ipv4Addr::new(127, 0, 0, 1)).is_some());
        assert!(is_blocked_ipv4(std::net::Ipv4Addr::new(127, 255, 255, 255)).is_some());
    }

    #[test]
    fn test_ipv4_blocked_link_local_169() {
        assert!(is_blocked_ipv4(std::net::Ipv4Addr::new(169, 254, 0, 1)).is_some());
        assert!(is_blocked_ipv4(std::net::Ipv4Addr::new(169, 254, 169, 254)).is_some());
    }

    #[test]
    fn test_ipv4_blocked_zero() {
        assert!(is_blocked_ipv4(std::net::Ipv4Addr::new(0, 0, 0, 0)).is_some());
    }

    #[test]
    fn test_ipv4_allowed_public() {
        assert!(is_blocked_ipv4(std::net::Ipv4Addr::new(8, 8, 8, 8)).is_none());
        assert!(is_blocked_ipv4(std::net::Ipv4Addr::new(1, 1, 1, 1)).is_none());
        assert!(is_blocked_ipv4(std::net::Ipv4Addr::new(203, 0, 113, 1)).is_none());
    }

    #[test]
    fn test_ipv6_loopback() {
        assert!(is_blocked_ipv6(std::net::Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1)).is_some());
    }

    #[test]
    fn test_ipv6_private_fc() {
        assert!(is_blocked_ipv6(std::net::Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 1)).is_some());
    }

    #[test]
    fn test_ipv6_private_fd() {
        assert!(is_blocked_ipv6(std::net::Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1)).is_some());
    }

    #[test]
    fn test_ipv6_link_local_fe80() {
        assert!(is_blocked_ipv6(std::net::Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1)).is_some());
    }

    #[test]
    fn test_ipv6_multicast_ff() {
        assert!(is_blocked_ipv6(std::net::Ipv6Addr::new(0xff00, 0, 0, 0, 0, 0, 0, 1)).is_some());
    }

    #[test]
    fn test_ipv4_mapped_loopback() {
        let mapped: std::net::Ipv6Addr = std::net::Ipv4Addr::new(127, 0, 0, 1).to_ipv6_mapped();
        assert!(is_blocked_ipv6(mapped).is_some());
    }

    #[test]
    fn test_ipv4_mapped_blocked() {
        let mapped: std::net::Ipv6Addr = std::net::Ipv4Addr::new(192, 168, 1, 1).to_ipv6_mapped();
        assert!(is_blocked_ipv6(mapped).is_some());
    }

    #[test]
    fn test_ipv6_allowed() {
        assert!(is_blocked_ipv6(std::net::Ipv6Addr::new(0x2606, 0x4700, 0, 0, 0, 0, 0, 1)).is_none());
    }
}
