mod server;

use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};

use anyhow::{bail, ensure, Result};
use serde::Deserialize;
use xiaoo_shared::gateway::controlled_session::ControlledModelProfile;

use crate::build_identity;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    default_model: String,
    models: BTreeMap<String, ControlledModelProfile>,
}

impl Config {
    fn load(path: PathBuf) -> Result<Self> {
        let contents = std::fs::read_to_string(path)
            .map_err(|_| anyhow::anyhow!("cannot read ACP configuration"))?;
        // TOML diagnostics include input contents; do not print them.
        let config: Self =
            toml::from_str(&contents).map_err(|_| anyhow::anyhow!("invalid ACP configuration"))?;
        ensure!(
            config.models.contains_key(&config.default_model),
            "default_model must name a configured profile"
        );
        for (id, profile) in &config.models {
            ensure!(
                !id.is_empty()
                    && id
                        .bytes()
                        .all(|ch| ch.is_ascii_alphanumeric() || b"-_.".contains(&ch)),
                "invalid model profile identifier"
            );
            profile.validate()?;
        }
        Ok(config)
    }
}

pub(crate) async fn run(mut args: impl Iterator<Item = OsString>) -> Result<()> {
    match (
        args.next().as_deref().and_then(|arg| arg.to_str()),
        args.next(),
        args.next(),
    ) {
        (Some("--version"), None, None) => build_identity::print_version(false),
        (Some("--version"), Some(verbose), None) if verbose == "--verbose" => {
            build_identity::print_version(true);
        }
        (Some("--help"), None, None) => {
            println!("Usage: xiaoo acp --config FILE | --version [--verbose]");
        }
        (Some("--config"), Some(path), None) => {
            server::serve(Config::load(PathBuf::from(path))?).await?;
        }
        _ => bail!("usage: xiaoo acp --config FILE | --version [--verbose]"),
    }
    Ok(())
}
