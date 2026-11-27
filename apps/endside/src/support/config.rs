use crate::backend::GatewayBackendConfig;
use crate::provider_defaults::default_api_key_env_for_provider;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use xiaoo_api::chat::HookerRegistryConfig;
use xiaoo_api::chat::ReasoningEffort;
use xiaoo_api::llm::{
    FlatLlmConnection, FlatLlmConnectionTarget, LlmModelEntry, RequestHeadersConfig,
};
use xiaoo_shared::builtin_agent_roles::{PLAN_AGENT_DESCRIPTION, PLAN_AGENT_ID, PLAN_AGENT_PROMPT};
use xiaoo_shared::config_lock::{hidden_sibling, ConfigFileLock};
use xiaoo_shared::gateway::MemoryAutomationConfig;
use xiaoo_shared::lsp_support::{build_extra_server_configs, LspServiceRegistry};
use xiaoo_shared::mcp_support::{self, McpSection};
use xiaoo_shared::skills_support::SkillsConfig as ResolvedSkillsConfig;
pub use xiaoo_shared::skills_support::SkillsSection;
use xiaoo_shared::text_utils::non_blank_trimmed;

const DEFAULT_AGENT_ID: &str = "main";
const DEFAULT_LLM_MAX_TOKENS: u32 = 16384;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LspConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub disabled_servers: Vec<String>,
    #[serde(default)]
    pub extra_servers: Vec<ExtraServerConfig>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExtraServerConfig {
    pub id: String,
    pub extensions: Vec<String>,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub root_markers: Vec<String>,
    pub language_id: String,
}

impl xiaoo_shared::lsp_support::ExtraServerConfigView for ExtraServerConfig {
    fn id(&self) -> &str {
        &self.id
    }

    fn extensions(&self) -> &[String] {
        &self.extensions
    }

    fn command(&self) -> &str {
        &self.command
    }

    fn args(&self) -> &[String] {
        &self.args
    }

    fn root_markers(&self) -> &[String] {
        &self.root_markers
    }

