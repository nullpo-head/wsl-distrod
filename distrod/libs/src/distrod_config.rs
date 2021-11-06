use anyhow::{bail, Context, Result};
use once_cell::sync::Lazy;
#[cfg(target_os = "linux")]
use std::io::Read;
use std::io::{BufWriter, Write};
#[cfg(target_os = "linux")]
use std::os::linux::fs::MetadataExt;
use std::sync::{Arc, RwLock};
use std::{
    fs::File,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DistrodConfig {
    pub distrod: DistrodGlobalConfig,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DistrodGlobalConfig {
    pub default_distro_image: PathBuf,
    pub distro_images_dir: PathBuf,
    pub log_level: Option<String>,
    pub kmsg_log_level: Option<String>,
}

static DISTROD_ROOT_DIR: &str = "/opt/distrod";

static DISTROD_CONFIG: Lazy<Result<RwLock<Arc<DistrodConfig>>>> = Lazy::new(|| {
    Ok(RwLock::new(Arc::new(read_distrod_config().with_context(
        || "Failed to read the Distrod config file.",
    )?)))
});

impl DistrodConfig {
    pub fn get() -> Result<Arc<DistrodConfig>> {
        match DISTROD_CONFIG.as_ref() {
            Ok(cfg) => {
                let r = cfg.read();
                if let Err(e) = r {
                    bail!("Failed to acquire the read lock of the config. {:?}", e);
                }
                Ok(r.unwrap().clone())
            }
            Err(e) => bail!("Failed to get the Distrod config. {:?}", e),
        }
    }

    pub fn update(self) -> Result<()> {
        write_distrod_config(&self).with_context(|| "Failed to save the new config.")?;
        match DISTROD_CONFIG.as_ref() {
            Ok(cfg) => {
                let w = cfg.write();
                if let Err(e) = w {
                    bail!("Failed to acquire the write lock of the config. {:?}", e);
                }
                *w.unwrap() = Arc::new(self);
                Ok(())
            }
            Err(e) => bail!("Failed to get the Distrod config. {:?}", e),
        }
    }
}

static DISTROD_ALIAS_DIR: Lazy<String> = Lazy::new(|| format!("{}/{}", DISTROD_ROOT_DIR, "alias"));

/// The directory where the alias commands are stored.
pub fn get_alias_dir() -> &'static str {
    DISTROD_ALIAS_DIR.as_str()
}

static DISTROD_BIN_DIR: Lazy<String> = Lazy::new(|| format!("{}/{}", DISTROD_ROOT_DIR, "bin"));

/// The path to the distrod binary.
pub fn get_distrod_bin_dir_path() -> &'static str {
    DISTROD_BIN_DIR.as_str()
}

static DISTROD_BIN_PATH: Lazy<String> =
    Lazy::new(|| format!("{}/{}", DISTROD_BIN_DIR.as_str(), "distrod"));

/// The path to the distrod binary.
pub fn get_distrod_bin_path() -> &'static str {
    DISTROD_BIN_PATH.as_str()
}

static DISTROD_EXEC_BIN_PATH: Lazy<String> =
    Lazy::new(|| format!("{}/{}", DISTROD_BIN_DIR.as_str(), "distrod-exec"));

/// The path to the distrod-exec binary.
pub fn get_distrod_exec_bin_path() -> &'static str {
    DISTROD_EXEC_BIN_PATH.as_str()
}

static DISTROD_RUN_OVERLAY_DIR_PAH: Lazy<String> =
    Lazy::new(|| format!("{}/{}", DISTROD_ROOT_DIR, "run"));

/// The path to the directory where the static files to be located on /run directory
pub fn get_distrod_run_overlay_dir() -> &'static str {
    DISTROD_RUN_OVERLAY_DIR_PAH.as_str()
}

static DISTROD_CONF_DIR_PAH: Lazy<String> =
    Lazy::new(|| format!("{}/{}", DISTROD_ROOT_DIR, "conf"));

/// The path to the directory where the configuration files are stored.
/// Configurations files are modified by users. Thus, the files stroed
/// in this directory should not be overwritten by the update and should be
/// backwork-compatible.
pub fn get_distrod_conf_dir() -> &'static str {
    DISTROD_CONF_DIR_PAH.as_str()
}

#[cfg(target_os = "linux")]
fn read_distrod_config() -> Result<DistrodConfig> {
    let config_path = Path::new(&*DISTROD_CONF_DIR_PAH).join("distrod.toml");
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

// This should be defined in Windows as well to make it compilable.
#[cfg(target_os = "windows")]
fn read_distrod_config() -> Result<DistrodConfig> {
    bail!("read_distrod_config function should not be called on Windows side.");
}

fn write_distrod_config(config: &DistrodConfig) -> Result<()> {
    let config_path = Path::new(&*DISTROD_CONF_DIR_PAH).join("distrod.toml");
    let mut config_file = BufWriter::new(File::create(&config_path).with_context(|| {
        format!(
            "Failed to open the distrod config file: '{:?}'.",
            &config_path
        )
    })?);

    config_file
        .write_all(&toml::to_vec(config).with_context(|| "Failed to serialize the new config.")?)
        .with_context(|| format!("Failed to write the config to '{:?}'.", config_path))?;
    Ok(())
}
