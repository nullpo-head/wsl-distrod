use anyhow::Result;

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
