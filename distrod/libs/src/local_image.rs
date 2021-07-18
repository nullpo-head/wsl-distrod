use std::path::{Path, PathBuf};

use crate::distro_image::{
    DistroImage, DistroImageFetcher, DistroImageFile, DistroImageList, PromptPath,
};
use anyhow::Result;

pub struct LocalDistroImage {
    prompt_path: PromptPath,
}

impl LocalDistroImage {
    pub fn new(prompt_path: PromptPath) -> LocalDistroImage {
        LocalDistroImage { prompt_path }
    }
}

impl DistroImageFetcher for LocalDistroImage {
    fn get_name(&self) -> &str {
        "Use a local tar.xz file"
    }

    fn fetch(&self) -> Result<DistroImageList> {
        let mut path;
        loop {
            path = (self.prompt_path)("Please input the path to your .tar.xz image file.", None)?;
            if !path.to_string_lossy().ends_with(".tar.xz") {
                log::error!("The path must end with '.tar.xz'");
                continue;
            }
            if !Path::new(&path).exists() {
                log::error!("The path does not exist.");
                continue;
            }
            break;
        }
        let path_buf = PathBuf::from(&path);
        Ok(DistroImageList::Image(DistroImage {
            name: path_buf
                .file_stem()
                .expect("File name exists")
                .to_string_lossy()
                .into_owned(),
            image: DistroImageFile::Local(path),
        }))
    }
}
