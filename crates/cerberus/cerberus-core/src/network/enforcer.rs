use crate::policy::NetworkPolicyMode;

#[cfg(feature = "ebpf")]
use crate::audit::{NetworkAccessEvent, NetworkAccessResult};

#[cfg(feature = "ebpf")]
use crate::network::matcher::MatchResult;

#[cfg(feature = "ebpf")]
use crate::network::matcher::NetworkPolicyMatcher;

#[cfg(feature = "ebpf")]
use crate::policy::NetworkAction;

#[cfg(feature = "ebpf")]
use crate::policy::NetworkPolicy;

pub struct NetworkEnforcer {
    #[cfg(feature = "ebpf")]
    matcher: NetworkPolicyMatcher,
    mode: NetworkPolicyMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnforceResult {
    Allowed,
    Monitored,
    Killed,
}

impl NetworkEnforcer {
    #[cfg(feature = "ebpf")]
    pub fn new(matcher: NetworkPolicyMatcher, mode: NetworkPolicyMode) -> Self {
        Self { matcher, mode }
    }

    #[cfg(feature = "ebpf")]
    pub fn process(&self, event: &NetworkAccessEvent, pid: u32) -> EnforceResult {
        let match_result = self.matcher.evaluate(event);
        let default_action = self.matcher.policy().default_action();

        let allowed = match match_result {
            MatchResult::Allowed => true,
            MatchResult::Denied => false,
            MatchResult::NoMatch => match default_action {
                NetworkAction::Allow => true,
                NetworkAction::Deny => false,
            },
        };

        if allowed {
            EnforceResult::Allowed
        } else {
            match self.mode {
                NetworkPolicyMode::Monitor => EnforceResult::Monitored,
                NetworkPolicyMode::Enforce => {
                    self.kill_process(pid);
                    EnforceResult::Killed
                }
            }
        }
    }

    #[cfg(feature = "ebpf")]
    fn kill_process(&self, pid: u32) {
        #[cfg(target_os = "linux")]
        unsafe {
            libc::kill(pid as i32, libc::SIGKILL);
        }
    }

    #[cfg(feature = "ebpf")]
    pub fn to_audit_result(result: &EnforceResult) -> NetworkAccessResult {
        match result {
            EnforceResult::Allowed => NetworkAccessResult::Allowed,
            EnforceResult::Monitored => NetworkAccessResult::Monitored,
            EnforceResult::Killed => NetworkAccessResult::DeniedByPolicy,
        }
    }

    pub fn mode(&self) -> &NetworkPolicyMode {
        &self.mode
    }
}

#[cfg(feature = "ebpf")]
impl Default for NetworkEnforcer {
    fn default() -> Self {
        let policy = NetworkPolicy::default();
        let matcher = NetworkPolicyMatcher::new(policy);
        Self::new(matcher, NetworkPolicyMode::Monitor)
    }
}

#[cfg(all(test, feature = "ebpf"))]
mod tests {
    use super::*;
    use crate::audit::{NetworkDirection, NetworkProtocol};
    use std::time::SystemTime;

    fn create_test_event(address: &str, port: u16) -> NetworkAccessEvent {
        NetworkAccessEvent {
            direction: NetworkDirection::Outbound,
            protocol: NetworkProtocol::Tcp,
            address: address.parse().unwrap(),
            port,
            result: NetworkAccessResult::Allowed,
            pid: 1234,
            timestamp: SystemTime::now(),
        }
    }

    #[test]
    fn test_allowed_connection() {
        let policy = NetworkPolicy {
            enabled: true,
            mode: Some(NetworkPolicyMode::Monitor),
            default_action: Some(NetworkAction::Allow),
            rules: vec![],
        };

        let matcher = NetworkPolicyMatcher::new(policy);
        let enforcer = NetworkEnforcer::new(matcher, NetworkPolicyMode::Monitor);

        let event = create_test_event("8.8.8.8", 443);
        let result = enforcer.process(&event, 1234);

        assert_eq!(result, EnforceResult::Allowed);
    }

    #[test]
    fn test_denied_monitor_mode() {
        let policy = NetworkPolicy {
            enabled: true,
            mode: Some(NetworkPolicyMode::Monitor),
            default_action: Some(NetworkAction::Deny),
            rules: vec![],
        };

        let matcher = NetworkPolicyMatcher::new(policy);
        let enforcer = NetworkEnforcer::new(matcher, NetworkPolicyMode::Monitor);

        let event = create_test_event("8.8.8.8", 443);
        let result = enforcer.process(&event, 1234);

        assert_eq!(result, EnforceResult::Monitored);
    }

    #[test]
    fn test_denied_enforce_mode() {
        let policy = NetworkPolicy {
            enabled: true,
            mode: Some(NetworkPolicyMode::Enforce),
            default_action: Some(NetworkAction::Deny),
            rules: vec![],
        };

        let matcher = NetworkPolicyMatcher::new(policy);
        let enforcer = NetworkEnforcer::new(matcher, NetworkPolicyMode::Enforce);

        let event = create_test_event("8.8.8.8", 443);
        let result = enforcer.process(&event, 1234);

        assert_eq!(result, EnforceResult::Killed);
    }

    #[test]
    fn test_to_audit_result_allowed() {
        let result = NetworkEnforcer::to_audit_result(&EnforceResult::Allowed);
        assert_eq!(result, NetworkAccessResult::Allowed);
    }

    #[test]
    fn test_to_audit_result_monitored() {
        let result = NetworkEnforcer::to_audit_result(&EnforceResult::Monitored);
        assert_eq!(result, NetworkAccessResult::Monitored);
    }

    #[test]
    fn test_to_audit_result_killed() {
        let result = NetworkEnforcer::to_audit_result(&EnforceResult::Killed);
        assert_eq!(result, NetworkAccessResult::DeniedByPolicy);
    }
}
