//! Network policy types.
//!
//! This module provides types for configuring network access
//! restrictions for sandboxed processes.

use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

/// Network action type.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NetworkAction {
    /// Allow the network operation.
    Allow,
    /// Deny the network operation.
    Deny,
}

/// Network policy enforcement mode.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NetworkPolicyMode {
    /// Monitor mode: log violations but allow.
    Monitor,
    /// Enforce mode: block violations.
    Enforce,
}

/// Port range: single port or range (start, end inclusive).
pub type PortRange = (u16, u16);

/// A single network access rule.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkRule {
    /// Action to take when rule matches.
    pub action: NetworkAction,
    /// Traffic direction: "inbound" or "outbound", None = both.
    #[serde(default)]
    pub direction: Option<String>,
    /// Protocol: "tcp" or "udp", None = both.
    #[serde(default)]
    pub protocol: Option<String>,
    /// Domain names to match.
    #[serde(default)]
    pub hosts: Vec<String>,
    /// CIDR notation networks (e.g., "192.168.1.0/24").
    #[serde(default)]
    pub cidrs: Vec<String>,
    /// Port ranges (start, end inclusive).
    #[serde(default)]
    pub ports: Vec<PortRange>,
}

impl NetworkRule {
    /// Validate the network rule.
    pub fn validate(&self) -> Result<(), String> {
        // Validate ports are in valid range
        for &(start, end) in &self.ports {
            if start == 0 || end == 0 {
                return Err("Port 0 is invalid".to_string());
            }
            if start > end {
                return Err(format!("Port range start {} > end {}", start, end));
            }
        }

        // Validate CIDR format (basic check)
        for cidr in &self.cidrs {
            if !cidr.contains('/') {
                return Err(format!("Invalid CIDR format: {}", cidr));
            }
            // Parse prefix length
            let parts: Vec<&str> = cidr.split('/').collect();
            if parts.len() != 2 {
                return Err(format!("Invalid CIDR format: {}", cidr));
            }
            let prefix: u8 = parts[1]
                .parse()
                .map_err(|_| format!("Invalid prefix length in CIDR: {}", cidr))?;
            if prefix > 32 {
                return Err(format!("CIDR prefix {} > 32", prefix));
            }
        }

        // Validate direction if present
        if let Some(ref dir) = self.direction {
            if dir != "inbound" && dir != "outbound" {
                return Err(format!("Invalid direction: {}", dir));
            }
        }

        // Validate protocol if present
        if let Some(ref proto) = self.protocol {
            if proto != "tcp" && proto != "udp" {
                return Err(format!("Invalid protocol: {}", proto));
            }
        }

        // Allow empty rules as "match all" - this is valid for default deny/allow

        Ok(())
    }
}

/// Network policy configuration.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkPolicy {
    /// Whether network policy is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Enforcement mode (None defaults to Monitor).
    #[serde(default)]
    pub mode: Option<NetworkPolicyMode>,
    /// Default action when no rules match (None defaults to Deny).
    #[serde(default)]
    pub default_action: Option<NetworkAction>,
    /// List of network rules.
    #[serde(default)]
    pub rules: Vec<NetworkRule>,
}

impl Default for NetworkPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: Some(NetworkPolicyMode::Monitor),
            default_action: Some(NetworkAction::Deny),
            rules: Vec::new(),
        }
    }
}

impl NetworkPolicy {
    /// Validate all rules in the policy.
    pub fn validate(&self) -> Result<(), super::PolicyError> {
        for (i, rule) in self.rules.iter().enumerate() {
            rule.validate()
                .map_err(|e| super::PolicyError::ValidationError(format!("Rule {}: {}", i, e)))?;
        }
        Ok(())
    }

    pub fn validate_network_access_compatibility(
        &self,
        network_access_allowed: bool,
    ) -> Result<(), super::PolicyError> {
        if self.is_enabled() && !network_access_allowed {
            return Err(super::PolicyError::ValidationError(
                "network_policy cannot be configured when namespaces.network = false blocks all network access"
                    .to_string(),
            ));
        }

        Ok(())
    }

    /// Check if network policy is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Get the enforcement mode (defaults to Monitor).
    pub fn mode(&self) -> NetworkPolicyMode {
        self.mode.clone().unwrap_or(NetworkPolicyMode::Monitor)
    }

