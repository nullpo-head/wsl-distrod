use anyhow::{anyhow, bail, Context, Result};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use libs::cli_ui::{self, build_progress_bar};
use libs::cli_ui::{init_logger, prompt_string};
use libs::container_org_image::ContainerOrgImageList;
use libs::distro_image::{
    self, download_file_with_progress, DistroImageFetcher, DistroImageFetcherGen, DistroImageFile,
};
use libs::distrod_config;
use libs::local_image::LocalDistroImage;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Cursor, Read};
use std::path::{Path, PathBuf};
use std::process::Command;
use structopt::StructOpt;
use tempfile::tempdir;
use tempfile::TempDir;
use xz2::read::XzDecoder;

mod wsl;

static DISTRO_NAME: &str = "Distrod";

#[derive(Debug, StructOpt)]
#[structopt(name = "distrod-install", rename_all = "kebab")]
pub struct Opts {
    /// Log level in the env_logger format. Simple levels: trace, debug, info(default), warn, error.
    #[structopt(short, long)]
    pub log_level: Option<String>,
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
    init_logger("Distrod".to_owned(), opts.log_level.clone());

    if let Err(err) = run(opts) {
        log::error!("{:?}", err);
    }
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
    if !unsafe { wsl::is_distribution_registered(distro_name) } {
        let install_opts = InstallOpts { root: false };
        return install_distro(distro_name, install_opts);
    }

    let mut command = wsl::WslCommand::new(opts.cmd.get(0), distro_name);
    if opts.cmd.len() > 1 {
        command.args(&opts.cmd[1..]);
    }
    let _status = command
        .status()
        .with_context(|| format!("Failed to run {:?}", &opts))?;
    Ok(())
}

fn config_distro(distro_name: &str, opts: ConfigOpts) -> Result<()> {
    if let Some(ref default_user) = opts.default_user {
        let uid = match default_user.parse::<u32>() {
            Ok(uid) => uid,
            _ => query_uid(distro_name, default_user.as_str())
                .with_context(|| format!("Failed to get the uid of {}.", default_user))?,
        };
        unsafe {
            wsl::set_distribution_default_user(distro_name, uid)
                .with_context(|| "Failed to set the default user")?;
        }
    }
    log::info!("Configuration done.");

    Ok(())
}

#[tokio::main]
async fn install_distro(distro_name: &str, opts: InstallOpts) -> Result<()> {
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
    let container_org_root_tarxz = fetch_distro_image()
        .await
        .with_context(|| "Failed to fetch a distro image.")?;
    let container_org_tar = tar::Archive::new(XzDecoder::new(container_org_root_tarxz));

    log::info!(
        "Unpacking and merging the given rootfs to the distrod rootfs. This may take a while..."
    );
    let tmp_dir = tempdir().with_context(|| "Failed to create a tempdir")?;
    let install_targz_path = merge_tar_archive(&tmp_dir, container_org_tar)?;

    log::info!("Now Windows is installing the new distribution. This may take a while...");
    register_distribution(distro_name, &install_targz_path)
        .with_context(|| "Failed to register the distribution.")?;
    log::info!("Done!");

    let uid = if !opts.root {
        let user_name = prompt_string("Please input the new Linux user name. This doesn't have to be the same as your Windows user name.", "user name", None)?;
        let uid = add_user(distro_name, &user_name);
        if let Err(ref e) = uid {
            log::warn!(
                "Adding a user failed, but you can try adding a new user as the root after installation. {:?}",
                e
            );
        }
        uid.unwrap_or(0)
    } else {
        0
    };

    log::info!("Initializing the new Distrod distribution. This may take a while...");
    let mut distrod_enable =
        wsl::WslCommand::new(Some(distrod_config::get_distrod_bin_path()), distro_name);
    distrod_enable.args(["enable", "-d"]);
    let exit_code = distrod_enable
        .status()
        .with_context(|| "Failed to initialize the rootfs image inside WSL.")?;
    if exit_code != 0 {
        bail!(
            "Initialization command exited with error. error: {} cmd: {:?}",
            exit_code,
            &distrod_enable
        );
    }

    if uid != 0 {
        // This should be done after enable, because this changes the default user from root.
        log::info!("Setting the default user to uid: {}", uid);
        set_default_user(distro_name, uid).with_context(|| "Failed to set the default user")?;
    }

    log::info!("Installation of Distrod is now complete.");
    let _ = wsl::WslCommand::new::<String, _>(None, distro_name)
        .status()
        .with_context(|| "Failed to initialize the rootfs image inside WSL.")?;

    log::info!("Hit enter to exit.");
    let mut s = String::new();
    let _ = io::stdin().read_line(&mut s);

    Ok(())
}

