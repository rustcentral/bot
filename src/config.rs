use std::{
    env,
    path::{Path, PathBuf},
};

use anyhow::Context;
use serde::Deserialize;

use crate::ai_channel;
pub(crate) mod file_watch;

#[derive(Debug, Deserialize)]
pub struct Configuration {
    /// The bot's discord token.
    pub token: String,
    #[serde(default, rename = "ai_channel")]
    pub ai_channels: Vec<ai_channel::Configuration>,
}

impl Configuration {
    /// Read the configuration from the specified location.
    ///
    /// Each path is a layer: values set in later entries override the values set by earlier ones.
    pub fn read<'a>(locations: impl IntoIterator<Item = &'a Path>) -> anyhow::Result<Self> {
        let mut settings = config::Config::builder();
        for location in locations {
            settings = settings.add_source(config::File::from(location).required(false));
        }

        let config = settings
            .add_source(config::Environment::with_prefix("BOT"))
            .build()
            .context("failed to build config")?;

        config
            .try_deserialize()
            .context("failed to deserialize config")
    }

    /// Reads the configuration from the locations specified in the environment variable. The paths
    /// in `default` are used if the variable is not set.
    ///
    /// See also: [Configuration::read].
    pub fn read_with_env<'a>(
        env_var: &str,
        default: impl IntoIterator<Item = &'a Path>,
    ) -> anyhow::Result<Self> {
        match env::var(env_var) {
            Ok(paths) => {
                let paths = paths.split(',').map(PathBuf::from).collect::<Vec<_>>();
                Self::read(paths.iter().map(|p| p.as_path()))
            }
            Err(_) => Self::read(default),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ensures that the example config file is valid.
    #[test]
    fn example_config_valid() {
        // Using a temp dir to avoid have to create a build script to copy the example toml into ./target
        let temp_dir = tempfile::tempdir().expect("Unable to create tempoary directory");

        let mut example_toml = temp_dir.path().to_path_buf();
        example_toml.push("example_bot.toml");
        std::fs::write(&example_toml, include_str!("../example_bot.toml"))
            .expect("Unable to write example data to temp file");

        Configuration::read([example_toml.as_path()])
            .expect("Unable to parse example configuration file.");
    }
}
