use std::{
    ffi::{OsStr, OsString},
    path::Path,
};

use anyhow::{anyhow, Context, Result};
use windows::{
    runtime::IntoParam,
    Win32::{Foundation::PWSTR, System::SubsystemForLinux::*},
};

pub unsafe fn is_distribution_registered<'a, Param0: IntoParam<'a, PWSTR>>(
    distributionname: Param0,
) -> bool {
    WslIsDistributionRegistered(distributionname).as_bool()
}

pub unsafe fn register_distribution<'a, Param0, Path0>(
    distributionname: Param0,
    targzfilename: Path0,
) -> Result<()>
where
    Param0: IntoParam<'a, PWSTR> + std::fmt::Debug,
    Path0: AsRef<Path>,
{
    let err = format!(
        "WslRegisterDistribution failed. distro: {:?}, path: {:?}",
        &distributionname,
        targzfilename.as_ref()
    );
    let path = targzfilename.as_ref().as_os_str();
    WslRegisterDistribution(distributionname, path).with_context(|| err)
}

pub unsafe fn set_distribution_default_user<'a, Param0: IntoParam<'a, PWSTR> + std::fmt::Debug>(
    distributionname: Param0,
    defaultuid: u32,
) -> Result<()> {
    let err = format!(
        "WslConfigureDistribution failed. distribution_name: {:?} uid: {}",
        distributionname, defaultuid
    );
    let default_distro_flag = WSL_DISTRIBUTION_FLAGS_ENABLE_INTEROP
        | WSL_DISTRIBUTION_FLAGS_APPEND_NT_PATH
        | WSL_DISTRIBUTION_FLAGS_ENABLE_DRIVE_MOUNTING;
    WslConfigureDistribution(distributionname, defaultuid, default_distro_flag).with_context(|| err)
}

#[derive(Debug)]
pub struct WslCommand {
    distribution_name: OsString,
    command: Option<OsString>,
    args: Vec<OsString>,
}

pub struct WslCommandOutput {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl WslCommand {
    pub fn new<S1: AsRef<OsStr>, S2: AsRef<OsStr>>(
        command: Option<S1>,
        distribution_name: S2,
    ) -> WslCommand {
        WslCommand {
            distribution_name: distribution_name.as_ref().to_owned(),
            command: command.map(|s| s.as_ref().to_owned()),
            args: vec![],
        }
    }

    pub fn arg<S: AsRef<OsStr>>(&mut self, arg: S) -> &mut Self {
        self.args.push(arg.as_ref().to_owned());
        self
    }

    pub fn args<S, I>(&mut self, args: I) -> &mut Self
    where
        S: AsRef<OsStr>,
        I: IntoIterator<Item = S>,
    {
        self.args
            .extend(args.into_iter().map(|s| s.as_ref().to_owned()));
        self
    }

    pub fn status(&mut self) -> Result<i32> {
        // Use wsl command instead of winapi for now, since it seems more robust way to avoid
        // strange crash on Windows 11.
        let status = self
            .gen_command()
            .status()
            .with_context(|| format!("Failed to invoke status() {:?}", &self))?;
        status
            .code()
            .ok_or_else(|| anyhow!("Failed to get the exit code."))
    }

    pub fn output(&mut self) -> Result<WslCommandOutput> {
        // Use wsl command instead of winapi for now, since it seems more robust way to avoid
        // strange crash on Windows 11.
        let output = self
            .gen_command()
            .output()
            .with_context(|| format!("Failed to invoke output() {:?}", &self))?;
        let status = output
            .status
            .code()
            .ok_or_else(|| anyhow!("Failed to get the exit code {:?}", &self))?;
        let stdout = output.stdout;
        let stderr = output.stderr;
        Ok(WslCommandOutput {
            status,
            stdout,
            stderr,
        })
    }

    fn gen_command(&mut self) -> std::process::Command {
        let mut command = std::process::Command::new("wsl");
        command.arg("-d");
        command.arg(&self.distribution_name);
        if let Some(ref command_name) = self.command {
            command.arg("--");
            command.arg(command_name);
            command.args(&self.args);
        }
        command
    }
}