    fn language_id(&self) -> &str {
        &self.language_id
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub checkpoint: xiaoo_api::runtime::checkpoint::CheckpointConfig,
    #[serde(default)]
    pub llm: LlmConfig,
    #[serde(default)]
    pub trace: Option<Value>,
    #[serde(default)]
    pub skills: Option<SkillsSection>,
    #[serde(default)]
    pub agent: BTreeMap<String, AgentRoleConfig>,
    #[serde(default)]
    pub subagent: BTreeMap<String, SubagentRoleConfig>,
    #[serde(default)]
    pub agents: AgentsConfig,
    #[serde(default)]
    pub hooker: HookerRegistryConfig,
    #[serde(default)]
    pub operation_backend: Option<GatewayBackendConfig>,
    #[serde(default)]
    pub lsp: Option<LspConfig>,
    #[serde(default)]
    pub tui: TuiConfig,
    #[serde(default)]
    pub mcp: McpSection,
    #[serde(default)]
    pub memory_automation: MemoryAutomationConfig,
    /// Effective MCP servers after adding optional `.mcp.json` entries. This
    /// is runtime-only so TUI config saves never copy imported servers into
    /// `config.toml`.
    #[serde(skip)]
    runtime_mcp_servers: Option<Vec<mcp_support::McpServerConfig>>,
    /// Preserve daemon- or plugin-owned top-level sections when the TUI
    /// rewrites config.toml after changing providers or other UI settings.
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TuiConfig {
    #[serde(default)]
    pub remote: Option<RemoteConfig>,
    #[serde(default)]
    pub agent_order: Vec<String>,
    /// Redact secrets echoed by the assistant in the TUI display. Default
    /// `false` (local TUI shows the transcript as-is); set `true` for
    /// shoulder-surf protection. Only affects the display sink — raw secrets
    /// remain in history/snapshots regardless.
    #[serde(default = "default_false")]
    pub redact_secrets_display: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RemoteConfig {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub bearer_token_env: Option<String>,
    #[serde(default)]
    pub auto_connect: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LlmConfig {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub api_base: String,
    #[serde(flatten)]
    pub request_headers: RequestHeadersConfig,
    /// Ordered `[[llm.models]]` entries; entry 0 is active.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<LlmModelEntry>,
    #[serde(default = "default_llm_max_tokens")]
    pub max_tokens: u32,
    #[serde(default)]
    pub reasoning_effort: ReasoningEffort,
    #[serde(default = "default_false")]
    pub kvcache_enabled: bool,
    #[serde(default = "default_false")]
    pub kvcache_debug_enabled: bool,
    /// Daemon-owned keys, round-tripped verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profiles: Option<toml::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_profile: Option<toml::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<toml::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit: Option<toml::Value>,
    #[serde(skip)]
    pub(crate) models_normalized: bool,
}

fn default_false() -> bool {
    false
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: String::new(),
            model: String::new(),
            api_key_env: None,
            api_base: String::new(),
            request_headers: Default::default(),
            models: Vec::new(),
            max_tokens: default_llm_max_tokens(),
            reasoning_effort: ReasoningEffort::Default,
            kvcache_enabled: false,
            kvcache_debug_enabled: false,
            profiles: None,
            active_profile: None,
            context_window: None,
            rate_limit: None,
            models_normalized: false,
        }
    }
}

impl LlmConfig {
    /// Apply the shared activation rule and mirror entry 0; idempotent.
    pub fn normalize_models(&mut self) -> Result<()> {
        if self.models.is_empty() || self.models_normalized {
            return Ok(());
        }
        let flat = FlatLlmConnection::from_parts(
            Some(self.provider.clone()),
            Some(self.model.clone()),
            self.api_key_env.clone(),
            Some(self.api_base.clone()),
            self.request_headers.clone(),
            self.profiles
                .as_ref()
                .and_then(|profiles| profiles.as_table())
                .map_or(0, |table| table.len()),
            self.active_profile
                .as_ref()
                .and_then(|value| value.as_str()),
        );
        LlmModelEntry::activate_entry(&self.models, &flat)?;
        self.mirror_entry_zero_to_flat();
        Ok(())
    }

    fn mirror_entry_zero_to_flat(&mut self) {
        let Some(entry) = self.models.first().cloned() else {
            return;
        };
        entry.copy_into_flat(self);
        self.models_normalized = true;
    }

    /// Persisting the flat fields next to the array would self-conflict on reload.
    fn clear_flat_connection_fields(&mut self) {
        if self.models.is_empty() {
            return;
        }
        self.provider = String::new();
        self.model = String::new();
        self.api_key_env = None;
        self.api_base = String::new();
        self.request_headers = RequestHeadersConfig::default();
    }

    /// `/connect` rewrite: a flat form is seeded as one entry (unless
    /// the daemon-owned profile keys keep it flat); a matching entry
    /// moves to index 0, otherwise the selection is inserted there.
    /// Persisting callers must re-validate with
    /// [`LlmModelEntry::validate_entries`].
    pub(crate) fn apply_active_selection(
        &mut self,
        provider: &str,
        model: &str,
        api_key_env: Option<String>,
        api_base: &str,
    ) {
        let api_key_env = non_blank_trimmed(api_key_env.as_deref()).map(str::to_string);
        if self.models.is_empty() {
            if profiles_owned(self) {
                apply_flat_selection(self, provider, model, api_key_env, api_base);
                return;
            }
            seed_models_entry_from_flat(self);
        }

        let index = self.models.iter().position(|entry| {
            entry_matches_selection(entry, provider, model, &api_key_env, api_base)
        });
        match index {
            Some(index) => {
                let entry = self.models.remove(index);
                self.models.insert(0, entry);
            }
            None => insert_selection_as_entry_zero(self, provider, model, api_key_env, api_base),
        }
        for entry in &mut self.models {
            *entry = entry.trimmed();
        }
        self.mirror_entry_zero_to_flat();
    }
}

impl FlatLlmConnectionTarget for LlmConfig {
    fn set_provider(&mut self, provider: String) {
        self.provider = provider;
    }
    fn set_model(&mut self, model: String) {
        self.model = model;
    }
    fn set_api_base(&mut self, api_base: Option<String>) {
        self.api_base = api_base.unwrap_or_default();
    }
    fn set_api_key_env(&mut self, api_key_env: Option<String>) {
        self.api_key_env = api_key_env;
    }
    fn set_request_headers(&mut self, request_headers: RequestHeadersConfig) {
        self.request_headers = request_headers;
    }
}

// ── `/connect` match-or-insert rewrite helpers ──────────────────────────────

/// Insert the selection as entry 0, inheriting entry-0 headers only on the same endpoint.
fn insert_selection_as_entry_zero(
    llm: &mut LlmConfig,
    provider: &str,
    model: &str,
    api_key_env: Option<String>,
    api_base: &str,
) {
    let provider = provider.trim();
    let model = model.trim();
    let inherited_headers = llm
        .models
        .first()
        .filter(|active| {
            active.provider.trim().eq_ignore_ascii_case(provider)
                && effective_api_base(
                    active.provider.trim(),
                    active.api_base.as_deref().unwrap_or_default(),
                ) == effective_api_base(provider, api_base)
        })
        .map(|active| active.request_headers.clone())
        .unwrap_or_default();
    llm.models.insert(
        0,
        LlmModelEntry {
            provider: provider.to_string(),
            model: model.to_string(),
            api_base: non_empty_trimmed(api_base),
            api_key_env,
            request_headers: inherited_headers,
        },
    );
}

/// Daemon-owned profile keys declared: such a file must stay flat.
fn profiles_owned(llm: &LlmConfig) -> bool {
    let (profiles_present, active_profile_present) = FlatLlmConnection::profile_key_flags(
        llm.profiles
            .as_ref()
            .and_then(|profiles| profiles.as_table())
            .map_or(0, |table| table.len()),
        llm.active_profile.as_ref().and_then(|value| value.as_str()),
    );
    profiles_present || active_profile_present
}

fn apply_flat_selection(
    llm: &mut LlmConfig,
    provider: &str,
    model: &str,
    api_key_env: Option<String>,
    api_base: &str,
) {
    if !llm.provider.trim().eq_ignore_ascii_case(provider.trim())
        || effective_api_base(&llm.provider, &llm.api_base)
            != effective_api_base(provider, api_base)
    {
        llm.request_headers = Default::default();
    }
    llm.provider = provider.trim().to_string();
    llm.model = model.trim().to_string();
    llm.api_key_env = api_key_env;
    llm.api_base = api_base.trim().to_string();
}

fn seed_models_entry_from_flat(llm: &mut LlmConfig) {
    if llm.provider.trim().is_empty() || llm.model.trim().is_empty() {
        return;
    }
    llm.models.push(LlmModelEntry {
        provider: llm.provider.clone(),
        model: llm.model.clone(),
        api_base: non_empty_trimmed(&llm.api_base),
        api_key_env: non_blank_trimmed(llm.api_key_env.as_deref()).map(str::to_string),
        request_headers: llm.request_headers.clone(),
    });
}

/// Provider matches case-insensitively (model ids case-sensitively), plus effective api_base and key env.
fn entry_matches_selection(
    entry: &LlmModelEntry,
    provider: &str,
    model: &str,
    api_key_env: &Option<String>,
    api_base: &str,
) -> bool {
    if !entry.provider.trim().eq_ignore_ascii_case(provider.trim())
        || entry.model.trim() != model.trim()
    {
        return false;
    }
    if effective_api_base(
        &entry.provider,
        entry.api_base.as_deref().unwrap_or_default(),
    ) != effective_api_base(provider, api_base)
    {
        return false;
    }
    let entry_default_env = default_api_key_env_for_provider(entry.provider.trim());
    let selection_default_env = default_api_key_env_for_provider(provider.trim());
    let entry_env =
        non_blank_trimmed(entry.api_key_env.as_deref()).or(entry_default_env.as_deref());
    let selection_env =
        non_blank_trimmed(api_key_env.as_deref()).or(selection_default_env.as_deref());
    entry_env == selection_env
}

pub(crate) fn non_empty_trimmed(value: &str) -> Option<String> {
    non_blank_trimmed(Some(value)).map(str::to_string)
}

/// Trimmed base or the provider default when blank; trailing slash removed.
fn effective_api_base<'a>(provider: &str, api_base: &'a str) -> &'a str {
    let provider = provider.trim();
    let api_base = api_base.trim();
    let api_base = if api_base.is_empty() {
        xiaoo_api::llm::resolve_provider_profile(provider)
            .and_then(|profile| profile.default_base_url)
            .unwrap_or_default()
    } else {
        api_base
    };
    api_base.trim_end_matches('/')
}

