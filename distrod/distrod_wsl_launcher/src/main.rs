use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use libs::cli_ui::{self, build_progress_bar, LogLevel};
use libs::cli_ui::{init_logger, prompt_string};
use libs::distro_image::{
    self, download_file_with_progress, DistroImageFetcher, DistroImageFetcherGen, DistroImageFile,
};
use libs::distrod_config;
use libs::local_image::LocalDistroImage;
use libs::lxd_image::LxdDistroImageList;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Cursor, Read};
use std::path::{Path, PathBuf};
use std::process::Command;
use structopt::StructOpt;
use tempfile::tempdir;
use tempfile::TempDir;
use wslapi::Library as WslApi;
use xz2::read::XzDecoder;

static DISTRO_NAME: &str = "Distrod";

#[derive(Debug, StructOpt)]
#[structopt(name = "distrod-install", rename_all = "kebab")]
pub struct Opts {
    #[structopt(short, long)]
    pub log_level: Option<LogLevel>,
    #[structopt(short, long)]
    pub distro_name: Option<String>,
    #[structopt(subcommand)]
    pub command: Option<Subcommand>,
}

#[derive(Debug, StructOpt)]
pub enum Subcommand {
    Install(InstallOpts),
    Run(RunOpts),
    Config(ConfigOpts),
}

#[derive(Debug, StructOpt)]
#[structopt(rename_all = "kebab")]
pub struct InstallOpts {
    #[structopt(long)]
    root: bool,
    #[structopt(short, long)]
    distro_name: Option<String>,
}

#[derive(Debug, StructOpt)]
#[structopt(rename_all = "kebab")]
pub struct RunOpts {
    cmd: Vec<String>,
}

#[derive(Debug, StructOpt)]
#[structopt(rename_all = "kebab")]
pub struct ConfigOpts {
    #[structopt(long)]
    default_user: Option<String>,
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
    let mut s = String::new();
    let _ = io::stdin().read_line(&mut s);
}

fn run(opts: Opts) -> Result<()> {
    let distro_name = opts.distro_name.unwrap_or_else(|| DISTRO_NAME.to_owned());
    match opts.command {
        None => {
            let run_opts = RunOpts { cmd: vec![] };
            run_distro(&distro_name, run_opts)?;
        }
        Some(Subcommand::Run(run_opts)) => {
            run_distro(&distro_name, run_opts)?;
        }
        Some(Subcommand::Install(install_opts)) => {
            install_distro(&distro_name, install_opts)?;
        }
        Some(Subcommand::Config(config_opts)) => {
            config_distro(&distro_name, config_opts)?;
        }
    }
    Ok(())
}

fn run_distro(distro_name: &str, opts: RunOpts) -> Result<()> {
    let wsl = WslApi::new()
        .with_context(|| "Failed to retrieve WSL API. Have you enabled the WSL2 feature?")?;

    if !wsl.is_distribution_registered(distro_name) {
        let install_opts = InstallOpts {
            root: false,
            distro_name: None,
        };
        return install_distro(distro_name, install_opts);
    }

    let command = construct_cmd_str(opts.cmd);
    wsl.launch_interactive(distro_name, command, true)
        .with_context(|| "Failed to execute command by launch interactive.")?;
    Ok(())
}

fn construct_cmd_str(cmd: Vec<String>) -> OsString {
    OsString::from(
        cmd.into_iter()
            .map(|arg| arg.replace("\\", "\\\\").replace(" ", "\\ "))
            .collect::<Vec<_>>()
            .join(" "),
    )
}

fn config_distro(distro_name: &str, opts: ConfigOpts) -> Result<()> {
    let wsl = WslApi::new()
        .with_context(|| "Failed to retrieve WSL API. Have you enabled the WSL2 feature?")?;

    if let Some(ref default_user) = opts.default_user {
        let tmp_dir = tempdir().with_context(|| "Failed to create a tempdir")?;
        let uid = query_uid(&wsl, distro_name, default_user.as_str(), tmp_dir)
            .with_context(|| format!("Failed to get the uid of {}.", default_user))?;
        wsl.configure_distribution(distro_name, uid, wslapi::WSL_DISTRIBUTION_FLAGS::DEFAULT)
            .with_context(|| "Failed to set the default user")?;
    }
    log::info!("Configuration done.");

    Ok(())
}

