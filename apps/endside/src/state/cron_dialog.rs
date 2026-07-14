//! Cron management dialog state for the TUI.
//!
//! Supports three modes:
//! - **List**: Browse cron jobs, select one to view details / delete / toggle.
//! - **ConfirmDelete**: Confirm before deleting a job.
//! - **EditForm**: Add or modify a cron job via form fields.

use crate::services::cron_service::CronJobEntry;

/// Which field is focused in the edit form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CronEditField {
    Name,
    Cron,
    Prompt,
    Description,
    AgentRole,
    TimeoutSecs,
    MaxRetries,
    RetryDelay,
}

impl CronEditField {
    pub fn next(self) -> Self {
        match self {
            Self::Name => Self::Cron,
            Self::Cron => Self::Prompt,
            Self::Prompt => Self::Description,
            Self::Description => Self::AgentRole,
            Self::AgentRole => Self::TimeoutSecs,
            Self::TimeoutSecs => Self::MaxRetries,
            Self::MaxRetries => Self::RetryDelay,
            Self::RetryDelay => Self::Name,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Name => Self::RetryDelay,
            Self::Cron => Self::Name,
            Self::Prompt => Self::Cron,
            Self::Description => Self::Prompt,
            Self::AgentRole => Self::Description,
            Self::TimeoutSecs => Self::AgentRole,
            Self::MaxRetries => Self::TimeoutSecs,
            Self::RetryDelay => Self::MaxRetries,
        }
    }
}

/// Mode of the cron dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CronDialogMode {
    /// Browsing the list of jobs.
    List,
    /// Confirming deletion of a job.
    ConfirmDelete { job_index: usize },
    /// Editing an existing job (Some) or creating a new job (None).
    EditForm {
        /// Index in the jobs vec if editing existing; `None` if adding new.
        editing_index: Option<usize>,
        /// The form field values.
        form: CronEditForm,
        /// Currently focused field.
        focus: CronEditField,
        /// Validation error message, if any.
        error: Option<String>,
    },
}

/// Form fields for editing a cron job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronEditForm {
    pub name: String,
    pub cron: String,
    pub prompt: String,
    pub description: String,
    pub agent_role: String,
    pub timeout_secs: String,
    pub max_retries: String,
    pub retry_delay: String,
}

impl Default for CronEditForm {
    fn default() -> Self {
        Self {
            name: String::new(),
            cron: String::new(),
            prompt: String::new(),
            description: String::new(),
            agent_role: String::new(),
            timeout_secs: String::new(),
            max_retries: String::new(),
            retry_delay: String::new(),
        }
    }
}

impl CronEditForm {
    /// Build a form pre-filled from an existing job entry.
    pub fn from_entry(entry: &CronJobEntry) -> Self {
        Self {
            name: entry.name.clone(),
            cron: entry.cron_raw.clone(),
            prompt: entry.prompt.clone(),
            description: entry.description.clone().unwrap_or_default(),
            agent_role: entry.agent_role.clone().unwrap_or_default(),
            timeout_secs: entry.timeout_secs.to_string(),
            max_retries: entry.max_retries.to_string(),
            retry_delay: entry.retry_delay_secs.to_string(),
        }
    }

    /// Get a mutable reference to the value string for a given field.
    pub fn field_value_mut(&mut self, field: CronEditField) -> &mut String {
        match field {
            CronEditField::Name => &mut self.name,
            CronEditField::Cron => &mut self.cron,
            CronEditField::Prompt => &mut self.prompt,
            CronEditField::Description => &mut self.description,
            CronEditField::AgentRole => &mut self.agent_role,
            CronEditField::TimeoutSecs => &mut self.timeout_secs,
            CronEditField::MaxRetries => &mut self.max_retries,
            CronEditField::RetryDelay => &mut self.retry_delay,
        }
    }

