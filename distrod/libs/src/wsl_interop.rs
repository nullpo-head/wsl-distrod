use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    path::PathBuf,
};

use anyhow::{anyhow, bail, Context, Result};
use procfs::process;

use crate::mount_info::get_mount_entries;

pub fn get_wsl_drive_path(drive_letter: &str) -> Result<Option<PathBuf>> {
    let entries = get_mount_entries().with_context(|| "Failed to get the mount entries.")?;
    Ok(entries.into_iter().find_map(|e| {
        if e.fstype == "9p"
            && e.source
                .starts_with(&format!("{}:\\", drive_letter.to_uppercase()))
        {
            Some(e.path)
        } else {
            None
        }
    }))
}

pub fn get_distro_name() -> Result<String> {
    let envs = collect_wsl_env_vars().with_context(|| "Failed to collect wsl envs.")?;
    Ok(envs
        .get(&OsString::from("WSL_DISTRO_NAME"))
        .ok_or_else(|| anyhow!("Failed to get distro name."))?
        .to_string_lossy()
        .to_string())
}

pub fn collect_wsl_env_vars() -> Result<HashMap<OsString, OsString>> {
    let mut wsl_env_names = HashSet::new();
    wsl_env_names.insert(OsString::from("WSLENV"));
    wsl_env_names.insert(OsString::from("WSL_DISTRO_NAME"));
    wsl_env_names.insert(OsString::from("WSL_INTEROP"));

    // The WSL env vars may not be set if the process is launched by sudo.
    // Traverse the parent process until we find the WSL env vars in that case.
    let mut proc = process::Process::myself()
        .with_context(|| "Failed to get Process struct for the current process")?;
    loop {
        let env = proc.environ()?;
        let wsl_envs: HashMap<_, _> = env
            .into_iter()
            .filter(|(name, _)| wsl_env_names.contains(name))
            .collect();
        if !wsl_envs.is_empty() {
            return Ok(wsl_envs);
        }

        if proc.pid == 1 {
            break;
        }
        let stat = proc
            .stat()
            .with_context(|| format!("Failed to get Process::Stat struct of {}", proc.pid))?;
        proc = process::Process::new(stat.ppid).with_context(|| {
            format!(
                "Failed to get Process struct for the parent process {}",
                stat.ppid
            )
        })?;
    }

    bail!("Couldn't find WSL envs");
}
