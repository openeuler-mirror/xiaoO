use crate::chat::{ModelInfo, ProviderInfo};

#[derive(Debug, Clone)]
pub struct ProviderDialog {
    pub providers: Vec<ProviderInfo>,
    pub selected_provider: usize,
    pub selected_model: usize,
    pub focus: DialogFocus,
    /// 列表内容区最近一次渲染出的可见行数，由 `render_provider_dialog` 回写。
    ///
    /// 弹窗高度随终端在 12..20 行之间浮动，一屏放得下几行只有渲染时才知道，
    /// 所以由渲染侧量好写回来。`0` 表示这次弹窗创建后还没有任何一帧回写过；
    /// 读取方应按"至少 1 行"兜底，不要假设它一定会被回写成非零。
    pub page_height: usize,
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
            page_height: 0,
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

    /// 当前高亮 provider 的模型条数。取长度不该 clone 整份列表：官方列表
    /// （openrouter 300+）下，方向键每按一次 clone 一遍是纯浪费。
    pub fn current_model_count(&self) -> usize {
        self.providers
            .get(self.selected_provider)
            .map_or(0, |provider| provider.models.len())
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
                let last = self.current_model_count().saturating_sub(1);
                self.selected_model = self.selected_model.saturating_add(1).min(last);
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

    /// 用官方列表替换某 provider 的模型，并尽量保持当前选中项。
    ///
    /// 选中项按 **id** 保持（不是按下标），因为官方列表的顺序与内置表不同：
    /// 替换前的选中 id 若仍在新列表里就选中它，否则回到首位——注意"首位是激活模型"
    /// 只在官方列表不含激活模型时成立（调用方会先 `pin_configured_model` 补到首位）；
    /// 官方列表本来含它时，首位就是官方列表的第一行。
    ///
    /// 空列表视为无效（调用方按失败处理），返回 `false` 且不改动任何状态；
    /// provider 名未命中同样返回 `false`。
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

        // 选中下标只在被替换的 provider 就是当前高亮项时才重算，否则别动
        // 用户在另一个 provider 上的选择（下标是相对当前高亮项而言的）。
        if index == self.selected_provider {
            let previous_id = self.providers[index]
                .models
                .get(self.selected_model)
                .map(|model| model.id.clone());
            let kept_index = previous_id.as_deref().and_then(|previous_id| {
                models
                    .iter()
                    .position(|model| model.id.eq_ignore_ascii_case(previous_id))
            });
            self.selected_model = kept_index.unwrap_or(0).min(models.len().saturating_sub(1));
        }

        self.providers[index].models = models;
        true
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/endside/render/provider_dialog_test.rs"]
mod tests;
