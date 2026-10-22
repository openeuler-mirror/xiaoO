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
    /// 而翻页必须按"实际一屏"走，所以由渲染侧量好写回来。`0` 表示这次弹窗创建后
    /// 还没有任何一帧回写过，此时翻页改为把窗口推进一行（既不会卡住，也不会一次跳到底）。
    pub page_height: usize,
    /// Providers 栏可视窗口的第一行：渲染时作为 `ListState` 的初始 offset，渲染后
    /// 再把 ratatui 校正过的 offset 收回这里，所以它始终等于屏幕上那一屏的起点。
    ///
    /// 为什么自己维护窗口而不是交给 ratatui：`ListState` 的默认行为是"最小
    /// 滚动"——选中项越出窗口一行，窗口才推一行。方向键这样最好，但整屏翻页
    /// 就会被它削成滚一行。所以翻页自己对齐整屏，方向键继续走最小滚动
    /// （[`Self::follow_selection`]），渲染时把窗口起点交给 `ListState::offset`。
    provider_window_start: usize,
    /// Models 栏窗口记账的第一行（同上）。
    model_window_start: usize,
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
            provider_window_start: 0,
            model_window_start: 0,
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

    /// 移动 provider 高亮的唯一入口。
    ///
    /// `selected_model` 是相对当前高亮 provider 的下标，跨 provider 不沿用，
    /// 因此下标一旦变化就归零——上下键、翻页、Home/End 共用这条规则。
    fn select_provider(&mut self, index: usize) {
        if index != self.selected_provider {
            self.selected_provider = index;
            self.selected_model = 0;
            self.model_window_start = 0;
        }
    }

    /// 翻页步长：一屏可见行数，至少 1 行。
    fn page_step(&self) -> usize {
        self.page_height.max(1)
    }

    fn max_provider_start(&self) -> usize {
        self.providers.len().saturating_sub(self.page_step())
    }

    fn max_model_start(&self) -> usize {
        self.current_model_count().saturating_sub(self.page_step())
    }

    /// Providers 栏可视窗口的第一行（按列表长度收敛，终端变矮/列表变短都不越界）。
    pub fn provider_window(&self) -> usize {
        self.provider_window_start.min(self.max_provider_start())
    }

    /// Models 栏可视窗口的第一行。
    pub fn model_window(&self) -> usize {
        self.model_window_start.min(self.max_model_start())
    }

    /// 渲染后回写"现在真正可见的窗口起点"（由 `render_provider_dialog` 调用）。
    ///
    /// ratatui 会把 offset 校正成看得见的第一项并把结果留在 `ListState` 里，把那个值
    /// 收回来，翻页才会从屏幕上真正的起点起算——否则列表刚被替换或预选到深处时，
    /// 下一次 `PgDn` 会从过期起点起算，高亮反向跳。
    pub fn record_visible_windows(&mut self, provider_start: usize, model_start: usize) {
        self.provider_window_start = provider_start;
        self.model_window_start = model_start;
    }

    /// 让窗口跟随选中项做"最小滚动"：选中项越出窗口一行，窗口才推一行。
    /// 与 ratatui `ListState` 的默认行为等价，只是窗口起点要自己记账，
    /// 翻页才能在此基础上整屏对齐。
    fn follow_selection(&mut self) {
        let page = self.page_step();
        match self.focus {
            DialogFocus::Providers => {
                if self.selected_provider < self.provider_window_start {
                    self.provider_window_start = self.selected_provider;
                } else if self.selected_provider >= self.provider_window_start.saturating_add(page)
                {
                    self.provider_window_start = self.selected_provider.saturating_add(1) - page;
                }
            }
            DialogFocus::Models => {
                if self.selected_model < self.model_window_start {
                    self.model_window_start = self.selected_model;
                } else if self.selected_model >= self.model_window_start.saturating_add(page) {
                    self.model_window_start = self.selected_model.saturating_add(1) - page;
                }
            }
        }
    }

    pub fn move_up(&mut self) {
        match self.focus {
            DialogFocus::Providers => {
                self.select_provider(self.selected_provider.saturating_sub(1));
            }
            DialogFocus::Models => {
                self.selected_model = self.selected_model.saturating_sub(1);
            }
        }
        self.follow_selection();
    }

    pub fn move_down(&mut self) {
        match self.focus {
            DialogFocus::Providers => {
                let last = self.providers.len().saturating_sub(1);
                self.select_provider(self.selected_provider.saturating_add(1).min(last));
            }
            DialogFocus::Models => {
                let last = self.current_model_count().saturating_sub(1);
                self.selected_model = self.selected_model.saturating_add(1).min(last);
            }
        }
        self.follow_selection();
    }

    /// `PageUp`：整屏回退，选中项落在新一屏的第一行。
    ///
    /// 回退目标是"上一页的页首"而不是 `窗口起点 - 一屏`：末屏与上一屏可能重叠
    /// （列表长度不是整屏的整数倍），减一屏会退到一个既不是页首、上一次也没
    /// 到过的位置，来回翻页就对不上了。
    ///
    /// 这里的"页"是按 `page` 切出来的**网格页**，与"用户上一次看到的那一屏"未必
    /// 是同一个东西：`record_visible_windows` 收回的是 ratatui 校正过的真实起点，
    /// 它可能不在网格上（列表刚被替换或深预选之后）。从这种起点 PgUp 会落到上一个
    /// 网格页首，与上一眼看到的那屏最多差 `page - 1` 行——这是有意的：只有对齐网格，
    /// 来回翻页才不会累积漂移。`page` 自身也可能因终端 resize 变化，回退距离随之改变。
    fn previous_page_start(&self, start: usize, page: usize) -> usize {
        start.saturating_sub(1) / page * page
    }

    pub fn page_up(&mut self) {
        let page = self.page_step();
        match self.focus {
            DialogFocus::Providers => {
                let start = self.previous_page_start(self.provider_window(), page);
                self.provider_window_start = start;
                self.select_provider(start);
            }
            DialogFocus::Models => {
                let start = self.previous_page_start(self.model_window(), page);
                self.model_window_start = start;
                self.selected_model = start;
            }
        }
    }

    /// `PageDown`：整屏前进，选中项落在新一屏的第一行；最后一屏对齐到列表末尾
    /// （不出现"只剩末项"的半空屏），再按不动。
    ///
    /// 选中项取 `max(当前, start)`：正常翻页时当前项一定在新页之前，所以它就等于
    /// 新页首；而末屏被 `max_*_start()` 夹取时，"新一屏的第一行"可能落在选中项
    /// **之前**（列表长度不是整屏整数倍，末屏与上一屏重叠），此时只移动窗口、
    /// 选中项不动——按 PgDn 让高亮往回跳是最容易让人困惑的行为。
    /// 这一条同时覆盖了"窗口已贴末尾"（`start == 当前窗口`）：窗口写入是幂等的，
    /// 选中项也不变，于是整表一屏或末屏上的 PgDn 什么都不做，不会误报
    /// "provider 变了"而触发一次模型列表拉取。
    pub fn page_down(&mut self) {
        let page = self.page_step();
        match self.focus {
            DialogFocus::Providers => {
                let start = self
                    .provider_window()
                    .saturating_add(page)
                    .min(self.max_provider_start());
                self.provider_window_start = start;
                // 只前进、不后退：末屏夹取时选中项可能已经在新窗口里。
                // `start` 恒不大于末项下标（见 `max_provider_start`），无需再夹一次。
                self.select_provider(self.selected_provider.max(start));
            }
            DialogFocus::Models => {
                let start = self
                    .model_window()
                    .saturating_add(page)
                    .min(self.max_model_start());
                self.model_window_start = start;
                self.selected_model = self.selected_model.max(start);
            }
        }
    }

    /// `Home`：回到首项与首屏。
    pub fn select_first(&mut self) {
        match self.focus {
            DialogFocus::Providers => {
                self.provider_window_start = 0;
                self.select_provider(0);
            }
            DialogFocus::Models => {
                self.model_window_start = 0;
                self.selected_model = 0;
            }
        }
    }

    /// `End`：选中末项，并让末屏可见；列表为空时保持 0，不越界。
    pub fn select_last(&mut self) {
        match self.focus {
            DialogFocus::Providers => {
                self.provider_window_start = self.max_provider_start();
                let last = self.providers.len().saturating_sub(1);
                self.select_provider(last);
            }
            DialogFocus::Models => {
                self.model_window_start = self.max_model_start();
                self.selected_model = self.current_model_count().saturating_sub(1);
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
