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
#[path = "../../../../tests/unit/llm-client/auth/state_test.rs"]
mod tests;
