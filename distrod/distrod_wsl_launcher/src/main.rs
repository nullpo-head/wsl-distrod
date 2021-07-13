use anyhow::Result;
use anyhow::{anyhow, bail, Context};
use colored::*;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use lxd_image::{DistroImageFetcher, DistroImageFile};
use std::fs::{self, File};
use std::io::{self, BufWriter, Cursor, Read, Write};
use std::path::Path;
use std::str::FromStr;
use structopt::StructOpt;
use strum::{EnumString, EnumVariantNames};
use tempdir::TempDir;
use wslapi::Library as WslApi;
use xz2::read::XzDecoder;

use crate::lxd_image::{DefaultImageFetcher, DistroImageList};

mod lxd_image;

static DISTRO_NAME: &'static str = "Distrod";

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
            install_distro_demo(opts)?;
        }
        _ => {}
    }
    Ok(())
}

fn install_distro(_opts: Opts) -> Result<()> {
    let wsl = WslApi::new()
        .with_context(|| "Failed to retrieve WSL API. Have you enabled the WSL2 feature?")?;
    let tmp_dir = TempDir::new("distrod").with_context(|| "Failed to create a tempdir")?;
    log::info!("tmp_dir: {:?}", tmp_dir.path());
    //let rootfs =
    //    download_lxd_image(tmp_dir.path()).with_context(|| "Failed to download an LXD image")?;
    let rootfs = fs::copy(
        r"C:\Users\abctk\Downloads\rootfs.tar.xz",
        tmp_dir.path().join("rootfs.tar.xz"),
    )
    .with_context(|| "Failed to copy file.")?;
    let rootfs_tarxz_path = tmp_dir.path().join("rootfs.tar.xz");
    let rootfs_tarxz = File::open(&rootfs_tarxz_path)
        .with_context(|| format!("Failed to open {:?}.", &rootfs_tarxz_path))?;
    log::info!("download path: {:?}", tmp_dir.path().join("rootfs.tar.xz"));

    let distrod_root = std::include_bytes!("../resources/distrod_root.tar.gz");
    let distrod_root_targz_path = tmp_dir.path().join("install.tar.gz");
    let mut distrod_root_targz = File::create(&distrod_root_targz_path)?;
    distrod_root_targz.write_all(distrod_root)?;
    drop(distrod_root_targz); // To avoid a warning about the file still being open.
    log::info!("Unpacking the root fs. This may take time...");
    wsl.register_distribution(DISTRO_NAME, &distrod_root_targz_path)
        .with_context(|| "Failed to register the distribution.")?;
    log::info!("done");

    let pp = &tmp_dir
        .path()
        .join("rootfs.tar.xz")
        .to_str()
        .unwrap()
        .to_owned();
    let p = Path::new(pp);
    log::info!("{:?} exists: {:?}", &p, p.exists());
    let mut fout = File::create(tmp_dir.path().join("fout"))?;
    let mut ferr = File::create(tmp_dir.path().join("ferr"))?;
    let proc = wsl
        .launch(DISTRO_NAME, "install", true, rootfs_tarxz, fout, ferr)
        .with_context(|| "Failed to initialize the rootfs image inside WSL.")?;
    std::thread::sleep(std::time::Duration::from_secs(5));
    log::info!("done2: exit: {:?}", proc.wait()?.code());
    //log::info!("done2: exit: {:?}", proc);
    let mut sout = String::new();
    let mut serr = String::new();
    File::read_to_string(&mut File::open(tmp_dir.path().join("fout"))?, &mut sout);
    File::read_to_string(&mut File::open(tmp_dir.path().join("ferr"))?, &mut serr);
    log::info!("fout: {}, ferr: {}", sout, serr);
    Ok(())
}

fn install_distro2(_opts: Opts) -> Result<()> {
    let wsl = WslApi::new()
        .with_context(|| "Failed to retrieve WSL API. Have you enabled the WSL2 feature?")?;
    let tmp_dir = TempDir::new("distrod").with_context(|| "Failed to create a tempdir")?;

    let lxd_root_tarxz = fetch_lxd_image().with_context(|| "Failed to download an LXD image")?;
    log::info!("Unpacking the downloaded fs. This may take time...");
    let mut lxd_tar = XzDecoder::new(lxd_root_tarxz);
    let distrod_targz = std::include_bytes!("../resources/distrod_root.tar.gz");
    let mut distrod_tar = GzDecoder::new(std::io::Cursor::new(distrod_targz));
    let mut rootfs_tar = vec![];
    lxd_tar.read_to_end(&mut rootfs_tar)?;
    distrod_tar.read_to_end(&mut rootfs_tar)?;

    log::info!("Packing the new rootfs...");
    let install_targz_path = tmp_dir.path().join("install.tar.gz");
    let mut install_targz = BufWriter::new(File::create(&install_targz_path)?);
    let mut encoder = GzEncoder::new(install_targz, flate2::Compression::default());
    encoder.write_all(&rootfs_tar);
    drop(encoder); // To avoid a warning about the file still being open.

    log::info!("Installing the rootfs...");
    wsl.register_distribution(DISTRO_NAME, &install_targz_path)
        .with_context(|| "Failed to register the distribution.")?;
    log::info!("done");
    std::thread::sleep(std::time::Duration::from_secs(5));
    let mut fout = File::create(tmp_dir.path().join("fout"))?;
    let mut ferr = File::create(tmp_dir.path().join("ferr"))?;
    let fin = "";
    let proc = wsl
        .launch(DISTRO_NAME, "echo $0", true, fin, fout, ferr)
        .with_context(|| "Failed to initialize the rootfs image inside WSL.")?;
    log::info!("done2: exit: {:?}", proc.wait()?.code());
    //log::info!("done2: exit: {:?}", proc);
    let mut sout = String::new();
    let mut serr = String::new();
    File::read_to_string(&mut File::open(tmp_dir.path().join("fout"))?, &mut sout);
    File::read_to_string(&mut File::open(tmp_dir.path().join("ferr"))?, &mut serr);
    log::info!("fout: {}, ferr: {}", sout, serr);
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
    let response =
        reqwest::blocking::get(&url).with_context(|| format!("Failed to download {}.", &url))?;
    log::info!("Download done.");
    Ok(Cursor::new(response.bytes()?))
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
                choice = choice.trim_end().to_owned();
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
