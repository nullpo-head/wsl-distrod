use anyhow::{anyhow, bail, Context, Result};
use colored::*;
use common::cli_ui::choose_from_list;
use distro::Distro;
use std::ffi::OsString;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::str::FromStr;
use structopt::StructOpt;
use strum::{EnumString, EnumVariantNames};
use xz2::read::XzDecoder;

use common::distro_image::{DistroImage, DistroImageFile};
use common::lxd_image;

mod container;
mod distro;
mod multifork;
mod procfile;

#[derive(Debug, StructOpt)]
#[structopt(name = "distrod")]
pub struct Opts {
    #[structopt(short, long)]
    pub log_level: Option<LogLevel>,
    #[structopt(short, long)]
    pub call_from_wsl: bool,
    #[structopt(subcommand)]
    pub command: Subcommand,
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

#[derive(Debug, StructOpt)]
pub enum Subcommand {
    Create(CreateOpts),
    Start(StartOpts),
    Exec(ExecOpts),
    Stop(StopOpts),
}

#[derive(Debug, StructOpt)]
#[structopt(rename_all = "kebab")]
pub struct StartOpts {
    root_fs: OsString,
}

#[derive(Clone, Debug, StructOpt)]
#[structopt(rename_all = "kebab")]
pub struct ExecOpts {
    command: OsString,
    args: Vec<String>,

    #[structopt(short, long)]
    working_directory: Option<OsString>,

    #[structopt(short, long)]
    root: Option<OsString>,
}

#[derive(Debug, StructOpt)]
#[structopt(rename_all = "kebab")]
pub struct StopOpts {
    #[structopt(short = "9", long)]
    sigkill: bool,
}

#[derive(Debug, StructOpt)]
#[structopt(rename_all = "kebab")]
pub struct CreateOpts {
    #[structopt(short = "d", long)]
    install_dir: Option<OsString>,
    #[structopt(short = "i", long)]
    image_path: Option<OsString>,
}

fn main() {
    if is_executed_as_alias() {
        init_logger(&Some(LogLevel::Info));
        if let Err(err) = run_as_command_alias() {
            log::error!("{:?}", err);
        }
        return;
    }

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

static ALIAS_DIR: &str = "/opt/distrod/alias";

fn is_executed_as_alias() -> bool {
    let inner = || -> Result<bool> {
        let self_path =
            std::env::current_exe().with_context(|| anyhow!("Failed to get the current_exe."))?;
        if self_path.file_name() == Some(std::ffi::OsStr::new("distrod")) {
            return Ok(false);
        }
        Ok(self_path.starts_with(ALIAS_DIR))
    };
    inner().unwrap_or(false)
}

fn run_as_command_alias() -> Result<()> {
    if !is_executed_as_alias() {
        bail!("Distrod is not run as an aliase");
    }
    let self_path =
        std::env::current_exe().with_context(|| anyhow!("Failed to get the current_exe."))?;
    let target_path = Path::new("/").join(self_path.strip_prefix(ALIAS_DIR)?);
    let args = std::env::args().into_iter().collect();
    let exec_opts = ExecOpts {
        command: target_path.into_os_string(),
        args,
        working_directory: None,
        root: None,
    };
    exec_command(exec_opts)
}

fn run(opts: Opts) -> Result<()> {
    if !nix::unistd::getuid().is_root() {
        bail!("Distrod needs the root permission.");
    }

    match opts.command {
        Subcommand::Create(install_opts) => {
            create_distro(install_opts)?;
        }
        Subcommand::Start(launch_opts) => {
            launch_distro(launch_opts)?;
        }
        Subcommand::Exec(exec_opts) => {
            exec_command(exec_opts)?;
        }
        Subcommand::Stop(stop_opts) => {
            stop_distro(stop_opts)?;
        }
    }
    Ok(())
}

fn create_distro(opts: CreateOpts) -> Result<()> {
    let image = match opts.image_path {
        None => lxd_image::fetch_lxd_image(choose_from_list)
            .with_context(|| "Failed to fetch the lxd image list.")?,
        Some(path) => DistroImage {
            image: DistroImageFile::Local(path),
            name: "distrod".to_owned(),
        },
    };
    let image_name = image.name;
    let tar_xz = match image.image {
        DistroImageFile::Local(path) => Box::new(
            File::open(&path)
                .with_context(|| format!("Failed to open the distro image file: {:?}.", &path))?,
        ) as Box<dyn Read>,
        DistroImageFile::Url(url) => {
            log::info!("Downloading '{}'...", url);
            let response = reqwest::blocking::get(&url)
                .with_context(|| format!("Failed to download {}.", &url))?;
            Box::new(std::io::Cursor::new(response.bytes()?)) as Box<dyn Read>
        }
    };
    let install_dir = opts
        .install_dir
        .unwrap_or_else(|| format!("/var/lib/distrod/{}", &image_name).into());
    if !Path::new(&install_dir).exists() {
        std::fs::create_dir_all(&install_dir)
            .with_context(|| format!("Failed to make a directory: {:?}.", &install_dir))?;
    }
    log::info!("Unpacking...");
    let tar = XzDecoder::new(tar_xz);
    let mut archive = tar::Archive::new(tar);
    archive.set_preserve_permissions(true);
    archive.set_unpack_xattrs(true);
    archive
        .unpack(&install_dir)
        .with_context(|| format!("Failed to unpack the image to '{:?}'.", &install_dir))?;
    log::info!("Extraction of {} is done!", &image_name);
    Ok(())
}

fn launch_distro(opts: StartOpts) -> Result<()> {
    let distro = Distro::get_installed_distro(&opts.root_fs)
        .with_context(|| "Failed to retrieve the installed distro.")?;
    if distro.is_none() {
        bail!(
            "Any distribution is not installed in '{:?}' for Distrod.",
            &opts.root_fs
        )
    }
    let mut distro = distro.unwrap();
    distro
        .launch()
        .with_context(|| "Failed to launch the distro.")
}

fn exec_command(opts: ExecOpts) -> Result<()> {
    let mut distro =
        Distro::get_running_distro().with_context(|| "Failed to get the running distro.")?;
    if distro.is_none() {
        if let Some(ref rootfs) = opts.root {
            distro = Distro::get_installed_distro(&rootfs)
                .with_context(|| "Failed to retrieve the installed distro.")?;
            if distro.is_none() {
                bail!(
                    "Any distribution is not installed in '{:?}' for Distrod.",
                    &rootfs
                )
            }
            let mut distrol = distro.unwrap();
            distrol
                .launch()
                .with_context(|| "Failed to launch the distro.")?;
            return exec_command(opts.clone());
        }
        bail!("No distro is currently running.");
    }
    let distro = distro.unwrap();
    log::debug!("Executing a command in the distro.");
    let status = distro.exec_command(&opts.command, &opts.args, opts.working_directory)?;
    std::process::exit(status as i32)
}

fn stop_distro(opts: StopOpts) -> Result<()> {
    let distro =
        Distro::get_running_distro().with_context(|| "Failed to get the running distro.")?;
    if distro.is_none() {
        bail!("No distro is currently running.");
    }
    let distro = distro.unwrap();
    log::debug!("Executing a command in the distro.");
    distro.stop(opts.sigkill)
}
