use std::collections::HashMap;
use std::net::{Ipv4Addr, ToSocketAddrs};
use std::time::{Duration, Instant};

pub struct DnsResolver {
    cache: HashMap<String, CacheEntry>,
    default_ttl: Duration,
}

struct CacheEntry {
    ips: Vec<Ipv4Addr>,
    resolved_at: Instant,
    ttl: Duration,
}

#[derive(Debug)]
pub enum ResolveError {
    DnsLookupFailed(String),
    NoAddressesFound(String),
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::DnsLookupFailed(msg) => write!(f, "DNS lookup failed: {}", msg),
            ResolveError::NoAddressesFound(hostname) => {
                write!(f, "No addresses found for hostname: {}", hostname)
            }
        }
    }
}

impl std::error::Error for ResolveError {}

impl DnsResolver {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            default_ttl: Duration::from_secs(300),
        }
    }

    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            cache: HashMap::new(),
            default_ttl: ttl,
        }
    }

    pub fn resolve(&mut self, hostname: &str) -> Result<Vec<Ipv4Addr>, ResolveError> {
        let hostname_lower = hostname.to_lowercase();

        if let Some(entry) = self.cache.get(&hostname_lower) {
            if !self.is_expired(entry) {
                return Ok(entry.ips.clone());
            }
        }

        let socket_addrs = (hostname, 0)
            .to_socket_addrs()
            .map_err(|e| ResolveError::DnsLookupFailed(e.to_string()))?;

        let ipv4_addrs: Vec<Ipv4Addr> = socket_addrs
            .filter_map(|addr| {
                if let std::net::SocketAddr::V4(v4) = addr {
                    Some(*v4.ip())
                } else {
                    None
                }
            })
            .collect();

        if ipv4_addrs.is_empty() {
            return Err(ResolveError::NoAddressesFound(hostname.to_string()));
        }

        let entry = CacheEntry {
            ips: ipv4_addrs.clone(),
            resolved_at: Instant::now(),
            ttl: self.default_ttl,
        };
        self.cache.insert(hostname_lower, entry);

        Ok(ipv4_addrs)
    }

    fn is_expired(&self, entry: &CacheEntry) -> bool {
        entry.resolved_at.elapsed() > entry.ttl
    }

    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }
}

impl Default for DnsResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_localhost() {
        let mut resolver = DnsResolver::new();
        let ips = resolver.resolve("localhost").unwrap();
        assert!(!ips.is_empty());
        assert!(ips.contains(&Ipv4Addr::new(127, 0, 0, 1)));
    }

    #[test]
    fn test_cache_hit() {
        let mut resolver = DnsResolver::new();
        let ips1 = resolver.resolve("localhost").unwrap();
        let ips2 = resolver.resolve("localhost").unwrap();
        assert_eq!(ips1, ips2);
        assert_eq!(resolver.cache.len(), 1);
    }
}
