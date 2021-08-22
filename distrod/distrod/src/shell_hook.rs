use std::{
    collections::HashSet,
    io::{BufRead, BufReader, BufWriter, Seek, SeekFrom, Write},
};

use anyhow::{Context, Result};

use crate::passwd::{Passwd, PasswdFile};
use libs::command_alias::CommandAlias;

pub fn enable_default_shell_hook() -> Result<()> {
    let mut shells = HashSet::new();
    let mut passwd_file = PasswdFile::open("/etc/passwd")?;
    passwd_file.update(&mut |passwd| {
        if CommandAlias::is_alias(passwd.shell) {
            return Ok(None);
        }
        let alias = CommandAlias::open_from_source(passwd.shell, true)?
            .expect("an alias should be created.");
        let mut new_passwd = Passwd::from_view(passwd);
        let shell = alias.get_link_path().to_string_lossy().to_string();
        shells.insert(shell.clone());
        new_passwd.shell = shell;
        Ok(Some(new_passwd))
    })?;
    if let Err(e) = register_shells_to_system(shells) {
        log::warn!("Failed to register shells to system. {}", e);
    }
    Ok(())
}

pub fn disable_default_shell_hook() -> Result<()> {
    let mut passwd_file = PasswdFile::open("/etc/passwd")?;
    passwd_file.update(&mut |passwd| {
        if !CommandAlias::is_alias(passwd.shell) {
            return Ok(None);
        }
        let alias = CommandAlias::open_from_link(passwd.shell)?;
        let mut new_passwd = Passwd::from_view(passwd);
        new_passwd.shell = alias.get_source_path().to_string_lossy().to_string();
        Ok(Some(new_passwd))
    })?;
    Ok(())
}

fn register_shells_to_system(mut shell_paths: HashSet<String>) -> Result<()> {
    {
        let mut open_opts = std::fs::OpenOptions::new();
        open_opts.read(true).write(false).create(false);
        let shells = open_opts
            .open("/etc/shells")
            .with_context(|| "Failed to open /etc/shells")?;
        let shells = BufReader::new(shells);
        for line in shells.lines() {
            let line = line?;
            if shell_paths.contains(&line) {
                shell_paths.remove(&line);
            }
        }
    }
    let mut open_opts = std::fs::OpenOptions::new();
    open_opts.read(true).append(true).create(false);
    let shells = open_opts
        .open("/etc/shells")
        .with_context(|| "Failed to open /etc/shells to write")?;
    let mut shells = BufWriter::new(shells);
    shells
        .seek(SeekFrom::End(0))
        .with_context(|| "Failed to seek to end")?;
    for shell_path in shell_paths {
        if shell_path.ends_with("/nologin") || shell_path.ends_with("/false") {
            continue;
        }
        shells
            .write(format!("{}\n", &shell_path).as_bytes())
            .with_context(|| format!("Failed to write '{}' to /etc/shells.", &shell_path))?;
    }
    Ok(())
}
