use anyhow::Result;
use anyhow::{bail, Context};
use colored::*;
use common::cli_ui::choose_from_list;
use common::distro_image::DistroImageFile;
use common::lxd_image;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use std::fs::{self, File};
use std::io::{self, BufWriter, Cursor, Read, Write};
use std::path::Path;
use std::str::FromStr;
use structopt::StructOpt;
use strum::{EnumString, EnumVariantNames};
use tempdir::TempDir;
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
    let tmp_dir = TempDir::new("distrod").with_context(|| "Failed to create a tempdir")?;

    let lxd_root_tarxz = fetch_lxd_image().with_context(|| "Failed to download an LXD image")?;
    let mut lxd_tar = tar::Archive::new(XzDecoder::new(lxd_root_tarxz));
    let distrod_targz = std::include_bytes!("../resources/distrod_root.tar.gz");
    let mut distrod_tar = tar::Archive::new(GzDecoder::new(std::io::Cursor::new(distrod_targz)));

    let install_targz_path = tmp_dir.path().join("install.tar.gz");
    let mut install_targz =
        BufWriter::new(File::create(&install_targz_path).with_context(|| {
            format!("Failed to create a new file at '{:?}'.", install_targz_path)
        })?);
    let mut encoder = GzEncoder::new(install_targz, flate2::Compression::default());

    log::info!(
        "Unpacking and mergeing the downloaded rootfs to the distrod rootfs. This may take time..."
    );
    let mut builder = tar::Builder::new(encoder);
    append_tar_archive(&mut builder, &mut lxd_tar)
        .with_context(|| "Failed to merge the downloaded LXD image.")?;
    append_tar_archive(&mut builder, &mut distrod_tar)
        .with_context(|| "Failed to merge the downloaded LXD image.")?;

    log::info!("Installing the rootfs...");
    wsl.register_distribution(DISTRO_NAME, &install_targz_path)
        .with_context(|| "Failed to register the distribution.")?;
    log::info!("Done!");
    let proc = wsl
        .launch_interactive(
            DISTRO_NAME,
            "/opt/distrod/distrod exec -r / /bin/bash",
            true,
        )
        .with_context(|| "Failed to initialize the rootfs image inside WSL.")?;
    Ok(())
}

fn download_lxd_image(download_dir: &Path) -> Result<File> {
    log::info!("pwd: {:?}", std::env::current_dir()?);
    log::info!("exist: {:?}", Path::new("install.tar.gz").exists());
    let image = lxd_image::fetch_lxd_image(choose_from_list)
        .with_context(|| "Failed to fetch the lxd image list.")?;
    log::info!("Unpacking...");
    let mut tar_xz_file = File::create(download_dir.join("rootfs.tar.xz")).with_context(|| {
        format!(
            "Failed to create a file for download. {:?}",
            download_dir.join(&image.name)
        )
    })?;
    let mut tar_xz_cont = match image.image {
        DistroImageFile::Local(_) => bail!("fetch_lxd_image should not return a Local image."),
        DistroImageFile::Url(url) => {
            log::info!("Downloading '{}'...", url);
            let client = reqwest::blocking::Client::builder().timeout(None).build()?;
            let response = client
                .get(&url)
                .send()
                .with_context(|| format!("Failed to download {}.", &url))?;
            Box::new(std::io::Cursor::new(response.bytes()?)) as Box<dyn Read>
        }
    };
    io::copy(&mut tar_xz_cont, &mut tar_xz_file)?;
    log::info!("Download is done!");
    Ok(tar_xz_file)
}

fn download_lxd_image_demo(download_dir: &Path) -> Result<()> {
    let image = lxd_image::fetch_lxd_image(choose_from_list)
        .with_context(|| "Failed to fetch the lxd image list.")?;
    let mut tar_xz_cont = match image.image {
        DistroImageFile::Local(_) => bail!("fetch_lxd_image should not return a Local image."),
        DistroImageFile::Url(url) => {
            log::info!("Downloading '{}'...", url);
            std::thread::sleep(std::time::Duration::from_secs(5));
        }
    };
    Ok(())
}

fn install_distro_demo(_opts: Opts) -> Result<()> {
    let wsl = WslApi::new()
        .with_context(|| "Failed to retrieve WSL API. Have you enabled the WSL2 feature?")?;
    if wsl.is_distribution_registered("distrod") {
        let proc = wsl
            .launch_interactive(
                DISTRO_NAME,
                "/opt/distrod/distrod exec -r / /bin/bash",
                true,
            )
            .with_context(|| "Failed to initialize the rootfs image inside WSL.")?;
        return Ok(());
    }
    let tmp_dir = TempDir::new("distrod").with_context(|| "Failed to create a tempdir")?;
    download_lxd_image_demo(tmp_dir.path()).with_context(|| "Failed to download an LXD image")?;
    wsl.register_distribution(
        DISTRO_NAME,
        &Path::new(r"C:\Users\abctk\Downloads\demorootfs.tar.gz"),
    )
    .with_context(|| "Failed to register the distribution.")?;
    let proc = wsl
        .launch_interactive(
            DISTRO_NAME,
            "/opt/distrod/distrod exec -r / /bin/bash",
            true,
        )
        .with_context(|| "Failed to initialize the rootfs image inside WSL.")?;
    Ok(())
}

fn fetch_lxd_image() -> Result<Cursor<bytes::Bytes>> {
    let image = lxd_image::fetch_lxd_image(choose_from_list)
        .with_context(|| "Failed to fetch the lxd image list.")?;
    let url = match image.image {
        DistroImageFile::Local(_) => bail!("fetch_lxd_image should not return a Local image."),
        DistroImageFile::Url(url) => url,
    };
    log::info!("Downloading '{}'...", url);
    let mut client = reqwest::blocking::Client::builder().timeout(None).build()?;
    let response = client
        .get(&url)
        .send()
        .with_context(|| format!("Failed to download {}.", &url))?;
    let bytes = response.bytes().with_context(|| "Download failed.")?;
    log::info!("Download done.");
    Ok(Cursor::new(bytes))
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
                .with_context(|| format!("Failed to read the data of an entry: {:?}.", &path));
        }
        let header = entry.header();
        builder
            .append(&header, Cursor::new(data))
            .with_context(|| format!("Failed to add an entry to an archive. {:?}", path))?;
    }
    Ok(())
}
