use anyhow::{bail, Context, Result};
use colored::*;
use libs::distro::Distro;
use nix::unistd::{Gid, Uid};
use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::Path;
use std::str::FromStr;
use structopt::StructOpt;
use strum::{EnumString, EnumVariantNames};

use libs::passwd::drop_privilege;

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
    exec_command("/", &opts.command, &opts.arg0, &opts.args)
}

fn exec_command<P1, P2, S1, S2>(rootfs: P1, command: P2, arg0: S1, args: &[S2]) -> Result<()>
where
    P1: AsRef<Path>,
    P2: AsRef<Path>,
    S1: AsRef<OsStr>,
    S2: AsRef<OsStr>,
{
    let ids = (
        nix::unistd::getuid().as_raw(),
        nix::unistd::getgid().as_raw(),
    );

    let distro =
        match Distro::get_running_distro().with_context(|| "Failed to get the running distro.")? {
            Some(distro) => distro,
            None => {
                // Systemd requires the real uid / gid to be the root.
                nix::unistd::setuid(Uid::from_raw(0))?;
                nix::unistd::setgid(Gid::from_raw(0))?;
                let distro = launch_distro(rootfs.as_ref())?;
                distro
            }
        };

    log::debug!("Executing a command in the distro.");
    let mut waiter = distro.exec_command::<_, _, _, _, &Path>(
        command.as_ref(),
        args,
        None,
        Some(arg0),
        Some(ids),
    )?;
    drop_privilege(ids.0, ids.1);
    let status = waiter
        .wait()
        .with_context(|| "Failed to wait the executed command.")?;
    std::process::exit(status as i32)
}

fn launch_distro<P: AsRef<Path>>(rootfs: P) -> Result<Distro> {
    let distro = Distro::get_installed_distro(rootfs.as_ref())
        .with_context(|| "Failed to retrieve the installed distro.")?;
    if distro.is_none() {
        bail!(
            "Any distribution is not installed in '{:?}' for Distrod.",
            rootfs.as_ref()
        )
    }
    let mut distro = distro.unwrap();
    distro
        .launch()
        .with_context(|| "Failed to launch the distro.")?;
    Ok(distro)
}
