use crate::network::cidr::CidrError;
use crate::network::resolver::ResolveError;
use crate::network::{Cidr, DnsResolver};
use crate::policy::NetworkPolicy;

#[cfg(feature = "ebpf")]
use crate::policy::{NetworkAction, NetworkRule};

#[cfg(feature = "ebpf")]
use crate::audit::{NetworkAccessEvent, NetworkDirection, NetworkProtocol};

use std::collections::HashMap;
use std::net::Ipv4Addr;

pub struct NetworkPolicyMatcher {
    policy: NetworkPolicy,
    resolver: DnsResolver,
    resolved_hosts: HashMap<String, Vec<Ipv4Addr>>,
    parsed_cidrs: Vec<(usize, Cidr)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchResult {
    Allowed,
    Denied,
    NoMatch,
}

impl NetworkPolicyMatcher {
    pub fn new(policy: NetworkPolicy) -> Self {
        Self {
            policy,
            resolver: DnsResolver::new(),
            resolved_hosts: HashMap::new(),
            parsed_cidrs: Vec::new(),
        }
    }

    pub fn initialize(&mut self) -> Result<(), MatcherError> {
        self.resolved_hosts.clear();
        self.parsed_cidrs.clear();

        for rule in &self.policy.rules {
            for host in &rule.hosts {
                let ips = self.resolver.resolve(host)?;
                self.resolved_hosts.insert(host.clone(), ips);
            }
        }

        for (i, rule) in self.policy.rules.iter().enumerate() {
            for cidr_str in &rule.cidrs {
                let cidr = cidr_str.parse::<Cidr>()?;
                self.parsed_cidrs.push((i, cidr));
            }
        }

        Ok(())
    }

    #[cfg(feature = "ebpf")]
    pub fn evaluate(&self, event: &NetworkAccessEvent) -> MatchResult {
        for (i, rule) in self.policy.rules.iter().enumerate() {
            if self.matches_rule(i, rule, event) {
                return match rule.action {
                    NetworkAction::Allow => MatchResult::Allowed,
                    NetworkAction::Deny => MatchResult::Denied,
                };
            }
        }

        MatchResult::NoMatch
    }

    #[cfg(feature = "ebpf")]
    fn matches_rule(
        &self,
        rule_index: usize,
        rule: &NetworkRule,
        event: &NetworkAccessEvent,
    ) -> bool {
        self.matches_direction(rule, event)
            && self.matches_protocol(rule, event)
            && self.matches_address(rule_index, rule, event)
            && self.matches_port(rule, event)
    }

    #[cfg(feature = "ebpf")]
    fn matches_direction(&self, rule: &NetworkRule, event: &NetworkAccessEvent) -> bool {
        match rule.direction.as_deref() {
            None => true,
            Some("outbound") => matches!(event.direction, NetworkDirection::Outbound),
            Some("inbound") => matches!(event.direction, NetworkDirection::Inbound),
            Some(_) => false,
        }
    }

    #[cfg(feature = "ebpf")]
    fn matches_protocol(&self, rule: &NetworkRule, event: &NetworkAccessEvent) -> bool {
        match rule.protocol.as_deref() {
            None => true,
            Some("tcp") => matches!(event.protocol, NetworkProtocol::Tcp),
            Some("udp") => matches!(event.protocol, NetworkProtocol::Udp),
            Some(_) => false,
        }
    }

    #[cfg(feature = "ebpf")]
    fn matches_address(
        &self,
        rule_index: usize,
        rule: &NetworkRule,
        event: &NetworkAccessEvent,
    ) -> bool {
        if rule.hosts.is_empty() && rule.cidrs.is_empty() {
            return true;
        }

        for host in &rule.hosts {
            if let Some(ips) = self.resolved_hosts.get(host) {
                if ips.contains(&event.address) {
                    return true;
                }
            }
        }

        for (idx, cidr) in &self.parsed_cidrs {
            if *idx == rule_index && cidr.contains(event.address) {
                return true;
            }
        }

        false
    }

