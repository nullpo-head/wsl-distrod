use anyhow::{anyhow, bail, Context, Result};
use colored::*;
use distro::Distro;
use libs::cli_ui::{choose_from_list, prompt_path};
use libs::distrod_config::{self, DistrodConfig};
use libs::local_image::LocalDistroImage;
use libs::multifork::set_noninheritable_sig_ign;
use std::ffi::{CString, OsString};
use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::prelude::OsStrExt;
use std::path::Path;
use std::str::FromStr;
use structopt::StructOpt;
use strum::{EnumString, EnumVariantNames};
use xz2::read::XzDecoder;

use libs::command_alias::CommandAlias;
use libs::distro;
use libs::distro_image::{
    self, DistroImage, DistroImageFetcher, DistroImageFetcherGen, DistroImageFile,
};
use libs::lxd_image::LxdDistroImageList;
use libs::passwd::IdCredential;
use libs::passwd::{self, Credential};

mod shell_hook;

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
    Enable(EnableOpts),
    Disable(DisableOpts),
    Create(CreateOpts),
    Start(StartOpts),
    Exec(ExecOpts),
    Stop(StopOpts),
}

#[derive(Debug, StructOpt)]
#[structopt(rename_all = "kebab")]
pub struct StartOpts {
    #[structopt(short, long)]
    rootfs: Option<OsString>,
}

#[derive(Clone, Debug, StructOpt)]
#[structopt(rename_all = "kebab")]
pub struct ExecOpts {
    command: OsString,
    args: Vec<String>,

    #[structopt(short, long)]
    arg0: Option<OsString>,

    #[structopt(short, long)]
    user: Option<String>,

    #[structopt(short, long)]
    uid: Option<u32>,

    #[structopt(short, long)]
    #[structopt(short, long)]
    working_directory: Option<OsString>,

