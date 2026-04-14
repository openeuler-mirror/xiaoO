use agent_contracts::TokenBudgetPolicy;
use agent_types::compression::{ContextAnalysis, ContextSeverity};
use agent_types::{BudgetError, TokenBudgetConfig};
use serde::{Deserialize, Serialize};

use crate::{CompactError, CompactResult};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CompactionPolicy {
    pub total_budget: usize,
    pub reserved_for_output: usize,
    pub reserved_for_system: usize,
    pub hard_limit_ratio: f64,
}

impl CompactionPolicy {
    pub fn from_budget(budget: &TokenBudgetConfig) -> Self {
        Self {
            total_budget: budget.total_budget,
            reserved_for_output: budget.reserved_for_output,
            reserved_for_system: budget.reserved_for_system,
            hard_limit_ratio: budget.hard_limit_ratio,
        }
    }

    pub fn available_budget(&self) -> CompactResult<usize> {
        if self.total_budget == 0 {
            return Err(CompactError::InvalidConfiguration {
                message: "total_budget must be greater than zero".to_string(),
            });
        }
        if !(0.0..=1.0).contains(&self.hard_limit_ratio) {
            return Err(CompactError::InvalidConfiguration {
                message: format!("invalid hard_limit_ratio: {}", self.hard_limit_ratio),
            });
        }
        Ok(self
            .total_budget
            .saturating_sub(self.reserved_for_output)
            .saturating_sub(self.reserved_for_system))
    }

    pub fn history_limit(&self) -> CompactResult<usize> {
        let available = self.available_budget()?;
        Ok((available as f64 * self.hard_limit_ratio).floor() as usize)
    }
}

impl TokenBudgetPolicy for CompactionPolicy {
    fn total_budget(&self) -> usize {
        self.total_budget
    }

    fn reserved_for_output(&self) -> usize {
        self.reserved_for_output
    }

    fn reserved_for_system(&self) -> usize {
        self.reserved_for_system
    }

    fn hard_limit_ratio(&self) -> f64 {
        self.hard_limit_ratio
    }

    fn validate(&self) -> Result<(), BudgetError> {
        if self.total_budget == 0 {
            return Err(BudgetError::InvalidTotalBudget {
                message: "total_budget must be greater than zero".to_string(),
            });
        }
        if !(0.0..=1.0).contains(&self.hard_limit_ratio) {
            return Err(BudgetError::InvalidHardLimitRatio {
                ratio: self.hard_limit_ratio,
            });
        }
        Ok(())
    }

    fn available_budget(&self) -> Result<usize, BudgetError> {
        TokenBudgetPolicy::validate(self)?;
        Ok(self
            .total_budget
            .saturating_sub(self.reserved_for_output)
            .saturating_sub(self.reserved_for_system))
    }

    fn history_limit(&self) -> Result<usize, BudgetError> {
        let available = TokenBudgetPolicy::available_budget(self)?;
        Ok((available as f64 * self.hard_limit_ratio).floor() as usize)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ContextThresholds {
    pub warning_ratio: f64,
    pub auto_compact_ratio: f64,
    pub blocking_ratio: f64,
}

impl ContextThresholds {
    pub fn validate(&self) -> CompactResult<()> {
        let ratios = [
            ("warning_ratio", self.warning_ratio),
            ("auto_compact_ratio", self.auto_compact_ratio),
            ("blocking_ratio", self.blocking_ratio),
        ];

        for (name, ratio) in ratios {
            if !(0.0 < ratio && ratio <= 1.0) {
                return Err(CompactError::InvalidConfiguration {
                    message: format!("{name} must be in (0.0, 1.0]"),
                });
            }
        }

        if !(self.warning_ratio <= self.auto_compact_ratio
            && self.auto_compact_ratio <= self.blocking_ratio)
        {
            return Err(CompactError::InvalidConfiguration {
                message: "warning_ratio <= auto_compact_ratio <= blocking_ratio must hold"
                    .to_string(),
            });
        }

        Ok(())
    }
}

pub struct CompactionPolicyService {
    thresholds: ContextThresholds,
}

impl CompactionPolicyService {
    pub fn new(thresholds: ContextThresholds) -> CompactResult<Self> {
        thresholds.validate()?;
        Ok(Self { thresholds })
    }

    pub fn thresholds(&self) -> &ContextThresholds {
        &self.thresholds
    }

    pub fn analyze(
        &self,
        estimated_tokens: usize,
        policy: &CompactionPolicy,
    ) -> CompactResult<ContextAnalysis> {
        let history_limit = policy.history_limit()? as f64;
        if history_limit <= 0.0 {
            return Err(CompactError::InvalidConfiguration {
                message: "history_limit must be greater than zero (check total_budget vs reserved tokens)".to_string(),
            });
        }
        let history_limit_usize = history_limit as usize;
        let warning_threshold = (history_limit * self.thresholds.warning_ratio).floor() as usize;
        let auto_compact_threshold =
            (history_limit * self.thresholds.auto_compact_ratio).floor() as usize;
        let blocking_threshold = (history_limit * self.thresholds.blocking_ratio).floor() as usize;

        let severity = if estimated_tokens >= blocking_threshold {
            ContextSeverity::Blocking
        } else if estimated_tokens >= auto_compact_threshold {
            ContextSeverity::AutoCompact
        } else if estimated_tokens >= warning_threshold {
            ContextSeverity::Warning
        } else {
            ContextSeverity::Normal
        };

        Ok(ContextAnalysis {
            estimated_tokens,
            should_compact: !matches!(severity, ContextSeverity::Normal),
            severity,
            total_tokens: estimated_tokens,
            available_tokens: history_limit_usize.saturating_sub(estimated_tokens),
            usage_ratio: estimated_tokens as f64 / history_limit,
        })
    }
}