/// TUI-side mirror of the daemon's `AgentsConfig`
/// (`apps/serverside/src/daemon_config.rs`). `default_agent_id` is
/// `Option<String>` so the daemon's standard "unset" spelling — the key
/// ABSENT — round-trips as `None` and never materializes as
/// `default_agent_id = "main"` on save (which would either flip the
/// daemon's root agent or fail its startup validation on an empty list).
/// An explicit-but-blank `""` is treated as unset here for local-mode
/// compatibility; the daemon rejects that spelling at startup, so it
/// cannot fork remote-mode resolution.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AgentsConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub list: Vec<AgentConfig>,
}

/// TUI-side mirror of the daemon's `AgentConfig`
/// (`apps/serverside/src/daemon_config.rs`). The TUI does not consume
/// `default`/`profile_id`/`system_prompt`, but they MUST parse
/// (and round-trip through `save_to`) so the shared config.toml keeps the
/// daemon's fields intact and both sides resolve the same root agent id:
/// `resolve_default_agent_id` mirrors the daemon's fallback chain, which
/// includes the `default = true` entry. `workspace` is the exception: the
/// TUI reads it to suppress the client-cwd hint for pinned agents
/// (see `remote_open_workspace_hint`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AgentConfig {
    pub id: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub default: bool,
    /// Explicit workspace pin; when set, the daemon's configured workspace
    /// wins over the client-cwd hint (see `remote_open_workspace_hint`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Write via a temp sibling + `fs::rename` so a crash cannot truncate a good file.
