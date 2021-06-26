use anyhow::bail;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::linux::fs::MetadataExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::container::Container;

const DISTRO_RUN_INFO_PATH: &str = "/var/run/distrod.json";

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

    pub fn get_running_distro() -> Result<Option<Distro>> {
        let run_info = get_distro_run_info_file(false, false)
            .with_context(|| format!("Failed to open the distro run info file."))?;
        if run_info.is_none() {
            return Ok(None);
        }
        let mut run_info = run_info.unwrap();
        let mut json = String::new();
        run_info.read_to_string(&mut json)?;

        let container = Container::get_running_container_from_json(&json)
            .with_context(|| format!("Failed to import running container info."))?;
        if container.is_none() {
            return Ok(None);
        }
        let container = container.unwrap();

        Ok(Some(Distro { container }))
    }

    pub fn launch(&mut self) -> Result<()> {
        self.container
            .launch(None, "/mnt/distrod_root")
            .with_context(|| "Failed to launch a container.")?;
        self.export_run_info()?;
        std::thread::sleep(std::time::Duration::from_secs(60));
        Ok(())
    }

    pub fn exec_command<'a, I, S, T, P>(&self, command: S, args: I, wd: Option<P>) -> Result<u32>
    where 
        I: IntoIterator<Item = &'a T>,
        S: AsRef<str>,
        T: AsRef<str> + 'a,
        P: AsRef<Path>,
    {
        Ok(0)
    }

    fn export_run_info(&self) -> Result<()> {
        if let Ok(Some(_)) = get_distro_run_info_file(false, false) {
            fs::remove_file(&DISTRO_RUN_INFO_PATH)
                .with_context(|| "Failed to remove the existing run info file.")?;
        }
        let mut file = get_distro_run_info_file(true, true)
            .with_context(|| "Failed to create a run info file.")?
            .expect("[BUG] get_distro_run_info_file shuold return Some when create:true");
        file.write_all(self.container.run_info_as_json()?.as_bytes())
            .with_context(|| "Failed to write to a distro run info file.")?;
        Ok(())
    }
}

fn get_distro_run_info_file(create: bool, write: bool) -> Result<Option<File>> {
    let mut json = fs::OpenOptions::new();
    json.read(true);
    if create {
        json.create(true);
    }
    if write {
        json.write(true);
    }
    let json = json.open(DISTRO_RUN_INFO_PATH);
    if let Err(ref error) = json {
        if error.raw_os_error() == Some(nix::errno::Errno::ENOENT as i32) {
            return Ok(None);
        }
    }
    let json = json.with_context(|| format!("Failed to open the run info file of the distro."))?;
    let metadata = json.metadata()?;
    if metadata.st_uid() != 0 || metadata.st_gid() != 0 {
        bail!("The run info file of the distrod is unsafe, which is owned by a non-root user/group.");
    }
    Ok(Some(json))
}
