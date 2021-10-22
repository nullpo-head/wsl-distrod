use std::{io::Write, os::unix::prelude::PermissionsExt, path::Path, process::Command};

use anyhow::{anyhow, bail, Context, Result};
use regex::Regex;
use tempfile::NamedTempFile;

use crate::template::Template;
use libs::wsl_interop;

pub fn enable_autostart_on_windows_boot(distro_name: &str) -> Result<()> {
    let c = wsl_interop::get_wsl_drive_path("C")?.ok_or_else(|| anyhow!("C drive not found."))?;

    let user_name = get_user_name(&c)?;
    let (_task_xml, task_xml_win_path) = generate_task_xml(&user_name, distro_name)?;
    let sched_ps_cont = generate_schedule_posh_command(&user_name, &task_xml_win_path, distro_name);

    let mut powershell =
        Command::new(c.join("Windows/System32/WindowsPowerShell/v1.0/powershell.exe"));
    log::trace!("powershell command:\n{}", &sched_ps_cont);
    powershell.arg("-Command").arg(sched_ps_cont);
    let mut powershell = powershell
        .spawn()
        .with_context(|| "Failed to execute Powershell.")?;
    let status = powershell.wait()?;
    if !status.success() {
        bail!("Powershell failed. {}", status);
    }
    Ok(())
}

pub fn disable_autostart_on_windows_boot(distro_name: &str) -> Result<()> {
    let c = wsl_interop::get_wsl_drive_path("C")?.ok_or_else(|| anyhow!("C drive not found."))?;

    let user_name = get_user_name(&c)?;
    let unsched_ps_cont = generate_unschedule_posh_command(&user_name, distro_name);

    let mut powershell =
        Command::new(c.join("Windows/System32/WindowsPowerShell/v1.0/powershell.exe"));
    log::trace!("powershell command:\n{}", &unsched_ps_cont);
    powershell.arg("-Command").arg(unsched_ps_cont);
    let mut powershell = powershell
        .spawn()
        .with_context(|| "Failed to execute Powershell.")?;
    let status = powershell.wait()?;
    if !status.success() {
        bail!("Powershell failed. {}", status);
    }
    Ok(())
}

fn get_user_name(drive_path: &Path) -> Result<String> {
    let mut whoami = Command::new(drive_path.join("Windows/System32/whoami.exe"));
    let user_name = whoami
        .output()
        .with_context(|| "Failed to execute whoami.exe.")?;
    if !user_name.status.success() {
        log::warn!(
            "whoami.exe had an error: {}",
            String::from_utf8_lossy(&user_name.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&user_name.stdout)
        .trim()
        .to_string())
}

fn generate_task_xml(user_name: &str, distro_name: &str) -> Result<(NamedTempFile, String)> {
    let bytes = include_bytes!("../resources/distrod_autostart.xml");
    let mut task_xml = Template::new(String::from_utf8_lossy(bytes).into_owned());
    task_xml
        .assign("USER_NAME", user_name)
        .assign("DISTRO_NAME", distro_name)
        .assign("TASK_NAME", &format!("StartDistrod_{}", &distro_name));
    let mut task_xml_file = NamedTempFile::new().with_context(|| "Failed to create temp file.")?;

    task_xml_file
        .write_all(task_xml.render().as_bytes())
        .with_context(|| "Failed to make the task xml file.")?;
    let mut perm = task_xml_file.as_file().metadata()?.permissions();
    perm.set_mode(0o644);
    task_xml_file
        .as_file_mut()
        .set_permissions(perm)
        .with_context(|| {
            format!(
                "Failed to set the permission of {:#?}",
                task_xml_file.path()
            )
        })?;

    let mut wslpath = Command::new("/bin/wslpath");
    wslpath.args(&["-w".as_ref(), task_xml_file.path().as_os_str()]);
    let path_to_task_xml = wslpath.output().with_context(|| {
        format!(
            "Failed to execute wslpath -w '{}'",
            task_xml_file.path().to_string_lossy()
        )
    })?;
    if !path_to_task_xml.status.success() {
        bail!(
            "wslpath -w '{}' exited with error. stderr: {}",
            &task_xml_file.path().to_string_lossy(),
            String::from_utf8_lossy(&path_to_task_xml.stderr)
        );
    }
    let task_xml_win_path = String::from_utf8_lossy(&path_to_task_xml.stdout)
        .trim()
        .to_owned();

    Ok((task_xml_file, task_xml_win_path))
}

fn generate_schedule_posh_command(
    user_name: &str,
    task_file_path: &str,
    distro_name: &str,
) -> String {
    let bytes = include_bytes!("../resources/schedule_autostart_task.ps1");
    let mut sched_ps = Template::new(String::from_utf8_lossy(bytes).into_owned());
    sched_ps
        .assign("USER_NAME", user_name)
        .assign("TASK_FILE_WINDOWS_PATH", task_file_path)
        .assign("TASK_NAME", &get_schedule_task_name(user_name, distro_name));
    sched_ps.render()
}

fn generate_unschedule_posh_command(user_name: &str, distro_name: &str) -> String {
    let bytes = include_bytes!("../resources/unschedule_autostart_task.ps1");
    let mut sched_ps = Template::new(String::from_utf8_lossy(bytes).into_owned());
    sched_ps.assign("TASK_NAME", &get_schedule_task_name(user_name, distro_name));
    sched_ps.render()
}

fn get_schedule_task_name(user_name: &str, distro_name: &str) -> String {
    let nonlatin = Regex::new("[^a-zA-Z0-9]").unwrap();
    let user_name = nonlatin.replace_all(user_name, "-");
    format!("StartWSL_{}_for_{}", distro_name, user_name)
}