fn write_config_atomically(path: &Path, content: &str) -> Result<()> {
    let temp_path = hidden_sibling(path, &unique_temp_suffix());
    let result =
        write_temp_file(&temp_path, content, path).and_then(|_| fs::rename(&temp_path, path));
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result.with_context(|| format!("failed to write config file {}", path.display()))
}

fn unique_temp_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static CALL: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!(
        "tmp-{}-{}-{}",
        std::process::id(),
        nanos,
        CALL.fetch_add(1, Ordering::Relaxed)
    )
}

/// `create_new` (no symlink following), the destination's permission bits, and a flush.
fn write_temp_file(temp_path: &Path, content: &str, dest: &Path) -> std::io::Result<()> {
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(temp_path)?;
    if let Some(mode) = fs::metadata(dest)
        .ok()
        // Mask off the file-type bits; chmod only sees permission bits.
        .map(|meta| meta.permissions().mode() & 0o7777)
    {
        file.set_permissions(fs::Permissions::from_mode(mode))?;
    }
    file.write_all(content.as_bytes())?;
    file.sync_all()
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AgentRoleConfig {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub max_turns: Option<u32>,
    #[serde(default)]
    pub tools: BTreeMap<String, bool>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SubagentRoleConfig {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub max_turns: Option<u32>,
    #[serde(default)]
    pub tools: BTreeMap<String, bool>,
}

impl Config {
    pub fn load_from(path: &Path) -> Result<Self> {
        Ok(Self::load_from_with_content(path)?.0)
    }

    /// Read and parse the config, returning the content from the same
    /// read: the TUI's identity snapshot must hash the bytes that entered
    /// memory (`ConfigFileIdentity::from_loaded_content`).
    pub fn load_from_with_content(path: &Path) -> Result<(Self, String)> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("failed to read config file {}", path.display()))?;
        let mut config: Config = toml::from_str(&content).map_err(|error| {
            // The raw message may quote secrets; keep the location only.
            anyhow::anyhow!(
                "invalid TOML or config fields in {}{}",
                path.display(),
                xiaoo_shared::toml_errors::locate_toml_error(&content, &error)
            )
        })?;
        config
            .llm
            .normalize_models()
            .with_context(|| format!("invalid llm config in {}", path.display()))?;
        Ok((config, content))
    }

    /// Save under the cross-process config-file lock. Callers already
    /// holding the lock across a load→mutate→save cycle must use
    /// [`Self::save_to_locked`] (a nested `flock` would self-deadlock).
    pub fn save_to(&self, path: &Path) -> Result<String> {
        Self::ensure_config_parent_dir(path)?;
        let _lock = match ConfigFileLock::acquire(path) {
            Ok(lock) => Some(lock),
            Err(error) => {
                tracing::warn!(
                    "proceeding without the config lock for {}: {error:#}",
                    path.display()
                );
                None
            }
        };
        self.save_to_locked(path)
    }

    /// Save while the caller already holds the config-file lock.
    pub fn save_to_locked(&self, path: &Path) -> Result<String> {
        Self::ensure_config_parent_dir(path)?;
        let mut persisted = self.clone();
        persisted.agent.remove(PLAN_AGENT_ID);
        persisted.llm.clear_flat_connection_fields();
        let content = toml::to_string_pretty(&persisted).context("failed to serialize config")?;
        write_config_atomically(path, &content)?;
        // Written bytes let the caller refresh the identity snapshot.
        Ok(content)
    }

    fn ensure_config_parent_dir(path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create config directory {}", parent.display())
            })?;
        }
        Ok(())
    }

    pub fn load_runtime_mcp_servers(
        &mut self,
        explicit_path: Option<&Path>,
        workspace: &Path,
        home: Option<&Path>,
        toml_source: &Path,
    ) -> Result<(), mcp_support::McpConfigError> {
        self.runtime_mcp_servers = Some(load_merged_mcp_servers(
            &self.mcp.servers,
            explicit_path,
            workspace,
            home,
            toml_source,
        )?);
        Ok(())
    }

    pub fn mcp_servers(&self) -> &[mcp_support::McpServerConfig] {
        self.runtime_mcp_servers
            .as_deref()
            .unwrap_or(&self.mcp.servers)
    }

    pub fn list_agent_ids(&self) -> Vec<String> {
        self.agents
            .list
            .iter()
            .map(|agent| agent.id.to_lowercase())
            .collect()
    }

    /// Whether the daemon runs the e2b operation backend
    /// (`[server.operation_backend] kind = "e2b"`), read from the
    /// preserved `extra` section so config rewrites never drop other
    /// `[server]` keys.
    pub fn daemon_uses_e2b_backend(&self) -> bool {
        self.extra
            .get("server")
            .and_then(|server| server.get("operation_backend"))
            .and_then(|backend| backend.get("kind"))
            .and_then(toml::Value::as_str)
            // Exact match, aligned with the daemon's own case-sensitive check.
            .is_some_and(|kind| kind == "e2b")
    }

    /// Whether `agent_id` (lowercase) has an explicit workspace pin.
    pub fn agent_workspace_is_pinned(&self, agent_id: &str) -> bool {
        self.agents
            .list
            .iter()
            .any(|agent| agent.id.to_lowercase() == agent_id && agent.workspace.is_some())
    }

    /// Mirrors the daemon's fallback chain
    /// (`DaemonConfig::resolve_agent`, `apps/serverside/src/daemon_config.rs`):
    /// a non-empty `agents.default_agent_id` → the `default = true` entry →
    /// the first entry → `"main"`. The unset spelling matters: the daemon
    /// only walks the chain when the key is ABSENT (`Option<String>`), so
    /// the TUI must treat an absent key — not a defaulted `"main"` — as
    /// "walk the chain". Keeping the chains symmetric is load-bearing: in
    /// remote mode the TUI routes streamed agent events by re-resolving the
    /// root agent id locally (`is_root_stream_agent`), so any divergence
    /// hides the daemon's root replies inside a phantom subagent lane.
    /// A non-empty explicit id is trimmed before use, mirroring the
    /// daemon's `resolve_agent_by_id` (which trims); whitespace-only
    /// spellings are treated as unset, like `""`.
    pub fn resolve_default_agent_id(&self) -> String {
        if let Some(id) = self
            .agents
            .default_agent_id
            .as_deref()
            .filter(|id| !id.trim().is_empty())
        {
            return id.trim().to_lowercase();
        }
        self.agents
            .list
            .iter()
            .find(|agent| agent.default)
            .or_else(|| self.agents.list.first())
            .map(|agent| agent.id.to_lowercase())
            .unwrap_or_else(|| DEFAULT_AGENT_ID.to_string())
    }

    /// Mirrors the daemon's `validate_agents`
    /// (`apps/serverside/src/daemon_config.rs`): the resolved default must
    /// be in `agents.list` (when non-empty), and at most one entry may set
    /// `default = true`. The second check runs regardless of list size so
    /// a shared config the daemon would reject at startup fails TUI
    /// bootstrap too, instead of silently resolving the first `default =
    /// true` entry.
    pub fn validate_default_agent_id(&self) -> Result<()> {
        if self
            .agents
            .list
            .iter()
            .filter(|agent| agent.default)
            .count()
            > 1
        {
            bail!("only one agents.list item may set default=true");
        }

        let ids = self.list_agent_ids();
        if ids.is_empty() {
            return Ok(());
        }

        let default_agent_id = self.resolve_default_agent_id();
        if ids.contains(&default_agent_id) {
            return Ok(());
        }

        bail!(
            "default agent id {:?} not in agents.list (available: {:?})",
            default_agent_id,
            ids
        )
    }

    pub fn agent_role_ids(&self) -> Vec<String> {
        let mut ordered = Vec::new();
        let mut seen = BTreeSet::new();

        for configured_role_id in &self.tui.agent_order {
            let configured_role_id = configured_role_id.trim();
            if configured_role_id.is_empty() {
                continue;
            }

            if let Some(role_id) = self
                .agent
                .keys()
                .find(|role_id| role_id.eq_ignore_ascii_case(configured_role_id))
                .cloned()
            {
                if seen.insert(role_id.clone()) {
                    ordered.push(role_id);
                }
            }
        }

        for role_id in self.agent.keys() {
            if seen.insert(role_id.clone()) {
                ordered.push(role_id.clone());
            }
        }

        ordered
    }

    pub fn agent_role(&self, role_id: &str) -> Option<&AgentRoleConfig> {
        self.agent.get(role_id)
    }

    /// Resolve the active agent role's config; `None` if no role is
    /// selected or the role is unknown. Shared by `AppState` and the
    /// turn-config snapshot.
    pub fn active_agent_role_config(&self, active_role: Option<&str>) -> Option<&AgentRoleConfig> {
        active_role.and_then(|role_id| self.agent_role(role_id))
    }

    pub fn resolve_skills_config(&self) -> ResolvedSkillsConfig {
        let mut skills_dirs = Vec::new();

        // Priority 1: Project level (highest)
        skills_dirs.push(PathBuf::from(".xiaoo/skills"));

        // Priority 2: Config file user dirs (medium)
        if let Some(skills) = self.skills.as_ref() {
            if let Some(extra_dirs) = skills.dirs.as_ref() {
                for dir in extra_dirs {
                    let path = PathBuf::from(dir);
                    let dir_str = path.to_string_lossy();
                    if dir_str != ".xiaoo/skills"
                        && !dir_str.ends_with("/.xiaoo/skills")
                        && !dir_str.ends_with("\\.xiaoo\\skills")
                        && dir_str != "/usr/lib/.xiaoo/skills"
                    {
                        skills_dirs.push(path);
                    }
                }
            }
        }

        // Priority 3: User level
        if let Some(home) = dirs::home_dir() {
            skills_dirs.push(home.join(".xiaoo").join("skills"));
        }

        // Priority 4: System level (lowest) - for built-in skills like xiaoo-guardian
        skills_dirs.push(PathBuf::from("/usr/lib/.xiaoo/skills"));

        ResolvedSkillsConfig {
            skills_dirs,
            allow_scripts: self
                .skills
                .as_ref()
                .and_then(|skills| skills.allow_scripts)
                .unwrap_or(false),
            disabled: self
                .skills
                .as_ref()
                .map(|skills| skills.disabled.iter().cloned().collect())
                .unwrap_or_default(),
            max_listing_description_chars: SkillsSection::resolve_listing_limit(
                self.skills.as_ref(),
            ),
            ..ResolvedSkillsConfig::default()
        }
    }

    pub fn build_lsp_registry(&self) -> Option<Arc<LspServiceRegistry>> {
        let lsp = self.lsp.as_ref()?;
        if !lsp.enabled {
            return None;
        }
        let extra = build_extra_server_configs(&lsp.extra_servers);
        Some(Arc::new(LspServiceRegistry::new_with_disabled(
            extra,
            lsp.disabled_servers.clone(),
        )))
    }
}

