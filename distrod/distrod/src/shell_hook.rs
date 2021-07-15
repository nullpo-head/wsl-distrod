use anyhow::Result;

use crate::command_alias::CommandAlias;
use crate::passwd::{Passwd, PasswdFile};

pub fn enable_default_shell_hook() -> Result<()> {
    let mut passwd_file = PasswdFile::open()?;
    passwd_file.update(|passwd| {
        if CommandAlias::is_alias(passwd.shell) {
            return None;
        }
        let alias = CommandAlias::open_from_source(passwd.shell, true)
            .ok()?
            .expect("an alias should be created.");
        let mut new_passwd = Passwd::from_view(passwd);
        new_passwd.shell = alias.get_link_path().to_string_lossy().to_string();
        Some(new_passwd)
    })?;
    Ok(())
}

pub fn disable_default_shell_hook() -> Result<()> {
    let mut passwd_file = PasswdFile::open()?;
    passwd_file.update(|passwd| {
        if !CommandAlias::is_alias(passwd.shell) {
            return None;
        }
        let alias = CommandAlias::open_from_link(passwd.shell).ok()?;
        let mut new_passwd = Passwd::from_view(passwd);
        new_passwd.shell = alias.get_source_path().to_string_lossy().to_string();
        Some(new_passwd)
    })?;
    Ok(())
}
