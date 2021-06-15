use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

pub struct Container {
    root_fs: PathBuf,
}

impl Container {
    pub fn new<P: AsRef<Path>>(root_fs: P) -> Result<Self> {
        Ok(Container {
            root_fs: PathBuf::from(root_fs.as_ref()),
        })
    }

    pub fn launch(&mut self, init: Option<Vec<String>>) -> Result<()> {
        bail!("Not implemented")
    }
}
