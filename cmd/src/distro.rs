use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::container::Container;

pub struct Distro {
    container: Container,
}

impl Distro {
    pub fn get_installed_distro<P: AsRef<Path>>(rootfs: P) -> Result<Option<Distro>> {
        let path_buf = PathBuf::from(rootfs.as_ref());
        if !path_buf.is_dir() {
            return Ok(None);
        }
        let container =
            Container::new(&path_buf).with_context(|| "Failed to initialize a container")?;
        Ok(Some(Distro { container }))
    }

    pub fn launch(&mut self) -> Result<()> {
        self.container
            .launch(None)
            .with_context(|| "Failed to launch a container.")
    }
}