#[tokio::main]
async fn install_distro(distro_name: &str, opts: InstallOpts) -> Result<()> {
    let wsl = WslApi::new()
        .with_context(|| "Failed to retrieve WSL API. Have you enabled the WSL2 feature?")?;

    println!(
        r"
        ██████╗ ██╗███████╗████████╗██████╗  ██████╗ ██████╗ 
        ██╔══██╗██║██╔════╝╚══██╔══╝██╔══██╗██╔═══██╗██╔══██╗
        ██║  ██║██║███████╗   ██║   ██████╔╝██║   ██║██║  ██║
        ██║  ██║██║╚════██║   ██║   ██╔══██╗██║   ██║██║  ██║
        ██████╔╝██║███████║   ██║   ██║  ██║╚██████╔╝██████╔╝
        ╚═════╝ ╚═╝╚══════╝   ╚═╝   ╚═╝  ╚═╝ ╚═════╝ ╚═════╝ 
=================================================================================
Thanks for trying Distrod! Choose your distribution to install.                  
You can install a local .tar.xz, or download an image from linuxcontainers.org.  

* linuxcontainers.org is a vendor-neutral project that offers distro images for 
  containers, which is not related to Distrod. LXC/LXD is one of its projects.
  BTW, you can run Systemd with distrod, so you can try LXC/LXD with distrod!
================================================================================="
    );
    let lxd_root_tarxz = fetch_distro_image()
        .await
        .with_context(|| "Failed to fetch a distro image.")?;
    let lxd_tar = tar::Archive::new(XzDecoder::new(lxd_root_tarxz));

    log::info!(
        "Unpacking and merging the given rootfs to the distrod rootfs. This may take a while..."
    );
    let tmp_dir = tempdir().with_context(|| "Failed to create a tempdir")?;
    let install_targz_path = merge_tar_archive(&tmp_dir, lxd_tar)?;

    log::info!("Now Windows is installing the new distribution. This may take a while...");
    register_distribution(&wsl, distro_name, &install_targz_path)
        .with_context(|| "Failed to register the distribution.")?;
    log::info!("Done!");

    log::info!("Initializing the new Distrod distribution. This may take a while...");
    wsl.launch_interactive(
        distro_name,
        format!(
            "{} -l trace enable -d",
            distrod_config::get_distrod_bin_path()
        ),
        true,
    )
    .with_context(|| "Failed to initialize the rootfs image inside WSL.")?;

    // This must be done after enable, because the enable step modifies /etc/skel
    if !opts.root {
        let user_name = prompt_string("Please input the new Linux user name. This doesn't have to be the same as your Windows user name.", "user name", None)?;
        let uid = add_user(&wsl, distro_name, &user_name, tmp_dir);
        if uid.is_err() {
            log::warn!(
                "Adding a user failed, but you can try adding a new user as the root after installation."
            );
        } else {
            wsl.configure_distribution(
                distro_name,
                uid.unwrap(),
                wslapi::WSL_DISTRIBUTION_FLAGS::DEFAULT,
            )
            .with_context(|| "Failed to configure the default uid of the distribution.")?;
        }
    };

    log::info!("Installation of Distrod has completed.");

    wsl.launch_interactive(distro_name, "", true)
        .with_context(|| "Failed start a new session.")?;
    Ok(())
}

async fn fetch_distro_image() -> Result<Box<dyn Read>> {
    let local_image_fetcher =
        || Ok(Box::new(LocalDistroImage::new(&cli_ui::prompt_path)) as Box<dyn DistroImageFetcher>);
    let lxd_image_fetcher =
        || Ok(Box::new(LxdDistroImageList::default()) as Box<dyn DistroImageFetcher>);
    let fetchers = vec![
        Box::new(local_image_fetcher) as DistroImageFetcherGen,
        Box::new(lxd_image_fetcher) as DistroImageFetcherGen,
    ];
    let image = distro_image::fetch_image(fetchers, &cli_ui::choose_from_list, 1)
        .await
        .with_context(|| "Failed to fetch the image list.")?;
    match image.image {
        DistroImageFile::Local(path) => {
            let file =
                File::open(&path).with_context(|| format!("Failed to open '{:?}'.", &path))?;
            Ok(Box::new(BufReader::new(file)) as Box<dyn Read>)
        }
        DistroImageFile::Url(url) => {
            log::info!("Downloading '{}'...", url);
            let mut bytes = vec![];
            download_file_with_progress(&url, build_progress_bar, &mut bytes).await?;
            log::info!("Download done.");
            Ok(Box::new(Cursor::new(bytes)) as Box<dyn Read>)
        }
    }
}

