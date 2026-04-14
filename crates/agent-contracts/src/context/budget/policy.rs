use agent_types::BudgetError;

pub trait TokenBudgetPolicy: Send + Sync {
    fn total_budget(&self) -> usize;
    fn reserved_for_output(&self) -> usize;
    fn reserved_for_system(&self) -> usize;
    fn hard_limit_ratio(&self) -> f64;

    fn validate(&self) -> Result<(), BudgetError>;
    fn available_budget(&self) -> Result<usize, BudgetError>;
    fn history_limit(&self) -> Result<usize, BudgetError>;
}
