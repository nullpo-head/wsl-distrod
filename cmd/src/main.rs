use anyhow::Result;
use anyhow::{bail, Context};
use colored::*;
use distro::Distro;
use lxd_image::{DistroImageFetcher, DistroImageFile};
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;
use std::str::FromStr;
use xz2::read::XzDecoder;

use structopt::StructOpt;
use strum::{EnumString, EnumVariantNames};

use crate::lxd_image::{DefaultImageFetcher, DistroImageList};

mod container;
mod distro;
mod lxd_image;
mod multifork;
mod procfile;

#[derive(Debug, StructOpt)]
#[structopt(name = "distrod")]
pub struct Opts {
    #[structopt(short, long)]
    pub log_level: Option<LogLevel>,
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
    Install(InstallOpts),
    Launch(LaunchOpts),
    Exec(ExecOpts),
    Stop(StopOpts),
}

#[derive(Debug, StructOpt)]
#[structopt(rename_all = "kebab")]
pub struct LaunchOpts {
    root_fs: String,
}

#[derive(Debug, StructOpt)]
#[structopt(rename_all = "kebab")]
pub struct ExecOpts {
    command: String,
    args: Vec<String>,

    #[structopt(short, long)]
    working_directory: Option<String>,
}

#[derive(Debug, StructOpt)]
#[structopt(rename_all = "kebab")]
pub struct StopOpts {
    #[structopt(short = "9", long)]
    sigkill: bool,
}

#[derive(Debug, StructOpt)]
#[structopt(rename_all = "kebab")]
pub struct InstallOpts {
    distro: Option<String>,

    #[structopt(short, long)]
    install_dir: Option<String>,
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
    if !nix::unistd::getuid().is_root() {
        //bail!("Distrod needs the root permission.");
    }
    match opts.command {
        Subcommand::Install(install_opts) => {
            install_distro(install_opts)?;
        }
        Subcommand::Launch(launch_opts) => {
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

fn install_distro(opts: InstallOpts) -> Result<()> {
    let install_dir = opts
        .install_dir
        .unwrap_or_else(|| "/var/lib/distrod".to_owned());
    if !Path::new(&install_dir).exists() {
        std::fs::create_dir_all(&install_dir)
            .with_context(|| format!("Failed to make a directory: {}.", &install_dir))?;
    }
    let image = lxd_image::fetch_lxd_image(choose_from_list)
        .with_context(|| "Failed to fetch the lxd image list.")?;
    let tar_xz = match image.image {
        DistroImageFile::Local(path) => Box::new(File::open(path)?) as Box<dyn Read>,
        DistroImageFile::Url(url) => {
            log::info!("Downloading '{}'...", url);
            let response = reqwest::blocking::get(&url)
                .with_context(|| format!("Failed to download {}.", &url))?;
            Box::new(std::io::Cursor::new(response.bytes()?)) as Box<dyn Read>
        }
    };
    log::info!("Unpacking...");
    let distro_install_dir = format!("{}/{}", install_dir, image.name);
    let tar = XzDecoder::new(tar_xz);
    let mut archive = tar::Archive::new(tar);
    archive
        .unpack(&distro_install_dir)
        .with_context(|| format!("Failed to unpack the image to '{}'.", &distro_install_dir))?;
    log::info!("Installation of {} is done!", image.name);
    Ok(())
}

fn choose_from_list(list: DistroImageList) -> Result<Box<dyn DistroImageFetcher>> {
    match list {
        DistroImageList::Fetcher(list_item_kind, fetchers, default) => {
            if fetchers.is_empty() {
                bail!("Empty list of {}.", &list_item_kind);
            }
            let default = match default {
                DefaultImageFetcher::Index(index) => fetchers[index].get_name().to_owned(),
                DefaultImageFetcher::Name(name) => name,
            };
            for (i, fetcher) in fetchers.iter().enumerate() {
                println!("{} {}", format!("[{}]", i + 1).cyan(), fetcher.get_name());
            }
            log::info!("Choose {} from the list above.", &list_item_kind);
            loop {
                log::info!("Type the name or the index of your choice.");
                print!("[Default: {}]: ", &default);
                let _ = io::stdout().flush();
                let mut choice = String::new();
                io::stdin()
                    .read_line(&mut choice)
                    .with_context(|| "failed to read from the stdin.")?;
                choice = choice.trim_end_matches('\n').to_owned();
                if choice.is_empty() {
                    choice = default.to_owned();
                }
                let index = fetchers
                    .iter()
                    .position(|fetcher| fetcher.get_name() == choice.as_str());
                if let Some(index) = index {
                    return Ok(fetchers.into_iter().nth(index).unwrap());
                }
                if let Ok(index) = choice.parse::<usize>() {
                    if index <= fetchers.len() && index >= 1 {
                        return Ok(fetchers.into_iter().nth(index - 1).unwrap());
                    }
                }
                log::info!("{} is off the list.", choice);
            }
        }
        DistroImageList::Image(_) => bail!("Image should not be passed to choose_from_list."),
    }
}

fn launch_distro(opts: LaunchOpts) -> Result<()> {
    let distro = Distro::get_installed_distro(&opts.root_fs)
        .with_context(|| "Failed to retrieve the installed distro.")?;
    if distro.is_none() {
        bail!(
            "Any distribution is not installed in '{}' for Distrod.",
            &opts.root_fs
        )
    }
    let mut distro = distro.unwrap();
    distro
        .launch()
        .with_context(|| "Failed to launch the distro.")
}

fn exec_command(opts: ExecOpts) -> Result<()> {
    let distro =
        Distro::get_running_distro().with_context(|| "Failed to get the running distro.")?;
    if distro.is_none() {
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
