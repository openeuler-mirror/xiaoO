use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TokenBudgetConfig {
    pub total_budget: usize,
    pub reserved_for_output: usize,
    pub reserved_for_system: usize,
    pub hard_limit_ratio: f64,
}

#[derive(Debug, Error)]
pub enum BudgetError {
    #[error("invalid total budget: {message}")]
    InvalidTotalBudget { message: String },
    #[error("invalid hard limit ratio: {ratio}")]
    InvalidHardLimitRatio { ratio: f64 },
}
