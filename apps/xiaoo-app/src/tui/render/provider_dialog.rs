use crate::chat::{ModelInfo, ProviderInfo};

#[derive(Debug, Clone)]
pub struct ProviderDialog {
    pub providers: Vec<ProviderInfo>,
    pub selected_provider: usize,
    pub selected_model: usize,
    pub focus: DialogFocus,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DialogFocus {
    Providers,
    Models,
}

impl ProviderDialog {
    pub fn new(providers: Vec<ProviderInfo>) -> Self {
        Self {
            providers,
            selected_provider: 0,
            selected_model: 0,
            focus: DialogFocus::Providers,
        }
    }

    pub fn new_with_selection(
        providers: Vec<ProviderInfo>,
        provider_name: Option<&str>,
        model_id: Option<&str>,
    ) -> Self {
        let mut dialog = Self::new(providers);

        let Some(provider_name) = provider_name.filter(|name| !name.is_empty()) else {
            return dialog;
        };

        if let Some(provider_index) = dialog
            .providers
            .iter()
            .position(|provider| provider.name.eq_ignore_ascii_case(provider_name))
        {
            dialog.selected_provider = provider_index;
            if let Some(model_id) = model_id.filter(|id| !id.is_empty()) {
                if let Some(model_index) = dialog.providers[provider_index]
                    .models
                    .iter()
                    .position(|model| model.id.eq_ignore_ascii_case(model_id))
                {
                    dialog.selected_model = model_index;
                }
            }
        }

        dialog
    }

    pub fn current_models(&self) -> Vec<ModelInfo> {
        self.providers
            .get(self.selected_provider)
            .map(|p| p.models.clone())
            .unwrap_or_default()
    }

    pub fn move_up(&mut self) {
        match self.focus {
            DialogFocus::Providers => {
                if self.selected_provider > 0 {
                    self.selected_provider -= 1;
                    self.selected_model = 0;
                }
            }
            DialogFocus::Models => {
                if self.selected_model > 0 {
                    self.selected_model -= 1;
                }
            }
        }
    }

    pub fn move_down(&mut self) {
        match self.focus {
            DialogFocus::Providers => {
                if self.selected_provider < self.providers.len().saturating_sub(1) {
                    self.selected_provider += 1;
                    self.selected_model = 0;
                }
            }
            DialogFocus::Models => {
                let models = self.current_models();
                if self.selected_model < models.len().saturating_sub(1) {
                    self.selected_model += 1;
                }
            }
        }
    }

    pub fn switch_to_providers(&mut self) {
        self.focus = DialogFocus::Providers;
    }

    pub fn switch_to_models(&mut self) {
        self.focus = DialogFocus::Models;
    }

    pub fn selected(&self) -> Option<(String, String)> {
        let provider = self.providers.get(self.selected_provider)?;
        let model = provider.models.get(self.selected_model)?;
        Some((provider.name.clone(), model.id.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn providers() -> Vec<ProviderInfo> {
        vec![
            ProviderInfo {
                name: "openai".to_string(),
                models: vec![
                    ModelInfo {
                        id: "gpt-4o".to_string(),
                        name: "GPT-4o".to_string(),
                    },
                    ModelInfo {
                        id: "gpt-4-turbo".to_string(),
                        name: "GPT-4 Turbo".to_string(),
                    },
                ],
            },
            ProviderInfo {
                name: "deepseek".to_string(),
                models: vec![
                    ModelInfo {
                        id: "deepseek-v4-flash".to_string(),
                        name: "DeepSeek V4 Flash".to_string(),
                    },
                    ModelInfo {
                        id: "deepseek-v4-pro".to_string(),
                        name: "DeepSeek V4 Pro".to_string(),
                    },
                    ModelInfo {
                        id: "deepseek-chat".to_string(),
                        name: "DeepSeek Chat V3".to_string(),
                    },
                ],
            },
        ]
    }

    #[test]
    fn new_with_selection_preselects_matching_provider_and_model() {
        let dialog =
            ProviderDialog::new_with_selection(providers(), Some("openai"), Some("gpt-4-turbo"));

        assert_eq!(dialog.selected_provider, 0);
        assert_eq!(dialog.selected_model, 1);
        assert_eq!(
            dialog.selected(),
            Some(("openai".to_string(), "gpt-4-turbo".to_string()))
        );
    }

    #[test]
    fn new_with_selection_falls_back_to_first_model_when_model_missing() {
        let dialog = ProviderDialog::new_with_selection(
            providers(),
            Some("deepseek"),
            Some("missing-model"),
        );

        assert_eq!(dialog.selected_provider, 1);
        assert_eq!(dialog.selected_model, 0);
        assert_eq!(
            dialog.selected(),
            Some(("deepseek".to_string(), "deepseek-v4-flash".to_string()))
        );
    }
}
