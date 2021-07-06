use anyhow::{anyhow, Context, Result};
use chrono::NaiveDateTime;

pub trait DistroImageFetcher {
    fn get_name(&self) -> &str;
    fn fetch(&self) -> Result<DistroImageList>;
}

pub enum DistroImageList {
    Fetcher(
        String,
        Vec<Box<dyn DistroImageFetcher>>,
        DefaultImageFetcher,
    ),
    Image(DistroImage),
}

#[derive(Debug)]
pub enum DefaultImageFetcher {
    Index(usize),
    Name(String),
}

#[derive(Debug)]
pub struct DistroImage {
    pub name: String,
    pub image: DistroImageFile,
}

#[derive(Debug)]
pub enum DistroImageFile {
    Local(String),
    Url(String),
}

static LINUX_CONTAINERS_ORG_BASE: &str = "https://uk.images.linuxcontainers.org/";

pub fn fetch_lxd_image(
    choose_from_list: fn(list: DistroImageList) -> Result<Box<dyn DistroImageFetcher>>,
) -> Result<DistroImage> {
    let mut distro_image_list = Box::new(LxdDistroImageList {}) as Box<dyn DistroImageFetcher>;
    loop {
        let fetched_image_list = distro_image_list.fetch()?;
        match fetched_image_list {
            DistroImageList::Fetcher(_, _, _) => {
                distro_image_list = choose_from_list(fetched_image_list)?;
            }
            DistroImageList::Image(image) => {
                return Ok(image);
            }
        }
    }
}

struct LxdDistroImageList;

impl DistroImageFetcher for LxdDistroImageList {
    fn get_name(&self) -> &str {
        "LXD image"
    }

    fn fetch(&self) -> Result<DistroImageList> {
        let distros: Vec<_> = fetch_apache_file_list("images/")
            .map(|links| {
                links
                    .into_iter()
                    .map(|link| {
                        Box::new(LxdDistroVersionList {
                            name: link.name,
                            version_list_url: format!("images/{}", link.url),
                        }) as Box<dyn DistroImageFetcher>
                    })
                    .collect()
            })
            .with_context(|| "Failed to parse the distro image list of the LXD image server.")?;
        Ok(DistroImageList::Fetcher(
            "a LXD image".to_owned(),
            distros,
            DefaultImageFetcher::Name("ubuntu".to_owned()),
        ))
    }
}

#[derive(Debug)]
struct LxdDistroVersionList {
    name: String,
    version_list_url: String,
}

impl DistroImageFetcher for LxdDistroVersionList {
    fn get_name(&self) -> &str {
        self.name.as_str()
    }

    fn fetch(&self) -> Result<DistroImageList> {
        let mut links = fetch_apache_file_list(&self.version_list_url)
            .with_context(|| "Failed to parse the version list.")?;
        links.sort_by(|a, b| a.last_modified.cmp(&b.last_modified));
        let versions: Vec<_> = links
            .into_iter()
            .map(|link| {
                Box::new(LxdDistroVersion {
                    distro_name: self.name.clone(),
                    version_name: link.name,
                    platform_list_url: format!("{}{}", self.version_list_url, link.url),
                }) as Box<dyn DistroImageFetcher>
            })
            .collect();
        let default = match self.get_name() {
            "ubuntu" => DefaultImageFetcher::Name("focal".to_owned()),
            _ => DefaultImageFetcher::Index(versions.len() - 1),
        };
        Ok(DistroImageList::Fetcher(
            "a version".to_owned(),
            versions,
            default,
        ))
    }
}

#[derive(Debug)]
struct LxdDistroVersion {
    distro_name: String,
    version_name: String,
    platform_list_url: String,
}

impl DistroImageFetcher for LxdDistroVersion {
    fn get_name(&self) -> &str {
        self.version_name.as_str()
    }

    fn fetch(&self) -> Result<DistroImageList> {
        let mut dates = fetch_apache_file_list(&format!("{}amd64/cloud", &self.platform_list_url))
            .with_context(|| format!("Failed to get the image for amd64/cloud. Perhaps '{}amd64/cloud' is not found?", &self.platform_list_url))?;
        dates.sort_by(|a, b| b.last_modified.cmp(&a.last_modified));
        let latest = &dates[0];
        let rootfs_url = format!(
            "{}{}amd64/cloud/{}rootfs.tar.xz",
            &LINUX_CONTAINERS_ORG_BASE, &self.platform_list_url, latest.url
        );
        Ok(DistroImageList::Image(DistroImage {
            name: format!("{}-{}", &self.distro_name, &self.version_name),
            image: DistroImageFile::Url(rootfs_url),
        }))
    }
}

fn fetch_apache_file_list(relative_url: &str) -> Result<Vec<FileOnApache>> {
    let url = LINUX_CONTAINERS_ORG_BASE.to_owned() + relative_url;
    let date_selector =
        scraper::Selector::parse("body > table > tbody > tr > td:nth-child(3)").unwrap();
    let a_link_selector =
        scraper::Selector::parse("body > table > tbody > tr > td:nth-child(2) > a").unwrap();
    log::info!("Fetching from linuxcontainers.org...");
    let apache_file_list_body = reqwest::blocking::get(&url)
        .with_context(|| format!("Failed to fetch {}", &url))?
        .text()?;
    let doc = scraper::Html::parse_document(&apache_file_list_body);
    let dates: Vec<_> = doc.select(&date_selector).collect();
    let a_links: Vec<_> = doc.select(&a_link_selector).collect();
    let links: Result<Vec<_>> = a_links[1..]
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let name = a
                .text()
                .next()
                .ok_or_else(|| anyhow!(format!("No text in a tag: {:#?}.", &a)))?;
            let name = name.trim_end_matches('/').to_owned();
            let url = a
                .value()
                .attr("href")
                .ok_or_else(|| anyhow!(format!("a tag has No href. {:#?}", &a)))?
                .to_owned();
            let url = url.trim_start_matches("./").to_owned();
            Ok(FileOnApache {
                name,
                url,
                last_modified: NaiveDateTime::parse_from_str(
                    &dates[i + 1]
                        .text()
                        .next()
                        .ok_or_else(|| {
                            anyhow!(format!("Last modified time is not found. Tag: {:#?}.", &a))
                        })?
                        .trim_end(),
                    "%Y-%m-%d %H:%M",
                )
                .with_context(|| {
                    format!(
                        "Failed to parse the date time.: {:#?}",
                        dates[i + 1].text().next()
                    )
                })?,
            })
        })
        .collect();
    links.with_context(|| "Failed to parse the Apache file list page. Maybe the page is updated?")
}

#[derive(Debug)]
struct FileOnApache {
    name: String,
    url: String,
    last_modified: NaiveDateTime,
}
