use std::fmt;
use std::net::Ipv4Addr;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CidrError {
    InvalidFormat(String),
    InvalidPrefixLength(u8),
    InvalidAddress(String),
}

impl fmt::Display for CidrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CidrError::InvalidFormat(msg) => write!(f, "Invalid CIDR format: {}", msg),
            CidrError::InvalidPrefixLength(len) => {
                write!(f, "Invalid prefix length: {} (must be 0-32)", len)
            }
            CidrError::InvalidAddress(msg) => write!(f, "Invalid address: {}", msg),
        }
    }
}

impl std::error::Error for CidrError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cidr {
    network: Ipv4Addr,
    prefix_len: u8,
    mask: u32,
}

impl Cidr {
    pub fn new(network: Ipv4Addr, prefix_len: u8) -> Result<Self, CidrError> {
        if prefix_len > 32 {
            return Err(CidrError::InvalidPrefixLength(prefix_len));
        }

        let mask = if prefix_len == 0 {
            0u32
        } else if prefix_len == 32 {
            !0u32
        } else {
            !0u32 << (32 - prefix_len)
        };

        Ok(Cidr {
            network,
            prefix_len,
            mask,
        })
    }

    pub fn contains(&self, ip: Ipv4Addr) -> bool {
        (u32::from(ip) & self.mask) == (u32::from(self.network) & self.mask)
    }

    pub fn network(&self) -> Ipv4Addr {
        self.network
    }

    pub fn prefix_len(&self) -> u8 {
        self.prefix_len
    }

    pub fn parse(cidr_str: &str) -> Result<Self, Box<dyn std::error::Error>> {
        Cidr::from_str(cidr_str).map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
    }

    pub fn matches(&self, ip: &str) -> bool {
        match Ipv4Addr::from_str(ip) {
            Ok(addr) => self.contains(addr),
            Err(_) => false,
        }
    }
}

impl FromStr for Cidr {
    type Err = CidrError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split('/').collect();

        if parts.len() != 2 {
            return Err(CidrError::InvalidFormat(
                "Expected format: 'ip/prefix'".to_string(),
            ));
        }

        let network = Ipv4Addr::from_str(parts[0])
            .map_err(|_| CidrError::InvalidAddress(parts[0].to_string()))?;

        let prefix_len: u8 = parts[1].parse().map_err(|_| {
            CidrError::InvalidFormat(format!("Invalid prefix length: {}", parts[1]))
        })?;

        Cidr::new(network, prefix_len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_cidr() {
        let cidr = "192.168.1.0/24".parse::<Cidr>().unwrap();
        assert_eq!(cidr.network(), Ipv4Addr::new(192, 168, 1, 0));
        assert_eq!(cidr.prefix_len(), 24);
    }

    #[test]
    fn test_contains_ip_in_range() {
        let cidr = "192.168.1.0/24".parse::<Cidr>().unwrap();
        assert!(cidr.contains(Ipv4Addr::new(192, 168, 1, 1)));
        assert!(cidr.contains(Ipv4Addr::new(192, 168, 1, 100)));
        assert!(cidr.contains(Ipv4Addr::new(192, 168, 1, 255)));
        assert!(!cidr.contains(Ipv4Addr::new(192, 168, 0, 255)));
        assert!(!cidr.contains(Ipv4Addr::new(192, 168, 2, 0)));
    }

    #[test]
    fn test_parse_invalid_prefix() {
        let result = "192.168.1.0/33".parse::<Cidr>();
        assert!(result.is_err());
    }
}
