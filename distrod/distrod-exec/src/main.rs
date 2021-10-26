use anyhow::{Context, Result};
use libs::cli_ui::{init_logger, LogLevel};
use libs::distro::{self, Distro, DistroLauncher};
use libs::multifork::set_noninheritable_sig_ign;
use nix::unistd::{Gid, Uid};
use std::ffi::{CString, OsStr, OsString};
use std::os::unix::prelude::OsStrExt;
use std::path::Path;
use structopt::StructOpt;

use libs::passwd::get_real_credential;

/// Distrod-exec is a small helper command to allow a non-root user to run programs under the systemd container.
/// It implements the subset features of distrod's exec subcommand, but has the setuid bit set.
/// Typically it is run by the main distrod command when distrod is launched as an alias of another command.
#[derive(Debug, StructOpt)]
#[structopt(name = "distrod-exec")]
pub struct Opts {
    pub command: OsString,
    pub arg0: OsString,
    pub args: Vec<String>,

    #[structopt(short, long)]
    pub log_level: Option<LogLevel>,
}

fn main() {
    let opts = Opts::from_args();
    init_logger(
        "Distrod".to_owned(),
        *opts.log_level.as_ref().unwrap_or(&LogLevel::Info),
    );

    if let Err(err) = run(opts) {
        log::error!("{:?}", err);
    }
}

fn run(opts: Opts) -> Result<()> {
    if distro::is_inside_running_distro() {
        exec_command(&opts.command, &opts.arg0, &opts.args)
    } else {
        exec_command_in_distro(&opts.command, &opts.arg0, &opts.args)
    }
}

fn exec_command<P1, S1, S2>(command: P1, arg0: S1, args: &[S2]) -> Result<()>
where
    P1: AsRef<Path>,
    S1: AsRef<OsStr>,
    S2: AsRef<OsStr>,
{
    let cred = get_real_credential()?;
    cred.drop_privilege();

    let path = CString::new(command.as_ref().as_os_str().as_bytes()).with_context(|| {
        format!(
            "Failed to construct a CString for the alias command.: '{:?}'",
            command.as_ref()
        )
    })?;
    let mut cargs: Vec<CString> = vec![CString::new(arg0.as_ref().as_bytes())?];
    cargs.extend(args.iter().map(|arg| {
        CString::new(arg.as_ref().as_bytes())
            .expect("CString must be able to be created from non-null bytes.")
    }));
    nix::unistd::execv(&path, &cargs)?;
    std::process::exit(1);
}

fn exec_command_in_distro<P1, S1, S2>(command: P1, arg0: S1, args: &[S2]) -> Result<()>
where
    P1: AsRef<Path>,
    S1: AsRef<OsStr>,
    S2: AsRef<OsStr>,
{
    let inner = || -> Result<()> {
        let cred = get_real_credential()?;

        let distro = match DistroLauncher::get_running_distro()
            .with_context(|| "Failed to get the running distro.")?
        {
            Some(distro) => distro,
            None => {
                // Systemd requires the real uid / gid to be the root.
                nix::unistd::setuid(Uid::from_raw(0))?;
                nix::unistd::setgid(Gid::from_raw(0))?;
                launch_distro()?
            }
        };

        log::debug!("Executing a command in the distro.");
        set_noninheritable_sig_ign();
        let mut waiter = distro.exec_command(
            command.as_ref(),
            args,
            Some(std::env::current_dir().with_context(|| "Failed to get the current dir.")?),
            Some(arg0.as_ref()),
            Some(&cred),
        )?;
        cred.drop_privilege();
        let status = waiter.wait();
        std::process::exit(status as i32)
    };

    if let Err(e) = inner() {
        log::error!("Failed to run the given command in the Systemd container. Fall back to normal WSL2 command execution without using Systemd. {:?}", e);
        return exec_command(command, arg0.as_ref(), args);
    }
    Ok(())
}

fn launch_distro() -> Result<Distro> {
    let mut distro_launcher = DistroLauncher::new()?;
    distro_launcher
        .from_default_distro()
        .with_context(|| "Failed to get the default distro.")?;
    let distro = distro_launcher
        .launch()
        .with_context(|| "Failed to launch the distro.")?;
    Ok(distro)
}
