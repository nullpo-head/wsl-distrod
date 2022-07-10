use anyhow::{anyhow, bail, Context, Result};
use libs::cli_ui::{build_progress_bar, choose_from_list, init_logger, prompt_path};
use libs::container::{ContainerPath, HostPath};
use libs::distrod_config::{self, DistrodConfig};
use libs::local_image::LocalDistroImage;
use libs::multifork::set_noninheritable_sig_ign;
use nix::unistd::{Gid, Uid};
use std::ffi::{CString, OsString};
use std::fs::File;
use std::io::{stdin, Cursor, Read};
use std::os::unix::prelude::OsStrExt;
use std::path::Path;
use structopt::StructOpt;
use xz2::read::XzDecoder;

use libs::command_alias::CommandAlias;
use libs::container_org_image::ContainerOrgImageList;
use libs::distro::{self, DistroLauncher};
use libs::distro_image::{
    self, download_file_with_progress, DistroImage, DistroImageFetcher, DistroImageFetcherGen,
    DistroImageFile,
};
use libs::passwd::{self, get_credential_from_passwd_file, Credential};
use libs::wsl_interop;

mod autostart;
mod shell_hook;

#[derive(Debug, StructOpt)]
#[structopt(name = "distrod")]
pub struct Opts {
    /// Log level in the env_logger format. Simple levels: trace, debug, info(default), warn, error.
    #[structopt(short, long)]
    pub log_level: Option<String>,
    #[structopt(subcommand)]
    pub command: Subcommand,
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

    #[structopt(short = "i", long)]
    uid: Option<u32>,

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
    start_on_windows_boot: bool,
    #[structopt(short, long)]
    do_full_initialization: bool,
}

#[derive(Debug, StructOpt)]
#[structopt(rename_all = "kebab")]
pub struct DisableOpts {}

fn main() {
    if is_executed_as_alias() {
        init_logger("Distrod".to_owned(), None);
        if let Err(err) = run_as_command_alias() {
            log::error!("{:?}", err);
        }
        return;
    }

    let opts = Opts::from_args();
    let log_level = opts.log_level.as_ref().cloned().or_else(|| {
        DistrodConfig::get()
            .ok()
            .and_then(|config| config.distrod.log_level.clone())
    });
    init_logger("Distrod".to_owned(), log_level);

    if let Err(err) = run(opts) {
        log::error!("{:?}", err);
        std::process::exit(1);
    }
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
    log::debug!("running as command alias");
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
    distro::initialize_distro_rootfs(HostPath::new("/")?, opts.do_full_initialization)
        .with_context(|| "Failed to initialize the rootfs.")?;
    shell_hook::enable_default_shell_hook()
        .with_context(|| "Failed to enable the hook to the default shell.")?;
    log::info!("Distrod has been enabled. Now your shell will start under systemd.");
    if opts.start_on_windows_boot {
        log::info!(
            "Enabling automatic startup of Distrod. UAC dialog will appear because scheduling\n\
             a task requires the admin privilege. Please hit enter to proceed."
        );
        let mut buf = String::new();
        let _ = stdin().read_line(&mut buf);
        autostart::enable_autostart_on_windows_boot(
            &wsl_interop::get_distro_name().with_context(|| "Failed to get the distro name.")?,
        )
        .with_context(|| "Failed to enable the autostart on Windows boot.")?;
        log::info!("Distrod will now start automatically on Windows startup.");
    }
    Ok(())
}

fn disable_wsl_exec_hook(_opts: DisableOpts) -> Result<()> {
    shell_hook::disable_default_shell_hook()
        .with_context(|| "Failed to disable the hook to the default shell.")?;
    if let Err(e) = distro::cleanup_distro_rootfs(HostPath::new("/")?) {
        log::warn!(
            "Failed to clean up the rootfs. Some garbage might not be removed.: {:?}",
            e
        );
    }
    log::info!("Distrod has been disabled. Now systemd will not start automatically.");
    if let Err(e) = autostart::disable_autostart_on_windows_boot(
        &wsl_interop::get_distro_name().with_context(|| "Failed to get the distro name.")?,
    ) {
        log::warn!("Failed to disable the autostart on Windows boot.: {:?}", e);
    }
    Ok(())
}

