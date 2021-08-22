use std::ffi::OsString;

use anyhow::{Context, Result};
use async_trait::async_trait;

pub type ListChooseFn = fn(list: DistroImageList) -> Result<Box<dyn DistroImageFetcher>>;
pub type PromptPath = fn(message: &str, default: Option<&str>) -> Result<OsString>;

#[async_trait]
pub trait DistroImageFetcher {
    fn get_name(&self) -> &str;
    async fn fetch(&self) -> Result<DistroImageList>;
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
    Local(OsString),
    Url(String),
}

pub type DistroImageFetcherGen = Box<dyn Fn() -> Result<Box<dyn DistroImageFetcher>> + Sync>;

pub async fn fetch_image(
    fetchers: Vec<DistroImageFetcherGen>,
    choose_from_list: ListChooseFn,
    default_index: usize,
) -> Result<DistroImage> {
    let mut distro_image_list = Box::new(DistroImageFetchersList {
        fetchers,
        default_index,
    }) as Box<dyn DistroImageFetcher>;
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

struct DistroImageFetchersList {
    fetchers: Vec<DistroImageFetcherGen>,
    default_index: usize,
}

#[async_trait]
impl DistroImageFetcher for DistroImageFetchersList {
    fn get_name(&self) -> &str {
        "Image candidates"
    }

    async fn fetch(&self) -> Result<DistroImageList> {
        let fetchers: Result<Vec<Box<_>>> = self.fetchers.iter().map(|f| f()).collect();
        Ok(DistroImageList::Fetcher(
            "the way to get a distro image".to_owned(),
            fetchers?,
            DefaultImageFetcher::Index(self.default_index),
        ))
    }
}

pub async fn download_file_with_progress<F, W>(
    url: &str,
    progress_bar_builder: F,
    out: &mut W,
) -> Result<()>
where
    F: FnOnce(u64) -> indicatif::ProgressBar,
    W: std::io::Write,
{
    let client = reqwest::Client::builder().build()?;
    let mut response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("Failed to download {}.", &url))?;
    let total_size = response
        .content_length()
        .with_context(|| format!("Failed to get the content length of {}.", &url))?;

    let progress_bar = progress_bar_builder(total_size);
    let mut downloaded_size = 0;

    while let Some(bytes) = response.chunk().await? {
        out.write_all(&bytes)?;
        downloaded_size = std::cmp::min(downloaded_size + bytes.len(), total_size as usize);
        progress_bar.set_position(downloaded_size as u64);
    }

    progress_bar.finish();
    Ok(())
}