fn merge_tar_archive<R: Read>(work_dir: &TempDir, mut rootfs: tar::Archive<R>) -> Result<PathBuf> {
    let distrod_targz = std::include_bytes!("../resources/distrod_root.tar.gz");
    let mut distrod_tar = tar::Archive::new(GzDecoder::new(std::io::Cursor::new(distrod_targz)));

    let install_targz_path = work_dir.path().join("install.tar.gz");
    let install_targz =
        BufWriter::new(File::create(&install_targz_path).with_context(|| {
            format!("Failed to create a new file at '{:?}'.", install_targz_path)
        })?);
    let encoder = GzEncoder::new(install_targz, flate2::Compression::default());

    let mut builder = tar::Builder::new(encoder);
    append_tar_archive(&mut builder, &mut rootfs)
        .with_context(|| "Failed to merge the downloaded LXD image.")?;
    append_tar_archive(&mut builder, &mut distrod_tar)
        .with_context(|| "Failed to merge the downloaded LXD image.")?;
    builder.finish()?;
    drop(builder); // So that we can close the install_targz file.
    Ok(install_targz_path)
}

fn append_tar_archive<W, R>(
    builder: &mut tar::Builder<W>,
    archive: &mut tar::Archive<R>,
) -> Result<()>
where
    W: std::io::Write,
    R: std::io::Read,
{
    for entry in archive
        .entries()
        .with_context(|| "Failed to read the entries of the archive.")?
    {
        let mut entry = entry?;
        let path = entry.path()?.as_os_str().to_owned();
        let mut data = vec![];
        {
            entry
                .read_to_end(&mut data)
                .with_context(|| format!("Failed to read the data of an entry: {:?}.", &path))?;
        }
        let header = entry.header();
        builder
            .append(&header, Cursor::new(data))
            .with_context(|| format!("Failed to add an entry to an archive. {:?}", path))?;
    }
    Ok(())
}

fn register_distribution<P: AsRef<Path>>(
    wsl: &wslapi::Library,
    distro_name: &str,
    tar_gz_filename: P,
) -> Result<()> {
    // Install the distro by WSL API only when this app is a Windows Store app and --distro-name is not given.
    if distro_name == DISTRO_NAME && is_windows_store_app() {
        wsl.register_distribution(distro_name, tar_gz_filename)
            .with_context(|| "Failed to register the distribution.")
    } else {
        // Otherwise, use wsl.exe --import to install the distro for flexibility.
        let mut cmd = Command::new("cmd.exe");
        cmd.arg("/C")
            .arg("wsl")
            .arg("--import")
            .arg(distro_name)
            .arg(format!("%LocalAppData%\\{}", distro_name))
            .arg(tar_gz_filename.as_ref());
        let mut child = cmd
            .spawn()
            .with_context(|| "Failed to launch wsl.exe command.")?;
        let status = child
            .wait()
            .with_context(|| "Failed to wait for wsl.exe command.")?;
        if !status.success() {
            bail!(
                "Failed: cmd.exe /C wsl --import {} {} {:#?}",
                distro_name,
                format!("%LocalAppData%\\{}", distro_name),
                tar_gz_filename.as_ref()
            );
        }
        log::info!(
            "{} is installed in %LocalAppData%\\{}",
            distro_name,
            distro_name
        );
        Ok(())
    }
}

fn is_windows_store_app() -> bool {
    let inner = || -> Result<bool> {
        let mut self_path =
            std::env::current_exe().with_context(|| "Failed to get the current exe path.")?;
        self_path.pop();
        let self_dir = self_path.file_name().unwrap_or_else(|| OsStr::new(""));
        Ok(self_dir == "WindowsApps")
    };
    inner().unwrap_or(false)
}

fn add_user(
    wsl: &wslapi::Library,
    distro_name: &str,
    user_name: &str,
    tmp_dir: TempDir,
) -> Result<u32> {
    wsl.launch_interactive(
        distro_name,
        format!(
            "useradd -m '{}' && while ! passwd {}; do : ; done && echo '{} ALL=(ALL:ALL) ALL' >> /etc/sudoers",
            user_name, user_name, user_name
        ),
        true,
    )?;
    log::info!("Querying the generated uid. This may take some time depending on your machine.");
    query_uid(wsl, distro_name, user_name, tmp_dir)
}

fn query_uid(
    wsl: &wslapi::Library,
    distro_name: &str,
    user_name: &str,
    tmp_dir: TempDir,
) -> Result<u32> {
    let uid_path = tmp_dir.path().join("uid");
    let uid_file = File::create(&uid_path).with_context(|| "Failed to create a temp file.")?;
    let status = wsl
        .launch(
            distro_name,
            format!("id -u {}", &user_name),
            true,
            wslapi::Stdio::null(),
            uid_file,
            wslapi::Stdio::null(),
        )
        .with_context(|| "Failed to launch id command.")?
        .wait()?;
    if !status.success() {
        bail!("'id -u' failed.");
    }
    let uid_string = std::fs::read_to_string(&uid_path)
        .with_context(|| "Failed to read the contents of id file.")?;
    let uid_u32 = uid_string.trim().parse::<u32>().with_context(|| {
        format!(
            "id command has written an unexpected data: '{}'",
            uid_string
        )
    })?;
    Ok(uid_u32)
}