#[tokio::main]
async fn create_distro(opts: CreateOpts) -> Result<()> {
    let image = match opts.image_path {
        None => {
            let local_image_fetcher =
                || Ok(Box::new(LocalDistroImage::new(&prompt_path)) as Box<dyn DistroImageFetcher>);
            let container_org_image_fetcher =
                || Ok(Box::new(ContainerOrgImageList::default()) as Box<dyn DistroImageFetcher>);
            let fetchers = vec![
                Box::new(local_image_fetcher) as DistroImageFetcherGen,
                Box::new(container_org_image_fetcher) as DistroImageFetcherGen,
            ];
            distro_image::fetch_image(fetchers, &choose_from_list, 1)
                .await
                .with_context(|| "Failed to fetch the image list.")?
        }
        Some(path) => {
            let name = format!(
                "local-{}",
                Path::new(&path)
                    .file_stem()
                    .ok_or_else(|| anyhow!("image {:?} should be a file.", &path))?
                    .to_string_lossy()
                    .replace(".tar", "")
            );
            DistroImage {
                image: DistroImageFile::Local(path),
                name,
            }
        }
    };

    let image_name = image.name;
    let tar_xz = match image.image {
        DistroImageFile::Local(path) => Box::new(
            File::open(&path)
                .with_context(|| format!("Failed to open the distro image file: {:?}.", &path))?,
        ) as Box<dyn Read>,
        DistroImageFile::Url(url) => {
            log::info!("Downloading '{}'...", url);
            let mut bytes = vec![];
            download_file_with_progress(&url, build_progress_bar, &mut bytes).await?;
            log::info!("Download done.");
            Box::new(Cursor::new(bytes)) as Box<dyn Read>
        }
    };

    log::info!("Unpacking...");
    let install_dir = match opts.install_dir {
        Some(install_dir) => install_dir,
        None => {
            let def_install_path =
                DistrodConfig::get().with_context(|| "Failed to get the Distrod config.")?;
            def_install_path
                .distrod
                .distro_images_dir
                .join(&image_name)
                .into()
        }
    };
    let install_dir = Path::new(&install_dir);
    if !install_dir.exists() {
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

    distro::initialize_distro_rootfs(
        HostPath::new(&install_dir.canonicalize().with_context(|| {
            format!("Failed to get the canonicalized path of {:?}", &install_dir)
        })?)?,
        true,
    )
    .with_context(|| "Failed to initialize the rootfs.")?;

    log::info!("{} is created at {:?}", &image_name, install_dir);
    Ok(())
}

fn launch_distro(opts: StartOpts) -> Result<()> {
    if distro::is_inside_running_distro()
        || DistroLauncher::get_running_distro()
            .with_context(|| "Failed to see if there's a running distro.")?
            .is_some()
    {
        bail!("There is already a running distro.");
    }
    let mut distro_launcher = DistroLauncher::new()?;
    if let Some(rootfs) = opts.rootfs {
        distro_launcher
            .with_rootfs(&rootfs)
            .with_context(|| format!("Failed to set {:?} to the rootfs of the distro.", &rootfs))?;
    } else {
        distro_launcher
            .from_default_distro()
            .with_context(|| "Failed to get the default distro.")?;
    }
    distro_launcher
        .launch()
        .with_context(|| "Failed to launch the distro.")?;
    Ok(())
}

fn exec_command(opts: ExecOpts) -> Result<()> {
    let distro = DistroLauncher::get_running_distro()
        .with_context(|| "Failed to get the running distro.")?;
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

    let passwd_path =
        ContainerPath::new("/etc/passwd")?.to_host_path(&HostPath::new(distro.get_rootfs())?);
    let cred = opts
        .uid
        .map(|uid| {
            Ok(
                get_credential_from_passwd_file(opts.user.as_ref(), Some(uid), &passwd_path)
                    .with_context(|| format!("Failed to open the passwd file. {:?}", &passwd_path))?
                    .unwrap_or(Credential {
                        uid: Uid::from_raw(uid),
                        gid: Gid::from_raw(uid),
                        groups: vec![Gid::from_raw(uid)],
                    }),
            )
        })
        .map_or(Ok(None), |v: Result<_>| v.map(Some))
        .with_context(|| "Failed to get credentail.")?;

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

fn stop_distro(opts: StopOpts) -> Result<()> {
    let distro = DistroLauncher::get_running_distro()
        .with_context(|| "Failed to get the running distro.")?;
    if distro.is_none() {
        bail!("No distro is currently running.");
    }
    let distro = distro.unwrap();
    log::debug!("Executing a command in the distro.");
    distro.stop(opts.sigkill)
}