    /// Try to convert form values into a CronJobEntry.
    /// Returns an error string on validation failure.
    pub fn to_entry(&self, default_timeout_secs: u64) -> Result<CronJobEntry, String> {
        if self.name.trim().is_empty() {
            return Err("Name is required.".to_string());
        }
        if self.cron.trim().is_empty() {
            return Err("Cron expression is required.".to_string());
        }
        // Validate cron expression
        crate::services::cron_service::validate_cron_expr(self.cron.trim())?;
        if self.prompt.trim().is_empty() {
            return Err("Prompt is required.".to_string());
        }

        let timeout_secs = if self.timeout_secs.trim().is_empty() {
            default_timeout_secs
        } else {
            self.timeout_secs
                .trim()
                .parse::<u64>()
                .map_err(|_| "Timeout must be a valid number.".to_string())?
        };

        let max_retries = if self.max_retries.trim().is_empty() {
            0
        } else {
            self.max_retries
                .trim()
                .parse::<u32>()
                .map_err(|_| "Max retries must be a valid number.".to_string())?
        };

        let retry_delay_secs = if self.retry_delay.trim().is_empty() {
            60
        } else {
            self.retry_delay
                .trim()
                .parse::<u64>()
                .map_err(|_| "Retry delay must be a valid number.".to_string())?
        };

        Ok(CronJobEntry {
            name: self.name.trim().to_string(),
            description: if self.description.trim().is_empty() {
                None
            } else {
                Some(self.description.trim().to_string())
            },
            cron_raw: self.cron.trim().to_string(),
            cron_valid: true,
            prompt: self.prompt.trim().to_string(),
            agent_role: if self.agent_role.trim().is_empty() {
                None
            } else {
                Some(self.agent_role.trim().to_string())
            },
            timeout_secs,
            enabled: true,
            max_retries,
            retry_delay_secs,
        })
    }
}

/// Main dialog state.
#[derive(Debug, Clone)]
pub struct CronDialog {
    pub mode: CronDialogMode,
    pub selected: usize,
    /// Snapshot of jobs currently being managed.
    pub jobs: Vec<CronJobEntry>,
    /// Path to jobs.toml.
    pub jobs_file: std::path::PathBuf,
    pub default_timeout_secs: u64,
    pub cron_section_present: bool,
}

impl CronDialog {
    pub fn new(
        jobs: Vec<CronJobEntry>,
        jobs_file: std::path::PathBuf,
        default_timeout_secs: u64,
        cron_section_present: bool,
    ) -> Self {
        Self {
            mode: CronDialogMode::List,
            selected: if jobs.is_empty() { 0 } else { 0 },
            jobs,
            jobs_file,
            default_timeout_secs,
            cron_section_present,
        }
    }

    // ── List navigation ────────────────────────────────────