pub(crate) fn load_merged_mcp_servers(
    toml_servers: &[mcp_support::McpServerConfig],
    explicit_path: Option<&Path>,
    workspace: &Path,
    home: Option<&Path>,
    toml_source: &Path,
) -> Result<Vec<mcp_support::McpServerConfig>, mcp_support::McpConfigError> {
    let json_source = mcp_support::resolve_json_config_path(explicit_path, workspace, home);
    let json_servers = mcp_support::load_json_servers(explicit_path, workspace, home)?;
    let fallback_json_source = workspace.join(".mcp.json");
    mcp_support::merge_server_configs(
        toml_servers.to_vec(),
        json_servers,
        toml_source,
        json_source.as_deref().unwrap_or(&fallback_json_source),
    )
}

pub fn require_tui_bootstrap_config(config: Option<Config>, config_path: &Path) -> Result<Config> {
    let mut config = config
        .ok_or_else(|| anyhow::anyhow!("config file not found: {}", config_path.display()))?;
    install_builtin_agent_roles(&mut config.agent)
        .with_context(|| format!("invalid TUI config {}", config_path.display()))?;

    // Fallback for configs that reached bootstrap without a file load.
    config
        .llm
        .normalize_models()
        .with_context(|| format!("invalid TUI config {}", config_path.display()))?;

    if config.llm.provider.trim().is_empty() {
        bail!(
            "invalid TUI config {}: missing [llm].provider",
            config_path.display()
        );
    }
    if config.llm.model.trim().is_empty() {
        bail!(
            "invalid TUI config {}: missing [llm].model",
            config_path.display()
        );
    }

    if config.llm.max_tokens == 0 {
        bail!(
            "invalid TUI config {}: [llm].max_tokens must be > 0",
            config_path.display()
        );
    }

    config
        .llm
        .request_headers
        .validate_for_provider(&config.llm.provider)?;

    config.validate_default_agent_id().with_context(|| {
        format!(
            "invalid TUI config {}: agents.default_agent_id validation failed",
            config_path.display()
        )
    })?;

    Ok(config)
}

