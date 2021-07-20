use anyhow::Context;
use anyhow::Result;
use colored::*;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use libs::cli_ui::prompt_string;
use libs::cli_ui::{self};
use libs::distro_image::{self, DistroImageFetcher, DistroImageFetcherGen, DistroImageFile};
use libs::local_image::LocalDistroImage;
use libs::lxd_image::LxdDistroImageList;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Cursor, Read, Write};
use std::path::PathBuf;
use std::str::FromStr;
use structopt::StructOpt;
use strum::{EnumString, EnumVariantNames};
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
    #[structopt(subcommand)]
    pub command: Option<Subcommand>,
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
    default_user: String,
}

fn main() {
    let opts = Opts::from_args();
    init_logger(&opts.log_level);

    if let Err(err) = run(opts) {
        log::error!("{:?}", err);
    }
    let mut s = String::new();
    let _ = io::stdin().read_line(&mut s);
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
    match opts.command {
        None | Some(Subcommand::Install(_)) => {
            install_distro(opts)?;
        }
        _ => {}
    }
    Ok(())
}

fn install_distro(_opts: Opts) -> Result<()> {
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
    let lxd_root_tarxz = fetch_distro_image().with_context(|| "Failed to fetch a distro image.")?;
    let lxd_tar = tar::Archive::new(XzDecoder::new(lxd_root_tarxz));

    log::info!(
        "Unpacking and merging the given rootfs to the distrod rootfs. This may take a while..."
    );
    let tmp_dir = tempdir().with_context(|| "Failed to create a tempdir")?;
    let install_targz_path = merge_tar_archive(&tmp_dir, lxd_tar)?;

    log::info!("Installing the rootfs...");
    wsl.register_distribution(DISTRO_NAME, &install_targz_path)
        .with_context(|| "Failed to register the distribution.")?;
    log::info!("Done!");

    let user_name = prompt_string("Please input the new Linux user name. This doesn't have to be the same as your Windows user name.", "user name", None)?;
    let uid = add_user(&wsl, &user_name, tmp_dir);
    if uid.is_err() {
        log::warn!(
            "Adding user failed, but you can try adding a new user as the root after installation."
        );
    }

    wsl.launch_interactive(DISTRO_NAME, "/opt/distrod/distrod enable -d", true)
        .with_context(|| "Failed to initialize the rootfs image inside WSL.")?;
    if let Ok(uid) = uid {
        // This should be done after enable, because this changes the default user from root.
        wsl.configure_distribution(DISTRO_NAME, uid, wslapi::WSL_DISTRIBUTION_FLAGS::DEFAULT)
            .with_context(|| "Failed to configure the default uid of the distribution.")?;
    }
    log::info!("Installation of Distrod has completed.");
    wsl.launch_interactive(DISTRO_NAME, "", true)
        .with_context(|| "Failed to initialize the rootfs image inside WSL.")?;
    Ok(())
}

fn fetch_distro_image() -> Result<Box<dyn Read>> {
    let local_image_fetcher =
        || Ok(Box::new(LocalDistroImage::new(cli_ui::prompt_path)) as Box<dyn DistroImageFetcher>);
    let lxd_image_fetcher =
        || Ok(Box::new(LxdDistroImageList::default()) as Box<dyn DistroImageFetcher>);
    let fetchers = vec![
        Box::new(local_image_fetcher) as Box<DistroImageFetcherGen>,
        Box::new(lxd_image_fetcher) as Box<DistroImageFetcherGen>,
    ];
    let image = distro_image::fetch_image(fetchers, cli_ui::choose_from_list, 1)
        .with_context(|| "Failed to fetch the image list.")?;
    match image.image {
        DistroImageFile::Local(path) => {
            let file =
                File::open(&path).with_context(|| format!("Failed to open '{:?}'.", &path))?;
            Ok(Box::new(BufReader::new(file)) as Box<dyn Read>)
        }
        DistroImageFile::Url(url) => {
            log::info!("Downloading '{}'...", url);
            let client = reqwest::blocking::Client::builder().timeout(None).build()?;
            let response = client
                .get(&url)
                .send()
                .with_context(|| format!("Failed to download {}.", &url))?;
            let bytes = response.bytes().with_context(|| "Download failed.")?;
            log::info!("Download done.");
            Ok(Box::new(Cursor::new(bytes)))
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

fn add_user(wsl: &wslapi::Library, user_name: &str, tmp_dir: TempDir) -> Result<u32> {
    wsl.launch_interactive(
        DISTRO_NAME,
        format!(
            "( ( which adduser > /dev/null 2>&1 && adduser {} ) || ( useradd '{}' && while ! passwd {}; do : ; done  ) ) && echo '{} ALL=(ALL:ALL) ALL' >> /etc/sudoers",
            user_name, user_name, user_name, user_name
        ),
        true,
    )?;
    let uid_path = tmp_dir.path().join("uid");
    let uid_file = File::create(&uid_path).with_context(|| "Failed to create a temp file.")?;
    let status = wsl
        .launch(
            DISTRO_NAME,
            format!("id -u {}", &user_name),
            true,
            wslapi::Stdio::null(),
            uid_file,
            wslapi::Stdio::null(),
        )
        .with_context(|| "Failed to launch id command.")?
        .wait();
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
