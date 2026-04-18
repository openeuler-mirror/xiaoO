//! Cerberus CLI - Secure command execution with policy enforcement.

use cerberus_cli::app::commands::{Commands, ProfileCommands};
use cerberus_cli::app::error::{blocked_reason, CliError};
use cerberus_cli::app::host::create_adapter;
use cerberus_cli::app::profile::resolve_profile_with_source;
use cerberus_cli::app::render::{render_history, render_result};
use cerberus_cli::app::storage::Storage;
use cerberus_cli::profiles::{
    resolve_policy_file, PolicySource, BUILTIN_PROFILE_WORKSPACE_WRITE_NETWORK_ON_DEV_ENV,
};
use cerberus_core::sandbox::{check_strict_policy_enforceable, detect_capabilities};
use cerberus_core::{execute, ExecRequest, FsPermission, FsRule, Policy};
use clap::Parser;
use std::path::PathBuf;
use std::time::Instant;

const DEFAULT_PROFILE: &str = BUILTIN_PROFILE_WORKSPACE_WRITE_NETWORK_ON_DEV_ENV;

/// Cerberus - Secure command execution with policy enforcement.
#[derive(Debug, Parser)]
#[command(name = "cerberus", version, about, long_about = None)]
struct Cli {
    /// Execution profile to use (built-in or discovered from config/).
    #[arg(
        short,
        long,
        global = true,
        default_value = DEFAULT_PROFILE,
        conflicts_with = "policy_file"
    )]
    profile: String,

    /// Load policy from an explicit TOML file (overrides --profile).
    #[arg(long, global = true, value_name = "PATH", conflicts_with = "profile")]
    policy_file: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

fn main() {
    if let Err(error) = run() {
        render_cli_error(&error);
        std::process::exit(1);
    }
}

fn run() -> Result<(), CliError> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Exec { args } => {
            let (policy, _) = resolve_exec_policy(&cli)?;
            exec_command(args.clone(), &policy)
        }
        Commands::History { limit, since } => show_history(*limit, since.as_deref()),
        Commands::Profile { command } => handle_profile(command),
        Commands::Init {
            claude,
            codex,
            opencode,
            show,
            uninstall,
            force,
        } => handle_init(*claude, *codex, *opencode, *show, *uninstall, *force),
    }
}

fn render_cli_error(error: &CliError) {
    match error {
        CliError::ExecutionError(error) => match blocked_reason(error) {
            Some(reason) => eprintln!("{}", reason),
            None => eprintln!("Error: {}", error),
        },
        _ => eprintln!("Error: {}", error),
    }
}

fn resolve_policy(cli: &Cli) -> Result<(Policy, PolicySource), CliError> {
    if let Some(ref path) = cli.policy_file {
        let policy = resolve_policy_file(path)
            .map_err(|e| CliError::ConfigError(format!("Failed to load policy file: {}", e)))?;
        let source = PolicySource::File {
            name: path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("custom")
                .to_string(),
            path: path.clone(),
        };
        return Ok((policy, source));
    }

    resolve_profile_with_source(&cli.profile)
}

fn resolve_exec_policy(cli: &Cli) -> Result<(Policy, PolicySource), CliError> {
    let (policy, source) = resolve_policy(cli)?;
    let policy = inject_workspace_access(policy, &source)?;
    Ok((policy, source))
}

fn inject_workspace_access(mut policy: Policy, source: &PolicySource) -> Result<Policy, CliError> {
    if !source.is_builtin() {
        return Ok(policy);
    }

    let workspace = std::env::current_dir()?;
    let has_workspace_rule = policy
        .custom_paths
        .iter()
        .any(|rule| rule.path == workspace);
    if has_workspace_rule {
        return Ok(policy);
    }

    policy.custom_paths.push(FsRule {
        path: workspace,
        permission: FsPermission::ReadWrite,
    });
    Ok(policy)
}

fn exec_command(args: Vec<String>, policy: &Policy) -> Result<(), CliError> {
    if args.is_empty() {
        eprintln!("Error: No command specified");
        std::process::exit(1);
    }

    let mut request = ExecRequest::new(&args[0]);
    for arg in &args[1..] {
        request = request.arg(arg);
    }

    let start = Instant::now();
    let result = execute(request, policy)?;
    let duration = start.elapsed();

    let storage = Storage::open_default()?;
    let cmd_str = args.join(" ");
    storage.record_execution(&cmd_str, &result, duration)?;

    print!("{}", render_result(&result));

    std::process::exit(result.exit_code);
}

fn show_history(limit: usize, since: Option<&str>) -> Result<(), CliError> {
    let storage = Storage::open_default()?;
    let records = storage.get_records(since, Some(limit))?;

    if records.is_empty() {
        println!("No execution history found.");
    } else {
        print!("{}", render_history(&records));
    }

    Ok(())
}