fn install_builtin_agent_roles(agent_roles: &mut BTreeMap<String, AgentRoleConfig>) -> Result<()> {
    if agent_roles.contains_key(PLAN_AGENT_ID) {
        bail!("agent role `{PLAN_AGENT_ID}` is builtin and cannot be overridden in config");
    }

    agent_roles.insert(
        PLAN_AGENT_ID.to_string(),
        AgentRoleConfig {
            description: PLAN_AGENT_DESCRIPTION.to_string(),
            prompt: Some(PLAN_AGENT_PROMPT.to_string()),
            max_turns: None,
            tools: BTreeMap::from([
                ("bash".to_string(), false),
                ("file_edit".to_string(), false),
                ("file_write".to_string(), false),
                ("send_file".to_string(), false),
                ("spawn_subagent".to_string(), false),
            ]),
        },
    );

    Ok(())
}

pub fn save_llm_secret(config_path: &Path, env_name: &str, secret: &str) -> Result<()> {
    xiaoo_shared::llm_secrets::save_llm_secret(config_path, env_name, secret)
}

pub fn init_llm_secrets(config_path: &Path) -> Result<()> {
    xiaoo_shared::llm_secrets::init_llm_secrets(config_path)
}

fn default_llm_max_tokens() -> u32 {
    DEFAULT_LLM_MAX_TOKENS
}

/// Protocol-family static fallback context window: a coarse estimate used
/// only when the runtime dynamic probe fails — the turn path falls back to
/// it, treating it as a plain window value (see
/// `gateway_api::runtime_request::resolve_window_for_turn`). The
/// authoritative window comes from the dynamic probing chain (provider
/// catalog > OpenRouter > Ollama `/api/show`) — see
/// `gateway_api::runtime_request::probe_dynamic_context_window`.
pub fn resolve_context_window(config: &Config) -> Option<usize> {
    xiaoo_api::llm::resolve_protocol_family(&config.llm.provider)
        .map(|family| family.static_context_window())
}

#[cfg(test)]
#[path = "../../../../tests/unit/endside/support/config_test.rs"]
mod tests;