    /// Get the default action (defaults to Deny).
    pub fn default_action(&self) -> NetworkAction {
        self.default_action.clone().unwrap_or(NetworkAction::Deny)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_network_policy_default() {
        let net_policy = NetworkPolicy::default();
        assert!(net_policy.enabled);
        assert_eq!(net_policy.mode, Some(NetworkPolicyMode::Monitor));
        assert_eq!(net_policy.default_action, Some(NetworkAction::Deny));
        assert!(net_policy.rules.is_empty());
    }

    #[test]
    fn test_network_rule_validate_valid_ports() {
        let rule = NetworkRule {
            action: NetworkAction::Allow,
            direction: Some("outbound".to_string()),
            protocol: Some("tcp".to_string()),
            hosts: vec!["example.com".to_string()],
            cidrs: vec![],
            ports: vec![(80, 80), (443, 443)],
        };
        assert!(rule.validate().is_ok());
    }

    #[test]
    fn test_network_rule_validate_invalid_port_zero() {
        let rule = NetworkRule {
            action: NetworkAction::Allow,
            direction: None,
            protocol: None,
            hosts: vec![],
            cidrs: vec![],
            ports: vec![(0, 80)],
        };
        let result = rule.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Port 0 is invalid"));
    }

    #[test]
    fn test_network_rule_validate_valid_max_port() {
        let rule = NetworkRule {
            action: NetworkAction::Allow,
            direction: None,
            protocol: None,
            hosts: vec![],
            cidrs: vec![],
            ports: vec![(65535, 65535)],
        };
        assert!(rule.validate().is_ok());
    }

    #[test]
    fn test_network_rule_validate_invalid_port_range() {
        let rule = NetworkRule {
            action: NetworkAction::Allow,
            direction: None,
            protocol: None,
            hosts: vec![],
            cidrs: vec![],
            ports: vec![(443, 80)],
        };
        let result = rule.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("start 443 > end 80"));
    }

    #[test]
    fn test_network_rule_validate_valid_cidr() {
        let rule = NetworkRule {
            action: NetworkAction::Allow,
            direction: None,
            protocol: None,
            hosts: vec![],
            cidrs: vec!["192.168.1.0/24".to_string(), "10.0.0.0/8".to_string()],
            ports: vec![],
        };
        assert!(rule.validate().is_ok());
    }

    #[test]
    fn test_network_rule_validate_invalid_cidr_no_slash() {
        let rule = NetworkRule {
            action: NetworkAction::Allow,
            direction: None,
            protocol: None,
            hosts: vec![],
            cidrs: vec!["192.168.1.0".to_string()],
            ports: vec![],
        };
        let result = rule.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid CIDR format"));
    }

