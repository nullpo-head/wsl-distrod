use crate::distro_image::{
    DefaultImageFetcher, DistroImage, DistroImageFetcher, DistroImageFile, DistroImageList,
    ListChooseFn,
};
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use chrono::NaiveDateTime;

static LINUX_CONTAINERS_ORG_BASE: &str = "https://images.linuxcontainers.org/";

pub async fn fetch_container_org_image(choose_from_list: ListChooseFn<'_>) -> Result<DistroImage> {
    let mut distro_image_list = Box::new(ContainerOrgImageList {}) as Box<dyn DistroImageFetcher>;
    loop {
        let fetched_image_list = distro_image_list.fetch().await?;
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

#[derive(Default)]
pub struct ContainerOrgImageList;

#[async_trait]
impl DistroImageFetcher for ContainerOrgImageList {
    fn get_name(&self) -> &str {
        "Download an image from linxcontainers.org"
    }

    async fn fetch(&self) -> Result<DistroImageList> {
        let distros: Vec<_> = fetch_apache_file_list("images/")
            .await
            .map(|links| {
                links
                    .into_iter()
                    .map(|link| {
                        Box::new(ContainerOrgDistroVersionList {
                            name: link.name,
                            version_list_url: format!("images/{}", link.url),
                        }) as Box<dyn DistroImageFetcher>
                    })
                    .collect()
            })
            .with_context(|| {
                "Failed to parse the distro image list of the linuxcontainer.org image server."
            })?;

        Ok(DistroImageList::Fetcher(
            "a linuxcontainers.org image".to_owned(),
            distros,
            DefaultImageFetcher::Name("ubuntu".to_owned()),
        ))
    }
}

#[derive(Debug)]
pub struct ContainerOrgDistroVersionList {
    name: String,
    version_list_url: String,
}

#[async_trait]
impl DistroImageFetcher for ContainerOrgDistroVersionList {
    fn get_name(&self) -> &str {
        self.name.as_str()
    }

    async fn fetch(&self) -> Result<DistroImageList> {
        let mut links = fetch_apache_file_list(&self.version_list_url)
            .await
            .with_context(|| "Failed to parse the version list.")?;
        links.sort_by(|a, b| a.last_modified.cmp(&b.last_modified));
        let versions: Vec<_> = links
            .into_iter()
            .map(|link| {
                Box::new(ContainerOrgDistroVersion {
                    distro_name: self.name.clone(),
                    version_name: link.name,
                    platform_list_url: format!("{}{}", self.version_list_url, link.url),
                }) as Box<dyn DistroImageFetcher>
            })
            .collect();
        let default = match self.get_name() {
            "ubuntu" => DefaultImageFetcher::Name("focal".to_owned()),
            "debian" => DefaultImageFetcher::Name("bullseye".to_owned()),
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
pub struct ContainerOrgDistroVersion {
    distro_name: String,
    version_name: String,
    platform_list_url: String,
}

#[async_trait]
impl DistroImageFetcher for ContainerOrgDistroVersion {
    fn get_name(&self) -> &str {
        self.version_name.as_str()
    }

    async fn fetch(&self) -> Result<DistroImageList> {
        let variant = match self.distro_name.as_str() {
            "gentoo" => "amd64/systemd",
            _ => "amd64/default",
        };
        let mut dates = fetch_apache_file_list(&format!("{}{}", &self.platform_list_url, variant))
            .await
            .with_context(|| {
                format!(
                    "Failed to get the image for {}. Perhaps '{}{}' is not found?",
                    variant, &self.platform_list_url, variant
                )
            })?;
        dates.sort_by(|a, b| b.last_modified.cmp(&a.last_modified));
        let latest = &dates[0];
        let rootfs_url = format!(
            "{}{}{}/{}rootfs.tar.xz",
            &LINUX_CONTAINERS_ORG_BASE, &self.platform_list_url, variant, latest.url
        );
        Ok(DistroImageList::Image(DistroImage {
            name: format!("{}-{}", &self.distro_name, &self.version_name),
            image: DistroImageFile::Url(rootfs_url),
        }))
    }
}

async fn fetch_apache_file_list(relative_url: &str) -> Result<Vec<FileOnApache>> {
    let url = LINUX_CONTAINERS_ORG_BASE.to_owned() + relative_url;
    let date_selector =
        scraper::Selector::parse("body > table > tbody > tr > td:nth-child(3)").unwrap();
    let a_link_selector =
        scraper::Selector::parse("body > table > tbody > tr > td:nth-child(2) > a").unwrap();
    log::info!("Fetching from linuxcontainers.org...");
    let apache_file_list_body = reqwest::get(&url)
        .await
        .with_context(|| format!("Failed to fetch {}", &url))?
        .text()
        .await
        .with_context(|| format!("Failed to get the text of {}", &url))?;
    let doc = scraper::Html::parse_document(&apache_file_list_body);
    let dates: Vec<_> = doc.select(&date_selector).collect();
    let a_links: Vec<_> = doc.select(&a_link_selector).collect();
    let links = a_links
        .iter()
        .skip(1)
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
                    dates[i + 1]
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
        .collect::<Result<Vec<_>>>()
        .with_context(|| "Failed to parse the Apache file list page. Maybe the page is updated?")?;
    if links.is_empty() {
        bail!("{:?} is not available", &relative_url);
    }
    Ok(links)
}

#[derive(Debug)]
struct FileOnApache {
    name: String,
    url: String,
    last_modified: NaiveDateTime,
}