    #[cfg(feature = "ebpf")]
    fn matches_port(&self, rule: &NetworkRule, event: &NetworkAccessEvent) -> bool {
        if rule.ports.is_empty() {
            return true;
        }

        for &(start, end) in &rule.ports {
            if event.port >= start && event.port <= end {
                return true;
            }
        }

        false
    }

    pub fn policy(&self) -> &NetworkPolicy {
        &self.policy
    }
}

impl Default for NetworkPolicyMatcher {
    fn default() -> Self {
        Self::new(NetworkPolicy::default())
    }
}

#[derive(Debug)]
pub enum MatcherError {
    ResolveError(String),
    CidrError(String),
}

impl std::fmt::Display for MatcherError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MatcherError::ResolveError(msg) => write!(f, "DNS resolution failed: {}", msg),
            MatcherError::CidrError(msg) => write!(f, "CIDR parse failed: {}", msg),
        }
    }
}

impl std::error::Error for MatcherError {}

impl From<ResolveError> for MatcherError {
    fn from(error: ResolveError) -> Self {
        MatcherError::ResolveError(error.to_string())
    }
}

impl From<CidrError> for MatcherError {
    fn from(error: CidrError) -> Self {
        MatcherError::CidrError(error.to_string())
    }
}

#[cfg(all(test, feature = "ebpf"))]
mod tests {
    use super::*;
    use crate::audit::NetworkAccessResult;
    use std::time::SystemTime;

    fn test_event(
        direction: NetworkDirection,
        protocol: NetworkProtocol,
        address: &str,
        port: u16,
    ) -> NetworkAccessEvent {
        NetworkAccessEvent {
            direction,
            protocol,
            address: address.parse().unwrap(),
            port,
            result: NetworkAccessResult::Allowed,
            pid: 42,
            timestamp: SystemTime::now(),
        }
    }

    fn base_policy(rule: NetworkRule) -> NetworkPolicy {
        NetworkPolicy {
            enabled: true,
            mode: Some(crate::policy::NetworkPolicyMode::Monitor),
            default_action: Some(NetworkAction::Deny),
            rules: vec![rule],
        }
    }

    #[test]
    fn test_direction_matching() {
        let rule = NetworkRule {
            action: NetworkAction::Allow,
            direction: Some("outbound".to_string()),
            protocol: None,
            hosts: vec![],
            cidrs: vec![],
            ports: vec![],
        };
        let policy = base_policy(rule);
        let matcher = NetworkPolicyMatcher::new(policy);

        let outbound = test_event(
            NetworkDirection::Outbound,
            NetworkProtocol::Tcp,
            "8.8.8.8",
            53,
        );
        let inbound = test_event(
            NetworkDirection::Inbound,
            NetworkProtocol::Tcp,
            "8.8.8.8",
            53,
        );

        assert_eq!(matcher.evaluate(&outbound), MatchResult::Allowed);
        assert_eq!(matcher.evaluate(&inbound), MatchResult::NoMatch);
    }

    #[test]
    fn test_port_matching() {
        let rule = NetworkRule {
            action: NetworkAction::Allow,
            direction: None,
            protocol: None,
            hosts: vec![],
            cidrs: vec![],
            ports: vec![(80, 80), (8000, 9000)],
        };
        let policy = base_policy(rule);
        let matcher = NetworkPolicyMatcher::new(policy);

        let single_port = test_event(
            NetworkDirection::Outbound,
            NetworkProtocol::Tcp,
            "8.8.8.8",
            80,
        );
        let range_port = test_event(
            NetworkDirection::Outbound,
            NetworkProtocol::Tcp,
            "8.8.8.8",
            8080,
        );
        let no_match = test_event(
            NetworkDirection::Outbound,
            NetworkProtocol::Tcp,
            "8.8.8.8",
            22,
        );

        assert_eq!(matcher.evaluate(&single_port), MatchResult::Allowed);
        assert_eq!(matcher.evaluate(&range_port), MatchResult::Allowed);
        assert_eq!(matcher.evaluate(&no_match), MatchResult::NoMatch);
    }
}
