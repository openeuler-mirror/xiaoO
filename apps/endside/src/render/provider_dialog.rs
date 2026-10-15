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

    /// 借出当前高亮 provider 的模型列表。渲染每帧都要用它，不该 clone 整份
    /// （官方列表 openrouter 300+；条数上限见 `provider.rs` 的 `MAX_MODELS_PER_PROVIDER`）。
    pub fn current_models_ref(&self) -> &[ModelInfo] {
        self.providers
            .get(self.selected_provider)
            .map(|p| p.models.as_slice())
            .unwrap_or(&[])
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

    /// 当前高亮 provider 的名字（官方列表拉取的触发对象）。
    pub fn selected_provider_name(&self) -> Option<&str> {
        self.providers
            .get(self.selected_provider)
            .map(|provider| provider.name.as_str())
    }

    /// 用官方列表替换某 provider 的模型。
    ///
    /// 空列表视为无效（调用方按失败处理），返回 `false` 且不改动任何状态；
    /// provider 名未命中同样返回 `false`。被替换的是当前高亮 provider 时，
    /// 选中下标收敛到新列表范围内，避免指向越界。
    pub fn replace_models(&mut self, provider: &str, models: Vec<ModelInfo>) -> bool {
        if models.is_empty() {
            return false;
        }
        let Some(index) = self
            .providers
            .iter()
            .position(|entry| entry.name == provider)
        else {
            return false;
        };

        if index == self.selected_provider {
            self.selected_model = self.selected_model.min(models.len().saturating_sub(1));
        }

        self.providers[index].models = models;
        true
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/endside/render/provider_dialog_test.rs"]
mod tests;