    #[structopt(short, long)]
    rootfs: Option<OsString>,
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

#[derive(Debug, StructOpt)]
#[structopt(rename_all = "kebab")]
pub struct EnableOpts {
    #[structopt(short, long)]
    do_full_initialization: bool,
}

#[derive(Debug, StructOpt)]
#[structopt(rename_all = "kebab")]
pub struct DisableOpts {}

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

fn is_executed_as_alias() -> bool {
    let inner = || -> Result<bool> {
        let self_path =
            std::env::current_exe().with_context(|| anyhow!("Failed to get the current_exe."))?;
        if self_path.file_name() == Some(std::ffi::OsStr::new("distrod")) {
            return Ok(false);
        }
        Ok(CommandAlias::is_alias(&self_path))
    };
    inner().unwrap_or(false)
}

fn run_as_command_alias() -> Result<()> {
    if !is_executed_as_alias() {
        bail!("Distrod is not run as an alias, but `run_as_command_alias` is called.");
    }
    let self_path =
        std::env::current_exe().with_context(|| anyhow!("Failed to get the current_exe."))?;
    let alias = CommandAlias::open_from_link(&self_path)?;
    let args: Vec<_> = std::env::args().into_iter().collect();
    let mut exec_args = vec![
        OsString::from("distrod-exec"),
        OsString::from("--"),
        OsString::from(alias.get_source_path()),
    ];
    exec_args.extend(args.iter().map(OsString::from));
    let cargs: Vec<CString> = exec_args
        .into_iter()
        .map(|arg| {
            CString::new(arg.as_bytes())
                .expect("CString must be able to be created from non-null bytes.")
        })
        .collect();
    let distrod_exec_path = distrod_config::get_distrod_exec_bin_path();
    nix::unistd::execv(&CString::new(distrod_exec_path)?, &cargs)?;
    std::process::exit(1);
}

fn run(opts: Opts) -> Result<()> {
    if !nix::unistd::getuid().is_root() {
        bail!("Distrod needs the root permission.");
    }

    match opts.command {
        Subcommand::Enable(enable_opts) => {
            enable_wsl_exec_hook(enable_opts)?;
        }
        Subcommand::Disable(disable_opts) => {
            disable_wsl_exec_hook(disable_opts)?;
        }
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

fn enable_wsl_exec_hook(opts: EnableOpts) -> Result<()> {
    shell_hook::enable_default_shell_hook()
        .with_context(|| "Failed to enable the hook to the default shell.")?;
    distro::initialize_distro_rootfs("/", opts.do_full_initialization)
        .with_context(|| "Failed to initialize the rootfs.")?;
    log::info!("Distrod has been enabled. Now your shell will start under systemd.");
    Ok(())
}

fn disable_wsl_exec_hook(_opts: DisableOpts) -> Result<()> {
    shell_hook::disable_default_shell_hook()
        .with_context(|| "Failed to disable the hook to the default shell.")?;
    log::info!("Distrod has been disabled. Now systemd won't start automatically.");
    Ok(())
}

fn create_distro(opts: CreateOpts) -> Result<()> {
    let image = match opts.image_path {
        None => {
            let local_image_fetcher =
                || Ok(Box::new(LocalDistroImage::new(prompt_path)) as Box<dyn DistroImageFetcher>);
            let lxd_image_fetcher =
                || Ok(Box::new(LxdDistroImageList::default()) as Box<dyn DistroImageFetcher>);
            let fetchers = vec![
                Box::new(local_image_fetcher) as Box<DistroImageFetcherGen>,
                Box::new(lxd_image_fetcher) as Box<DistroImageFetcherGen>,
            ];
            distro_image::fetch_image(fetchers, choose_from_list, 1)
                .with_context(|| "Failed to fetch the image list.")?
        }
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
            let client = reqwest::blocking::Client::builder().timeout(None).build()?;
            let response = client
                .get(&url)
                .send()
                .with_context(|| format!("Failed to download {}.", &url))?;
            log::info!("Download done.");
            Box::new(std::io::Cursor::new(response.bytes()?)) as Box<dyn Read>
        }
    };

    log::info!("Unpacking...");
    let install_dir = match opts.install_dir {
        Some(install_dir) => install_dir,
        None => {
            let def_install_path =
                DistrodConfig::get().with_context(|| "Failed to ghet the Distrod config.")?;
            def_install_path
                .distrod
                .distro_images_dir
                .join(&image_name)
                .into()
        }
    };
    if !Path::new(&install_dir).exists() {
        std::fs::create_dir_all(&install_dir)
            .with_context(|| format!("Failed to make a directory: {:?}.", &install_dir))?;
    }
    let tar = XzDecoder::new(tar_xz);
    let mut archive = tar::Archive::new(tar);
    archive.set_preserve_permissions(true);
    archive.set_unpack_xattrs(true);
    archive
        .unpack(&install_dir)
        .with_context(|| format!("Failed to unpack the image to '{:?}'.", &install_dir))?;

    distro::initialize_distro_rootfs(&install_dir, true)
        .with_context(|| "Failed to initialize the rootfs.")?;

    log::info!("{} is created at {:?}", &image_name, install_dir);
    Ok(())
}

fn launch_distro(opts: StartOpts) -> Result<()> {
    if Distro::is_inside_running_distro()
        || Distro::get_running_distro()
            .with_context(|| "Failed to see if there's a running distro.")?
            .is_some()
    {
        bail!("There is already a running distro.");
    }
    let distro =
        Distro::get_installed_distro(Some(&opts.rootfs.as_ref().unwrap_or(&OsString::from("/"))))
            .with_context(|| "Failed to retrieve the installed distro.")?;
    if distro.is_none() {
        bail!(
            "Any distribution is not installed in '{:?}' for Distrod.",
            &opts.rootfs
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
        if let Some(ref rootfs) = opts.rootfs {
            launch_distro(StartOpts {
                rootfs: Some(rootfs.clone()),
            })?;
            return exec_command(opts);
        }
        bail!("No distro is currently running.");
    }
    let distro = distro.unwrap();

    let host_root_path = OsString::from("/");
    let rootfs_path = opts.rootfs.as_ref().unwrap_or(&host_root_path);
    let cred = get_credential(opts.user.as_ref(), opts.uid, rootfs_path)?;

    log::debug!("Executing a command in the distro.");
    set_noninheritable_sig_ign();
    let mut waiter = distro.exec_command(
        &opts.command,
        &opts.args,
        opts.working_directory,
        opts.arg0,
        cred.as_ref(),
    )?;
    if let Some(cred) = cred {
        cred.drop_privilege();
    }
    let status = waiter.wait();
    std::process::exit(status as i32)
}

fn get_credential<P: AsRef<Path>>(
    name: Option<&String>,
    uid: Option<u32>,
    rootfs_path: P,
) -> Result<Option<Credential>> {
    let mut passwd_file = passwd::PasswdFile::open(&rootfs_path.as_ref().join("etc/passwd"))
        .with_context(|| {
            format!(
                "Failed to open the passwd file. '{:?}'",
                rootfs_path.as_ref()
            )
        })?;
    let cred = match (name, uid) {
        (Some(name), _) => Credential::from_user(IdCredential::Name(name), &mut passwd_file)?,
        (_, Some(uid)) => Credential::from_user(IdCredential::Uid(uid), &mut passwd_file)?,
        _ => return Ok(None),
    };
    Ok(Some(cred))
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