    pub fn move_up(&mut self) {
        if self.mode != CronDialogMode::List {
            return;
        }
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn move_down(&mut self) {
        if self.mode != CronDialogMode::List {
            return;
        }
        let max = if self.jobs.is_empty() {
            0
        } else {
            self.jobs.len().saturating_sub(1)
        };
        self.selected = (self.selected + 1).min(max);
    }

    // ── Actions ────────────────────────────────────────────

    /// Start the "add new job" flow.
    pub fn start_add(&mut self) {
        self.mode = CronDialogMode::EditForm {
            editing_index: None,
            form: CronEditForm::default(),
            focus: CronEditField::Name,
            error: None,
        };
    }

    /// Start editing the currently selected job.
    pub fn start_edit(&mut self) {
        if let Some(entry) = self.jobs.get(self.selected) {
            let form = CronEditForm::from_entry(entry);
            self.mode = CronDialogMode::EditForm {
                editing_index: Some(self.selected),
                form,
                focus: CronEditField::Name,
                error: None,
            };
        }
    }

    /// Ask for deletion confirmation of the selected job.
    pub fn start_delete_confirm(&mut self) {
        if self.jobs.get(self.selected).is_some() {
            self.mode = CronDialogMode::ConfirmDelete {
                job_index: self.selected,
            };
        }
    }

    /// Confirm and execute deletion.
    pub fn confirm_delete(&mut self) -> anyhow::Result<()> {
        if let CronDialogMode::ConfirmDelete { job_index } = self.mode {
            self.jobs.remove(job_index);
            self.selected = if self.jobs.is_empty() {
                0
            } else {
                self.selected.min(self.jobs.len().saturating_sub(1))
            };
            self.mode = CronDialogMode::List;
            self.save()?;
        }
        Ok(())
    }

    /// Toggle enabled/disabled for the selected job.
    pub fn toggle_enabled(&mut self) -> anyhow::Result<()> {
        if let Some(entry) = self.jobs.get_mut(self.selected) {
            entry.enabled = !entry.enabled;
            self.save()?;
        }
        Ok(())
    }

    /// Save the edit form: either add new or update existing.
    pub fn save_edit_form(&mut self) -> Result<(), String> {
        if let CronDialogMode::EditForm {
            editing_index,
            form,
            ..
        } = &self.mode
        {
            let entry = form.to_entry(self.default_timeout_secs)?;
            match editing_index {
                Some(index) => {
                    // Check for duplicate name if name changed
                    let old_name = &self.jobs[*index].name;
                    if entry.name != *old_name && self.jobs.iter().any(|j| j.name == entry.name) {
                        return Err(format!("Job name '{}' already exists.", entry.name));
                    }
                    self.jobs[*index] = entry;
                }
                None => {
                    if self.jobs.iter().any(|j| j.name == entry.name) {
                        return Err(format!("Job name '{}' already exists.", entry.name));
                    }
                    self.jobs.push(entry);
                    self.selected = self.jobs.len().saturating_sub(1);
                }
            }
            self.mode = CronDialogMode::List;
        }
        Ok(())
    }

    /// Save current jobs to file.
    fn save(&self) -> anyhow::Result<()> {
        // Build a temporary snapshot for writing
        let snapshot = crate::services::cron_service::CronConfigSnapshot {
            jobs_file: self.jobs_file.clone(),
            cron_section_present: self.cron_section_present,
            default_timeout_secs: self.default_timeout_secs,
            jobs: self.jobs.clone(), // Not actually used by write_jobs, we pass jobs separately
        };
        crate::services::cron_service::write_jobs(&snapshot, &self.jobs)
    }

    /// Go back to list mode.
    pub fn back_to_list(&mut self) {
        self.mode = CronDialogMode::List;
    }

    // ── Edit form field navigation ─────────────────────────

    pub fn edit_next_field(&mut self) {
        if let CronDialogMode::EditForm { focus, .. } = &mut self.mode {
            *focus = focus.next();
        }
    }

    pub fn edit_prev_field(&mut self) {
        if let CronDialogMode::EditForm { focus, .. } = &mut self.mode {
            *focus = focus.prev();
        }
    }

    /// Push a character to the currently focused edit field.
    pub fn edit_push_char(&mut self, c: char) {
        if let CronDialogMode::EditForm { form, focus, .. } = &mut self.mode {
            form.field_value_mut(*focus).push(c);
        }
    }

    /// Delete last character from the currently focused edit field.
    pub fn edit_backspace(&mut self) {
        if let CronDialogMode::EditForm { form, focus, .. } = &mut self.mode {
            form.field_value_mut(*focus).pop();
        }
    }

    /// Clear error on the edit form.
    pub fn edit_clear_error(&mut self) {
        if let CronDialogMode::EditForm { error, .. } = &mut self.mode {
            *error = None;
        }
    }

    /// Set error on the edit form.
    pub fn edit_set_error(&mut self, msg: String) {
        if let CronDialogMode::EditForm { error, .. } = &mut self.mode {
            *error = Some(msg);
        }
    }
}
