use std::ffi::OsString;

use anyhow::Result;

pub type ListChooseFn = fn(list: DistroImageList) -> Result<Box<dyn DistroImageFetcher>>;
pub type PromptPath = fn(message: &str, default: Option<&str>) -> Result<OsString>;

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
    Local(OsString),
    Url(String),
}

pub type DistroImageFetcherGen = dyn Fn() -> Result<Box<dyn DistroImageFetcher>>;

pub fn fetch_image(
    fetchers: Vec<Box<DistroImageFetcherGen>>,
    choose_from_list: ListChooseFn,
    default_index: usize,
) -> Result<DistroImage> {
    let mut distro_image_list = Box::new(DistroImageFetchersList {
        fetchers,
        default_index,
    }) as Box<dyn DistroImageFetcher>;
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

struct DistroImageFetchersList {
    fetchers: Vec<Box<DistroImageFetcherGen>>,
    default_index: usize,
}

impl DistroImageFetcher for DistroImageFetchersList {
    fn get_name(&self) -> &str {
        "Image candidates"
    }

    fn fetch(&self) -> Result<DistroImageList> {
        let fetchers: Result<Vec<Box<_>>> = self.fetchers.iter().map(|f| f()).collect();
        Ok(DistroImageList::Fetcher(
            "the way to get a distro image".to_owned(),
            fetchers?,
            DefaultImageFetcher::Index(self.default_index),
        ))
    }
}