    #[test]
    fn test_network_rule_validate_invalid_cidr_prefix_too_large() {
        let rule = NetworkRule {
            action: NetworkAction::Allow,
            direction: None,
            protocol: None,
            hosts: vec![],
            cidrs: vec!["192.168.1.0/33".to_string()],
            ports: vec![],
        };
        let result = rule.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("prefix 33 > 32"));
    }

    #[test]
    fn test_network_rule_validate_invalid_cidr_non_numeric_prefix() {
        let rule = NetworkRule {
            action: NetworkAction::Allow,
            direction: None,
            protocol: None,
            hosts: vec![],
            cidrs: vec!["192.168.1.0/abc".to_string()],
            ports: vec![],
        };
        let result = rule.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid prefix length"));
    }

    #[test]
    fn test_network_rule_validate_invalid_direction() {
        let rule = NetworkRule {
            action: NetworkAction::Allow,
            direction: Some("sideways".to_string()),
            protocol: None,
            hosts: vec![],
            cidrs: vec![],
            ports: vec![],
        };
        let result = rule.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid direction"));
    }

    #[test]
    fn test_network_rule_validate_valid_direction() {
        let rule_inbound = NetworkRule {
            action: NetworkAction::Allow,
            direction: Some("inbound".to_string()),
            protocol: None,
            hosts: vec![],
            cidrs: vec![],
            ports: vec![],
        };
        assert!(rule_inbound.validate().is_ok());

        let rule_outbound = NetworkRule {
            action: NetworkAction::Allow,
            direction: Some("outbound".to_string()),
            protocol: None,
            hosts: vec![],
            cidrs: vec![],
            ports: vec![],
        };
        assert!(rule_outbound.validate().is_ok());
    }

    #[test]
    fn test_network_rule_validate_invalid_protocol() {
        let rule = NetworkRule {
            action: NetworkAction::Allow,
            direction: None,
            protocol: Some("icmp".to_string()),
            hosts: vec![],
            cidrs: vec![],
            ports: vec![],
        };
        let result = rule.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid protocol"));
    }

    #[test]
    fn test_network_rule_validate_valid_protocol() {
        let rule_tcp = NetworkRule {
            action: NetworkAction::Allow,
            direction: None,
            protocol: Some("tcp".to_string()),
            hosts: vec![],
            cidrs: vec![],
            ports: vec![],
        };
        assert!(rule_tcp.validate().is_ok());

        let rule_udp = NetworkRule {
            action: NetworkAction::Allow,
            direction: None,
            protocol: Some("udp".to_string()),
            hosts: vec![],
            cidrs: vec![],
            ports: vec![],
        };
        assert!(rule_udp.validate().is_ok());
    }

    #[test]
    fn test_network_policy_validate_multiple_rules_error_on_second() {
        let policy = NetworkPolicy {
            enabled: true,
            mode: Some(NetworkPolicyMode::Monitor),
            default_action: Some(NetworkAction::Deny),
            rules: vec![
                NetworkRule {
                    action: NetworkAction::Allow,
                    direction: Some("outbound".to_string()),
                    protocol: Some("tcp".to_string()),
                    hosts: vec!["example.com".to_string()],
                    cidrs: vec!["192.168.1.0/24".to_string()],
                    ports: vec![(80, 80)],
                },
                NetworkRule {
                    action: NetworkAction::Deny,
                    direction: None,
                    protocol: None,
                    hosts: vec![],
                    cidrs: vec![],
                    ports: vec![(0, 100)],
                },
            ],
        };
        let result = policy.validate();
        assert!(result.is_err());
        let err_msg = result.as_ref().unwrap_err().to_string();
        assert!(err_msg.contains("Rule 1:"));
    }

    #[test]
    fn test_network_policy_validate_invalid_rule() {
        let policy = NetworkPolicy {
            enabled: true,
            mode: Some(NetworkPolicyMode::Monitor),
            default_action: Some(NetworkAction::Deny),
            rules: vec![NetworkRule {
                action: NetworkAction::Allow,
                direction: None,
                protocol: None,
                hosts: vec![],
                cidrs: vec!["192.168.1.0/33".to_string()],
                ports: vec![],
            }],
        };
        let result = policy.validate();
        assert!(result.is_err());
        let err_msg = result.as_ref().unwrap_err().to_string();
        assert!(err_msg.contains("Rule 0:"));
        assert!(err_msg.contains("prefix 33 > 32"));
    }

    #[test]
    fn test_network_policy_is_enabled() {
        let policy_enabled = NetworkPolicy {
            enabled: true,
            mode: None,
            default_action: None,
            rules: vec![],
        };
        assert!(policy_enabled.is_enabled());

        let policy_disabled = NetworkPolicy {
            enabled: false,
            mode: None,
            default_action: None,
            rules: vec![],
        };
        assert!(!policy_disabled.is_enabled());
    }

    #[test]
    fn test_network_policy_mode_defaults() {
        let policy_with_mode = NetworkPolicy {
            enabled: true,
            mode: Some(NetworkPolicyMode::Enforce),
            default_action: None,
            rules: vec![],
        };
        assert_eq!(policy_with_mode.mode(), NetworkPolicyMode::Enforce);

        let policy_without_mode = NetworkPolicy {
            enabled: true,
            mode: None,
            default_action: None,
            rules: vec![],
        };
        assert_eq!(policy_without_mode.mode(), NetworkPolicyMode::Monitor);
    }

    #[test]
    fn test_network_policy_default_action_defaults() {
        let policy_with_action = NetworkPolicy {
            enabled: true,
            mode: None,
            default_action: Some(NetworkAction::Allow),
            rules: vec![],
        };
        assert_eq!(policy_with_action.default_action(), NetworkAction::Allow);

        let policy_without_action = NetworkPolicy {
            enabled: true,
            mode: None,
            default_action: None,
            rules: vec![],
        };
        assert_eq!(policy_without_action.default_action(), NetworkAction::Deny);
    }

    #[test]
    fn test_network_policy_rejects_blocked_network_configuration() {
        let policy = NetworkPolicy::default();
        let result = policy.validate_network_access_compatibility(false);

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("network_policy cannot be configured when namespaces.network = false"));
    }

    #[test]
    fn test_disabled_network_policy_allows_blocked_network_configuration() {
        let policy = NetworkPolicy {
            enabled: false,
            ..NetworkPolicy::default()
        };

        let result = policy.validate_network_access_compatibility(false);

        assert!(result.is_ok());
    }
}