fn handle_profile(command: &ProfileCommands) -> Result<(), CliError> {
    match command {
        ProfileCommands::List => {
            println!("Available profiles:\n");
            let profiles = cerberus_cli::app::profile::discover_policies_with_sources();
            for (name, source) in profiles {
                match source {
                    PolicySource::BuiltIn { .. } => {
                        println!("  {} [built-in]", name);
                    }
                    PolicySource::File { path, .. } => {
                        println!("  {} [file: {}]", name, path.display());
                    }
                }
            }
        }
        ProfileCommands::Show { name } => {
            let (policy, source) = cerberus_cli::app::profile::resolve_profile_with_source(name)?;
            let policy = inject_workspace_access(policy, &source)?;

            println!("Profile: {}", source.name());
            println!();

            match source {
                PolicySource::BuiltIn { .. } => {
                    println!("Source: built-in");
                }
                PolicySource::File { path, .. } => {
                    println!("Source: file ({})", path.display());
                }
            }

            let caps = detect_capabilities();
            println!();
            println!("Sandbox Capabilities:");
            println!("  landlock:        {}", caps.landlock);
            println!("  seccomp:         {}", caps.seccomp);
            println!("  namespaces:");
            println!("    mount:         {}", caps.namespaces.mount);
            println!("    pid:           {}", caps.namespaces.pid);
            println!("    network:       {}", caps.namespaces.network);
            println!("    user:          {}", caps.namespaces.user);
            println!("  mount_isolation: {}", caps.mount_isolation);

            let enforcement_result = check_strict_policy_enforceable(&policy);
            let enforcement_level = enforcement_result.unwrap_or_else(|e| {
                eprintln!("\nWarning: Policy cannot be fully enforced: {}", e);
                cerberus_core::sandbox::EnforcementLevel::Unsupported
            });

            println!();
            println!("Enforcement Level: {}", enforcement_level);
            match enforcement_level {
                cerberus_core::sandbox::EnforcementLevel::Enforced => {
                    println!("  All requested isolation features are available.");
                }
                cerberus_core::sandbox::EnforcementLevel::Degraded => {
                    println!(
                        "  Some requested isolation features are unavailable or only partially enforced."
                    );
                }
                cerberus_core::sandbox::EnforcementLevel::Unsupported => {
                    println!("  Requested isolation cannot be fully enforced by this runtime.");
                    if !policy.landlock_optional && !policy.mount_isolation_fallback {
                        println!("  Policy will FAIL CLOSED on execution.");
                    } else {
                        println!(
                            "  Landlock fallback flags are enabled, but they only apply to Landlock and mount-isolation downgrade paths."
                        );
                    }
                }
            }

            println!();
            println!("Policy Fallback Flags:");
            println!("  landlock_optional:         {}", policy.landlock_optional);
            println!(
                "  mount_isolation_fallback:  {}",
                policy.mount_isolation_fallback
            );

            println!();
            println!("Namespaces:");
            println!("  mount:   {}", policy.namespaces.mount);
            println!("  pid:     {}", policy.namespaces.pid);
            println!(
                "  network: {} ({})",
                policy.namespaces.network,
                if policy.allow_network() {
                    "network allowed"
                } else {
                    "network blocked"
                }
            );
            println!("  user:    {}", policy.namespaces.user);

            println!();
            println!("Resources:");
            println!("  timeout: {}s", policy.resources.timeout_secs);
            if let Some(mem) = policy.resources.max_memory_bytes {
                println!("  max memory: {} MB", mem / 1024 / 1024);
            } else {
                println!("  max memory: unlimited");
            }
            if let Some(procs) = policy.resources.max_processes {
                println!("  max processes: {}", procs);
            } else {
                println!("  max processes: unlimited");
            }

            println!();
            println!("Filesystem path groups:");
            println!("  system_binaries:  {}", policy.path_groups.system_binaries);
            println!(
                "  system_libraries: {}",
                policy.path_groups.system_libraries
            );
            println!(
                "  temp_directories: {}",
                policy.path_groups.temp_directories
            );
            println!("  device_files:     {}", policy.path_groups.device_files);
            println!("  proc_filesystem:  {}", policy.path_groups.proc_filesystem);
            println!("  wsl_paths:        {}", policy.path_groups.wsl_paths);

            if !policy.custom_paths.is_empty() {
                println!();
                println!("Custom paths:");
                for rule in &policy.custom_paths {
                    let perm = match &rule.permission {
                        cerberus_core::FsPermission::ReadOnly => "readonly",
                        cerberus_core::FsPermission::ReadWrite => "readwrite",
                        cerberus_core::FsPermission::ReadExecute => "readexecute",
                        cerberus_core::FsPermission::ReadWriteExecute => "readwriteexecute",
                    };
                    println!("  {} [{}]", rule.path.display(), perm);
                }
            }

            println!();
            println!("Environment whitelist:");
            if policy.environment.whitelist.is_empty() {
                println!("  (none)");
            } else {
                for var in &policy.environment.whitelist {
                    println!("  {}", var);
                }
            }

            if let Some(ref net_policy) = policy.network_policy {
                println!();
                println!("Network policy:");
                println!("  enabled: {}", net_policy.enabled);
                if let Some(ref mode) = net_policy.mode {
                    println!("  mode: {:?}", mode);
                }
                if let Some(ref action) = net_policy.default_action {
                    println!("  default_action: {:?}", action);
                }
                if !net_policy.rules.is_empty() {
                    println!("  rules: {} defined", net_policy.rules.len());
                }
                if net_policy.enabled && !policy.allow_network() {
                    println!(
                        "  status: invalid with this profile, network_policy requires namespaces.network = true"
                    );
                } else if net_policy.enabled {
                    println!(
                        "  status: build-dependent; default builds fail closed without the eBPF backend, while ebpf-enabled builds use a real monitor/enforce path"
                    );
                } else if !policy.allow_network() {
                    println!(
                        "  status: defined but inactive; enable it only with namespaces.network = true and an available eBPF backend"
                    );
                } else {
                    println!(
                        "  status: defined but inactive, disabled network_policy is stored and shown but not enforced"
                    );
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cerberus_core::Policy;

    #[test]
    fn default_profile_is_workspace_write_network_on_dev_env() {
        let cli = Cli::parse_from(["cerberus", "exec", "--", "ls"]);
        assert_eq!(cli.profile, DEFAULT_PROFILE);
    }

    #[test]
    fn inject_workspace_access_adds_current_dir_for_builtin_profiles() {
        let policy = Policy::minimal();
        let source = PolicySource::BuiltIn {
            name: cerberus_cli::profiles::BUILTIN_PROFILE_WORKSPACE_WRITE_NETWORK_ON.to_string(),
        };

        let injected = inject_workspace_access(policy, &source).expect("workspace injection");
        let cwd = std::env::current_dir().expect("cwd");

        let workspace_rule = injected
            .custom_paths
            .iter()
            .find(|rule| rule.path == cwd)
            .expect("workspace rule should be present");
        assert_eq!(workspace_rule.permission, FsPermission::ReadWrite);
    }

    #[test]
    fn inject_workspace_access_skips_file_backed_policies() {
        let policy = Policy::minimal();
        let source = PolicySource::File {
            name: "custom".to_string(),
            path: PathBuf::from("/tmp/custom.toml"),
        };

        let injected =
            inject_workspace_access(policy.clone(), &source).expect("workspace injection");
        assert_eq!(injected.custom_paths.len(), policy.custom_paths.len());
    }
}

fn handle_init(
    claude: bool,
    codex: bool,
    opencode: bool,
    show: bool,
    uninstall: bool,
    force: bool,
) -> Result<(), CliError> {
    let host = if claude {
        cerberus_cli::app::host::Host::Claude
    } else if codex {
        cerberus_cli::app::host::Host::Codex
    } else if opencode {
        cerberus_cli::app::host::Host::OpenCode
    } else {
        eprintln!("Error: Must specify exactly one host: --claude, --codex, or --opencode");
        std::process::exit(1);
    };

    let adapter = create_adapter(host);
    let host_name = format!("{:?}", host);

    if show {
        let status = adapter.show(None);
        println!("=== {} Integration Status ===\n", host_name);
        println!(
            "Host installed: {}",
            if status.host_installed { "Yes" } else { "No" }
        );
        println!(
            "Integration installed: {}",
            if status.integration_installed {
                "Yes"
            } else {
                "No"
            }
        );
        println!();

        if !status.files.is_empty() {
            println!("Files:");
            for file in &status.files {
                let status_str = if file.exists { "exists" } else { "missing" };
                let owned_str = if file.cerberus_owned {
                    " (Cerberus)"
                } else {
                    ""
                };
                println!("  {} [{}{}]", file.path.display(), status_str, owned_str);
            }
            println!();
        }

        if !status.messages.is_empty() {
            for msg in &status.messages {
                println!("  {}", msg);
            }
        }

        return Ok(());
    }

    if uninstall {
        println!("Uninstalling {} integration...", host_name);
        let actions = adapter.uninstall(None)?;
        for action in &actions {
            println!("  {}", action);
        }
        println!("Done.");
        return Ok(());
    }

    println!("Installing {} integration...", host_name);
    let actions = adapter.install(force, None)?;
    for action in &actions {
        println!("  {}", action);
    }

    let status = adapter.show(None);
    if !status.host_installed {
        println!("\nNote: {} binary not found in PATH.", host_name);
        println!("      Install {} first for full integration.", host_name);
    }

    println!("\nDone.");
    Ok(())
}
