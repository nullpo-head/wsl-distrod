use anyhow::{bail, Context, Result};
use colored::*;
use libs::distro::Distro;
use libs::multifork::set_noninheritable_sig_ign;
use nix::unistd::{Gid, Uid};
use std::ffi::{CString, OsStr, OsString};
use std::io::Write;
use std::os::unix::prelude::OsStrExt;
use std::path::Path;
use std::str::FromStr;
use structopt::StructOpt;
use strum::{EnumString, EnumVariantNames};

use libs::passwd::Credential;

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

#[derive(Copy, Clone, Debug, EnumString, EnumVariantNames)]
#[strum(serialize_all = "kebab-case")]
pub enum LogLevel {
    Off,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

fn main() {
    let opts = Opts::from_args();
    init_logger(&opts.log_level);

    if let Err(err) = run(opts) {
        log::error!("{:?}", err);
    }
}

fn init_logger(log_level: &Option<LogLevel>) {
    let mut env_logger_builder = env_logger::Builder::new();

    if let Some(ref level) = log_level {
        env_logger_builder.filter_level(
            log::LevelFilter::from_str(
                <LogLevel as strum::VariantNames>::VARIANTS[*level as usize],
            )
            .unwrap(),
        );
    } else {
        env_logger_builder.filter_level(log::LevelFilter::Info);
    }

    env_logger_builder.format(move |buf, record| {
        writeln!(
            buf,
            "{}{} {}",
            "[Distrod]".bright_green(),
            match record.level() {
                log::Level::Info => "".to_string(),
                log::Level::Error | log::Level::Warn =>
                    format!("[{}]", record.level()).red().to_string(),
                _ => format!("[{}]", record.level()).bright_green().to_string(),
            },
            record.args()
        )
    });
    env_logger_builder.init();
}

fn run(opts: Opts) -> Result<()> {
    if Distro::is_inside_running_distro() {
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
    let cred = get_real_credential()?;

    let distro =
        match Distro::get_running_distro().with_context(|| "Failed to get the running distro.")? {
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
        Some(arg0),
        Some(&cred),
    )?;
    cred.drop_privilege();
    let status = waiter.wait();
    std::process::exit(status as i32)
}

fn get_real_credential() -> Result<Credential> {
    let egid = nix::unistd::getegid(); // root
    let groups = nix::unistd::getgroups().with_context(|| "Failed to get grups")?;
    let groups = groups.into_iter().filter(|group| *group != egid).collect();
    Ok(Credential::new(
        nix::unistd::getuid(),
        nix::unistd::getgid(),
        groups,
    ))
}

fn launch_distro() -> Result<Distro> {
    let distro = Distro::get_installed_distro::<&Path>(None)
        .with_context(|| "Failed to retrieve the installed distro.")?;
    if distro.is_none() {
        bail!("No default distro is configured.",)
    }
    let mut distro = distro.unwrap();
    distro
        .launch()
        .with_context(|| "Failed to launch the distro.")?;
    Ok(distro)
}
