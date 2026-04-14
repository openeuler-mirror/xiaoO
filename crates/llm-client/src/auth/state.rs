use std::time::Instant;

#[derive(Clone, Debug)]
pub enum AuthState {
    Ready,
    Cooldown { until: Instant },
    Blocked { reason: String },
    Disabled,
}

impl Default for AuthState {
    fn default() -> Self {
        Self::Ready
    }
}

impl AuthState {
    pub fn ready() -> Self {
        Self::Ready
    }

    pub fn cooldown(duration: std::time::Duration) -> Self {
        Self::Cooldown {
            until: Instant::now() + duration,
        }
    }

    pub fn blocked(reason: impl Into<String>) -> Self {
        Self::Blocked {
            reason: reason.into(),
        }
    }

    pub fn disabled() -> Self {
        Self::Disabled
    }

    pub fn is_available(&self) -> bool {
        match self {
            Self::Ready => true,
            Self::Cooldown { until } => Instant::now() >= *until,
            Self::Blocked { .. } | Self::Disabled => false,
        }
    }

    pub fn cooldown_remaining(&self) -> Option<std::time::Duration> {
        match self {
            Self::Cooldown { until } => {
                let now = Instant::now();
                if now < *until {
                    Some(*until - now)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    pub fn is_in_cooldown(&self) -> bool {
        matches!(self, Self::Cooldown { .. })
    }

    pub fn is_blocked(&self) -> bool {
        matches!(self, Self::Blocked { .. })
    }

    pub fn is_disabled(&self) -> bool {
        matches!(self, Self::Disabled)
    }

    pub fn block_reason(&self) -> Option<&str> {
        match self {
            Self::Blocked { reason } => Some(reason),
            _ => None,
        }
    }

    pub fn maybe_recover(&mut self) -> bool {
        if let Self::Cooldown { until } = self {
            if Instant::now() >= *until {
                *self = Self::Ready;
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ready_state_is_available() {
        let state = AuthState::ready();
        assert!(state.is_available());
        assert!(!state.is_in_cooldown());
        assert!(!state.is_blocked());
        assert!(!state.is_disabled());
    }

    #[test]
    fn test_cooldown_state_not_available() {
        let state = AuthState::cooldown(std::time::Duration::from_secs(60));
        assert!(!state.is_available());
        assert!(state.is_in_cooldown());
        assert!(state.cooldown_remaining().is_some());
    }

    #[test]
    fn test_blocked_state_not_available() {
        let state = AuthState::blocked("test error");
        assert!(!state.is_available());
        assert!(state.is_blocked());
        assert_eq!(state.block_reason(), Some("test error"));
    }

    #[test]
    fn test_disabled_state_not_available() {
        let state = AuthState::disabled();
        assert!(!state.is_available());
        assert!(state.is_disabled());
    }

    #[test]
    fn test_cooldown_expiry() {
        let mut state = AuthState::cooldown(std::time::Duration::from_millis(10));
        assert!(!state.is_available());
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(state.maybe_recover());
        assert!(state.is_available());
    }

    #[test]
    fn test_default_is_ready() {
        let state = AuthState::default();
        assert!(state.is_available());
    }
}
