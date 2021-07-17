use anyhow::{anyhow, bail, Context, Result};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::linux::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::container::Container;
use crate::mount_info::get_mount_entries;
pub use crate::multifork::Waiter;

const DISTRO_RUN_INFO_PATH: &str = "/var/run/distrod.json";
const DISTRO_OLD_ROOT_PATH: &str = "/mnt/distrod_root";

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
            .with_context(|| "Failed to open the distro run info file.")?;
        if run_info.is_none() {
            return Ok(None);
        }
        let mut run_info = run_info.unwrap();
        let mut json = String::new();
        run_info.read_to_string(&mut json)?;

        let container = Container::get_running_container_from_json(&json)
            .with_context(|| "Failed to import running container info.")?;
        if container.is_none() {
            return Ok(None);
        }
        let container = container.unwrap();

        Ok(Some(Distro { container }))
    }

    pub fn is_inside_running_distro() -> bool {
        let mounts = get_mount_entries();
        if mounts.is_err() {
            return true;
        }
        let mounts = mounts.unwrap();
        mounts
            .iter()
            .any(|entry| entry.path.starts_with(DISTRO_OLD_ROOT_PATH))
    }

    pub fn launch(&mut self) -> Result<()> {
        let _ = self
            .container
            .launch(None, DISTRO_OLD_ROOT_PATH)
            .with_context(|| "Failed to launch a container.")?;
        self.export_run_info()?;
        Ok(())
    }

    pub fn exec_command<I, S, T1, T2, P>(
        &self,
        command: S,
        args: I,
        wd: Option<P>,
        arg0: Option<T2>,
        ids: Option<(u32, u32)>,
    ) -> Result<Waiter>
    where
        I: IntoIterator<Item = T1>,
        S: AsRef<OsStr>,
        T1: AsRef<OsStr>,
        T2: AsRef<OsStr>,
        P: AsRef<Path>,
    {
        log::debug!("Distro::exec_command.");
        self.container
            .exec_command(command, args, wd, arg0, ids)
            .with_context(|| "Failed to exec command in the container")
    }

    pub fn stop(self, sigkill: bool) -> Result<()> {
        self.container.stop(sigkill)
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

pub fn initialize_distro_rootfs<P: AsRef<Path>>(
    path: P,
    overwrites_potential_userfiles: bool,
) -> Result<()> {
    let metadata = fs::metadata(path.as_ref())?;
    if !metadata.is_dir() {
        bail!("The given path is not a directory: '{:?}'", path.as_ref());
    }

    // Remove systemd network configurations
    for path in glob::glob(
        path.as_ref()
            .join("etc/systemd/network/*.network")
            .as_os_str()
            .to_str()
            .ok_or_else(|| anyhow!("Failed to convert systemd network file paths."))?,
    )? {
        let path = path?;
        fs::remove_file(&path).with_context(|| format!("Failed to remove '{:?}'.", &path))?;
    }

    // echo hostname to /etc/hostname
    let hostname_path = path.as_ref().join("etc/hostname");
    let mut hostname_buf = vec![0; 64];
    let hostname =
        nix::unistd::gethostname(&mut hostname_buf).with_context(|| "Failed to get hostname.")?;
    fs::write(&hostname_path, hostname.to_str()?.as_bytes())
        .with_context(|| format!("Failed to write hostname to '{:?}'.", &hostname_path))?;

    // Remove /etc/resolv.conf
    if overwrites_potential_userfiles {
        let resolv_conf_path = path.as_ref().join("etc/resolv.conf");
        fs::remove_file(path.as_ref().join(&resolv_conf_path))
            .with_context(|| format!("Failed to remove '{:?}'.", &resolv_conf_path))?;
        // Touch /etc/resolv.conf so that WSL over-writes it or we can do bind-mount on it
        File::create(&resolv_conf_path)
            .with_context(|| format!("Failed to touch '{:?}'", &resolv_conf_path))?;
    }
    Ok(())
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
    let json = json.with_context(|| "Failed to open the run info file of the distro.")?;
    let metadata = json.metadata()?;
    if metadata.st_uid() != 0 || metadata.st_gid() != 0 {
        bail!(
            "The run info file of the distrod is unsafe, which is owned by a non-root user/group."
        );
    }
    Ok(Some(json))
}
