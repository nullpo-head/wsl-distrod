use anyhow::{bail, Context, Result};
use once_cell::sync::Lazy;
use std::io::Read;
#[cfg(target_os = "linux")]
use std::os::linux::fs::MetadataExt;
use std::{
    fs::File,
    path::{Path, PathBuf},
};

use serde::Deserialize;

#[derive(Deserialize)]
pub struct DistrodConfig {
    distrod: DistrodGlobalConfig,
}

#[derive(Deserialize)]
pub struct DistrodGlobalConfig {
    default_distro_image: Option<PathBuf>,
    distro_images_dir: PathBuf,
}

static DISTROD_CONFIG_ROOT_DIR: &str = "/opt/distrod";

static DISTROD_CONFIG: Lazy<Result<DistrodConfig>> =
    Lazy::new(|| read_distrod_config().with_context(|| "Failed to read the Distrod config file."));

impl DistrodConfig {
    pub fn get() -> Result<&'static DistrodConfig> {
        match DISTROD_CONFIG.as_ref() {
            Ok(cfg) => Ok(cfg),
            Err(e) => bail!("Failed to get the Distrod config. {:?}", e),
        }
    }
}

static DISTROD_ALIAS_DIR: Lazy<String> =
    Lazy::new(|| format!("{}/{}", DISTROD_CONFIG_ROOT_DIR, "alias"));

pub fn get_alias_dir() -> &'static str {
    DISTROD_ALIAS_DIR.as_str()
}

static DISTROD_BIN_PATH: Lazy<String> =
    Lazy::new(|| format!("{}/{}", DISTROD_CONFIG_ROOT_DIR, "distrod"));

pub fn get_distrod_bin_path() -> &'static str {
    DISTROD_BIN_PATH.as_str()
}

static DISTROD_EXEC_BIN_PATH: Lazy<String> =
    Lazy::new(|| format!("{}/{}", DISTROD_CONFIG_ROOT_DIR, "distrod-exec"));

pub fn get_distrod_exec_bin_path() -> &'static str {
    DISTROD_EXEC_BIN_PATH.as_str()
}

#[cfg(target_os = "linux")]
fn read_distrod_config() -> Result<DistrodConfig> {
    let config_path = Path::new(DISTROD_CONFIG_ROOT_DIR).join("distrod.toml");
    let mut config_file = File::open(&config_path).with_context(|| {
        format!(
            "Failed to open the distrod config file: '{:?}'.",
            &config_path
        )
    })?;
    let metadata = config_file
        .metadata()
        .with_context(|| "Failed to get the permision of the metadata of the config.")?;
    if metadata.st_uid() != 0 || metadata.st_gid() != 0 {
        bail!("The distrod config file is not owned by root.");
    }

    let mut config_cont = String::new();
    config_file.read_to_string(&mut config_cont)?;

    toml::from_str(&config_cont).with_context(|| {
        format!(
            "Failed to parse the config file. Invalid format? '{:?}'.",
            &config_path
        )
    })
}

#[cfg(target_os = "windows")]
fn read_distrod_config() -> Result<DistrodConfig> {
    bail!("read_distrod_config function should not be called on Windows side.");
}