async fn fetch_distro_image() -> Result<Box<dyn Read>> {
    let local_image_fetcher =
        || Ok(Box::new(LocalDistroImage::new(&cli_ui::prompt_path)) as Box<dyn DistroImageFetcher>);
    let container_org_image_fetcher =
        || Ok(Box::new(ContainerOrgImageList::default()) as Box<dyn DistroImageFetcher>);
    let fetchers = vec![
        Box::new(local_image_fetcher) as DistroImageFetcherGen,
        Box::new(container_org_image_fetcher) as DistroImageFetcherGen,
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
        .with_context(|| "Failed to merge the given image.")?;
    append_tar_archive(&mut builder, &mut distrod_tar)
        .with_context(|| "Failed to merge the given image.")?;
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
            .append(header, Cursor::new(data))
            .with_context(|| format!("Failed to add an entry to an archive. {:?}", path))?;
    }
    Ok(())
}

fn register_distribution<P: AsRef<Path>>(distro_name: &str, tar_gz_filename: P) -> Result<()> {
    // Install the distro by WSL API only when this app is a Windows Store app and --distro-name is not given.
    if distro_name == DISTRO_NAME && is_windows_store_app() {
        unsafe {
            wsl::register_distribution(distro_name, tar_gz_filename)
                .with_context(|| "Failed to register the distribution.")
        }
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

fn add_user(distro_name: &str, user_name: &str) -> Result<u32> {
    let mut user_add = wsl::WslCommand::new(Some("/bin/sh"), distro_name);
    user_add.arg("-c");
    user_add.arg(format!(
            "useradd -m --shell /bin/bash '{}' && while ! passwd {}; do : ; done && echo '{} ALL=(ALL:ALL) ALL' >> /etc/sudoers",
            user_name, user_name, user_name
        ));
    let status = user_add
        .status()
        .with_context(|| "Failed to invoke user_add")?;
    if status != 0 {
        bail!("user_add exited with error code {}", status);
    }
    log::info!("Querying the generated uid. This may take some time depending on your machine.");
    query_uid(distro_name, user_name)
}

fn query_uid(distro_name: &str, user_name: &str) -> Result<u32> {
    let mut id = wsl::WslCommand::new(Some("id"), distro_name);
    id.arg("-u");
    id.arg(user_name);
    let output = id.output().with_context(|| "Failed to spawn id command.")?;
    if output.status != 0 {
        bail!("'id -u' exited with error code. {}", output.status);
    }
    let uid_string = String::from_utf8(output.stdout)
        .with_context(|| "The output of id command is invalid utf-8.")?;
    let uid_u32 = uid_string.trim().parse::<u32>().with_context(|| {
        format!(
            "id command has written an unexpected data: '{}'",
            uid_string
        )
    })?;
    Ok(uid_u32)
}

fn set_default_user(distro_name: &str, uid: u32) -> Result<()> {
    if is_windows10() {
        log::debug!("Setting default user by the WSL API");
        // This crases on Windows 11. See the else block.
        unsafe {
            wsl::set_distribution_default_user(distro_name, uid)
                .with_context(|| "Failed to configure the default uid of the distribution.")?;
        }
    } else {
        log::debug!("Setting default user by the workaround for Windows11");
        // Assume it's Windows 11.
        // On Windows 11, calling set_distribution_default_user after registering distribution crashes for some reason.
        // However, as a workaround, executing it in another command works, though I don't know why :/
        // Conversely, this method crases on Windows 10 for another strange reason :/ :/
        let mut self_recurse = Command::new(
            std::env::current_exe().with_context(|| "Failed to get the current exe.")?,
        );
        self_recurse.args(["config", "--default-user"]);
        self_recurse.arg(uid.to_string());
        let status = self_recurse
            .status()
            .with_context(|| "self-recursion failed.")?;
        if !status.success() {
            bail!("Setting the default user failed.");
        }
    }
    Ok(())
}

fn is_windows10() -> bool {
    get_windows10_build_number()
        .map(|number| number < 22000)
        .map_err(|e| {
            log::debug!("Failed to get windows10 build number. {:?}", &e);
            e
        })
        .unwrap_or(false)
}

fn get_windows10_build_number() -> Result<u32> {
    let mut ver = Command::new("cmd");
    ver.args(["/C", "ver"]);
    let output = ver
        .output()
        .with_context(|| "Failed to get the output of `ver` command.")?
        .stdout;
    let version_string = String::from_utf8(output)
        .with_context(|| "The output of `ver` command is not an UTF-8 string.")?;

    let version_pattern =
        regex::Regex::new(r"\[Version 10\.0\.([0-9]*)\.").expect("this pattern should be valid");
    let captures = version_pattern
        .captures(version_string.trim())
        .ok_or_else(|| anyhow!("Unknown Windows version string pattern."))?;
    captures
        .get(1)
        .ok_or_else(|| anyhow!("version was not found"))?
        .as_str()
        .parse::<u32>()
        .with_context(|| "Failed to parse the version string as u32")
}
