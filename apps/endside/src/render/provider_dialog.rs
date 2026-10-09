use crate::chat::{ModelInfo, ProviderInfo};

#[derive(Debug, Clone)]
pub struct ProviderDialog {
    pub providers: Vec<ProviderInfo>,
    pub selected_provider: usize,
    pub selected_model: usize,
    pub focus: DialogFocus,
    pub local_models_loading: bool,
    /// 本次对话框会话中已完成后台拉取的 provider 名单，防止上下移动时
    /// 对同一 provider 重复发起请求（关闭重开对话框即重新拉取）。
    pub fetched_providers: Vec<String>,
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
            local_models_loading: false,
            fetched_providers: Vec::new(),
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

    pub fn set_local_models_loading(&mut self) {
        self.local_models_loading = true;
    }

    /// 用后台拉取结果替换指定 provider 的模型列表。
    ///
    /// 空结果保留静态列表（离线兜底）——`local` 除外，它回落到占位条目，
    /// 保证对话框永不出现死空列。
    pub fn apply_fetched_models(&mut self, provider_name: &str, models: Vec<ModelInfo>) {
        self.local_models_loading = false;
        let Some(provider) = self.providers.iter_mut().find(|p| p.name == provider_name) else {
            return;
        };
        if !models.is_empty() {
            provider.models = models;
        } else if provider_name == "local" {
            provider.models = vec![ModelInfo {
                id: "glm4.7".to_string(),
                name: "glm4.7".to_string(),
            }];
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/endside/render/provider_dialog_test.rs"]
mod tests;
