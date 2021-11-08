use anyhow::{Context, Result};
use libs::cli_ui::LoggerInitializer;
use libs::distro::{self, Distro, DistroLauncher};
use libs::distrod_config::DistrodConfig;
use libs::multifork::set_noninheritable_sig_ign;
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

    /// Log level in the env_logger format. Simple levels: trace, debug, info(default), warn, error.
    #[structopt(short, long)]
    pub log_level: Option<String>,

    /// /dev/kmsg log level in the env_logger format. Simple levels: trace, debug, info, warn, error(default).
    #[structopt(short, long)]
    pub kmsg_log_level: Option<String>,
}

fn main() {
    let opts = Opts::from_args();

    init_logger(&opts);

    if let Err(err) = run(opts) {
        log::error!("{:?}", err);
    }
}

fn init_logger(opts: &Opts) {
    let mut logger_initializer = LoggerInitializer::default();
    let distrod_config = DistrodConfig::get();
    if let Some(log_level) = opts.log_level.as_ref().cloned().or_else(|| {
        distrod_config
            .as_ref()
            .ok()
            .and_then(|config| config.distrod.log_level.clone())
    }) {
        logger_initializer.with_log_level(log_level);
    }
    logger_initializer.with_kmsg(true);
    if let Some(kmsg_log_level) = opts.kmsg_log_level.as_ref().cloned().or_else(|| {
        distrod_config
            .ok()
            .and_then(|ref config| config.distrod.kmsg_log_level.clone())
    }) {
        logger_initializer.with_kmsg_log_level(kmsg_log_level);
    }
    logger_initializer.init("Distrod".to_owned());
}

fn run(opts: Opts) -> Result<()> {
    if distro::is_inside_running_distro() {
        exec_command(&opts.command, &opts.arg0, &opts.args).with_context(|| "exec_command failed.")
    } else {
        exec_command_in_distro(&opts.command, &opts.arg0, &opts.args)
            .with_context(|| "exec_command_in_distro failed.")
    }
}

fn exec_command<P1, S1, S2>(command: P1, arg0: S1, args: &[S2]) -> Result<()>
where
    P1: AsRef<Path>,
    S1: AsRef<OsStr>,
    S2: AsRef<OsStr>,
{
    let cred = get_real_credential().with_context(|| "Failed to get the real credential.")?;
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
        let cred = get_real_credential().with_context(|| "Failed to get the real credential.")?;

        let distro = match DistroLauncher::get_running_distro()
            .with_context(|| "Failed to get the running distro.")?
        {
            Some(distro) => distro,
            None => launch_distro()?,
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
    delay_init_launch();
    log::debug!("starting /init from distrod-exec");

    let mut distro_launcher =
        DistroLauncher::new().with_context(|| "Failed to init a DistroLauncher")?;
    distro_launcher
        .from_default_distro()
        .with_context(|| "Failed to get the default distro.")?;
    let distro = distro_launcher
        .launch()
        .with_context(|| "Failed to launch the distro.")?;
    Ok(distro)
}

static DISTROD_EXEC_DELAY_ENV_NAME: &str = "DISTROD_EXEC_INIT_LAUNCH_DELAY";

/// On some distros, starting Systemd during WSL's /init being initialized on Windows startup
/// makes /init crash. So launch Systemd after some delay.
fn delay_init_launch() {
    let delay_sec_str = match std::env::var(DISTROD_EXEC_DELAY_ENV_NAME) {
        Ok(delay_sec_str) => delay_sec_str,
        _ => return,
    };
    let delay_sec: u32 = match delay_sec_str.parse() {
        Ok(delay_sec) => delay_sec,
        Err(e) => {
            log::warn!(
                "[BUG] Invalid {} was given: {:?}. {:?}",
                DISTROD_EXEC_DELAY_ENV_NAME,
                delay_sec_str,
                e
            );
            return;
        }
    };

    log::debug!(
        "Delaying launching init by {}sec. {:?}",
        delay_sec,
        std::time::Instant::now()
    );
    std::thread::sleep(std::time::Duration::from_secs(delay_sec as u64));

    strip_wslenv_for_distod_exec_delay();
    log::debug!("delay finished {:?}", std::time::Instant::now());
}

fn strip_wslenv_for_distod_exec_delay() {
    let inner = || -> Result<()> {
        let wslenv = std::env::var("WSLENV")?;
        if let Some(stripped) = wslenv.strip_suffix(&format!(":{}", DISTROD_EXEC_DELAY_ENV_NAME)) {
            std::env::set_var("WSLENV", stripped);
        }
        Ok(())
    };
    let _ = inner();
}
