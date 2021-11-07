use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    path::PathBuf,
};

use anyhow::{anyhow, bail, Context, Result};
use procfs::process;

use crate::{envfile::PathVariable, mount_info::get_mount_entries};

pub fn get_wsl_drive_path(drive_letter: &str) -> Result<Option<PathBuf>> {
    let entries = get_mount_entries().with_context(|| "Failed to get the mount entries.")?;
    Ok(entries.into_iter().find_map(|e| {
        if e.fstype != "9p" {
            return None;
        }
        // Windows 10
        if e.source
            .starts_with(&format!("{}:\\", drive_letter.to_uppercase()))
        {
            return Some(e.path);
        }
        // Windows 11
        if e.source == "drvfs"
            && e.attributes
                .contains(&format!("path={}:\\", drive_letter.to_uppercase()))
        {
            return Some(e.path);
        }
        None
    }))
}

fn get_wsl_drive_mount_point() -> Result<Option<PathBuf>> {
    let c_drive = get_wsl_drive_path("c")
        .with_context(|| "Failed to get the path where C drive is mounted.")?;
    if c_drive.is_none() {
        return Ok(None);
    }
    let c_drive = c_drive.unwrap();
    Ok(Some(
        c_drive
            .parent()
            .map_or_else(|| PathBuf::from("/"), |p| p.to_owned()),
    ))
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

pub fn collect_wsl_paths() -> Result<Vec<String>> {
    let wsl_mount_point =
        get_wsl_drive_mount_point().with_context(|| "Failed to get the WSL drive mount point.")?;
    if wsl_mount_point.is_none() {
        return Ok(vec![]);
    }
    let wsl_mount_point = wsl_mount_point.unwrap();
    let wsl_mount_point = wsl_mount_point.to_string_lossy();

    let path = std::env::var("PATH")?;
    let path = PathVariable::parse(&path);
    let wsl_paths = path
        .iter()
        .filter(|path| path.starts_with(wsl_mount_point.as_ref()))
        .map(|p| p.to_owned())
        .collect();
    Ok(wsl_paths)
}
