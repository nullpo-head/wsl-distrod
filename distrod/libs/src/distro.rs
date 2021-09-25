use anyhow::{anyhow, bail, Context, Result};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Write};
use std::os::linux::fs::MetadataExt;
use std::os::unix::prelude::{CommandExt, OsStrExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::container::{Container, ContainerLauncher, ContainerPath, HostPath};
use crate::distrod_config::{self, DistrodConfig};
use crate::envfile::EnvFile;
use crate::mount_info::get_mount_entries;
pub use crate::multifork::Waiter;
use crate::passwd::Credential;
use crate::procfile::ProcFile;
use crate::systemdunit::SystemdUnitDisabler;
use crate::wsl_interop::collect_wsl_env_vars;
use serde::{Deserialize, Serialize};

const DISTRO_RUN_INFO_PATH: &str = "/var/run/distrod.json";
const DISTRO_OLD_ROOT_PATH: &str = "/mnt/distrod_root";

#[derive(Debug, Clone, Default)]
pub struct DistroLauncher {
    rootfs: Option<PathBuf>,
}

impl DistroLauncher {
    pub fn new() -> Self {
        DistroLauncher::default()
    }

    pub fn get_running_distro() -> Result<Option<Distro>> {
        let run_info_file = get_distro_run_info_file(false, false)
            .with_context(|| "Failed to open the distro run info file.")?;
        if run_info_file.is_none() {
            return Ok(None);
        }
        let run_info = BufReader::new(run_info_file.unwrap());
        let run_info: DistroRunInfo = serde_json::from_reader(run_info)?;
        if ProcFile::from_pid(run_info.init_pid)?.is_none() {
            return Ok(None);
        }
        Ok(Some(Distro {
            rootfs: HostPath::new(run_info.rootfs)?,
            container: ContainerLauncher::from_pid(run_info.init_pid)?,
        }))
    }

    pub fn with_rootfs<P: AsRef<Path>>(&mut self, path: P) -> Result<&mut Self> {
        self.rootfs = Some(
            path.as_ref()
                .canonicalize()
                .with_context(|| format!("Failed to canonicalize {:?}", path.as_ref()))?,
        );
        Ok(self)
    }

    pub fn from_default_distro(&mut self) -> Result<&mut Self> {
        let config =
            DistrodConfig::get().with_context(|| "Failed to acquire the Distrod config.")?;
        self.rootfs = Some(config.distrod.default_distro_image.as_path().to_owned());
        Ok(self)
    }

    pub fn launch(self) -> Result<Distro> {
        let rootfs = self
            .rootfs
            .as_ref()
            .ok_or_else(|| anyhow!("rootfs is not initialized."))?;
        if let Err(e) = setup_etc_environment_file(&rootfs) {
            log::warn!("Failed to setup /etc/environment. {:#?}", e);
        }
        let mut container_launcher = ContainerLauncher::new();
        if self.rootfs.as_deref() != Some(Path::new("/")) {
            mount_wsl_mountpoints(&mut container_launcher)
                .with_context(|| "Failed to mount WSL mountpoints.")?;
        }
        mount_kernelcmdline(&mut container_launcher)
            .with_context(|| "Failed to mount kernelcmdline.")?;
        let container = container_launcher
            .launch(
                None,
                HostPath::new(&rootfs)?,
                ContainerPath::new(DISTRO_OLD_ROOT_PATH)?,
            )
            .with_context(|| "Failed to launch a container.")?;
        export_distro_run_info(&rootfs, container.init_pid)
            .with_context(|| "Failed to export the Distro running information.")?;
        let distro = Distro {
            container,
            rootfs: HostPath::new(rootfs)?,
        };
        Ok(distro)
    }
}

fn mount_wsl_mountpoints(container_launcher: &mut ContainerLauncher) -> Result<()> {
    let binds = vec![
        ("/init", true),
        ("/sys", false),
        ("/dev", false),
        ("/mnt/wsl", false),
        ("/run/WSL", false),
        ("/etc/wsl.conf", true),
        ("/etc/resolv.conf", true),
        ("/proc/sys/fs/binfmt_misc", false),
    ];
    for (bind_file, is_file) in binds {
        if !Path::new(bind_file).exists() {
            log::warn!("WSL path {:?} does not exist.", bind_file);
            continue;
        }
        container_launcher.with_mount(
            Some(HostPath::new(bind_file)?),
            ContainerPath::new(bind_file)?,
            None,
            nix::mount::MsFlags::MS_BIND,
            None,
            is_file,
        );
    }

    // Mount 9p drives, that is, Windows drives.
    let mount_entries = get_mount_entries().with_context(|| "Failed to retrieve mount entries")?;
    for mount_entry in mount_entries {
        let path = &mount_entry.path;
        if mount_entry.fstype.as_str() != "9p" {
            continue;
        }
        if path.to_str() == Some("/init") {
            // /init is also mounted by 9p, but we have already mounted it.
            continue;
        }
        container_launcher.with_mount(
            Some(HostPath::new(path)?),
            ContainerPath::new(path)?,
            None,
            nix::mount::MsFlags::MS_BIND,
            None,
            false,
        );
    }
    Ok(())
}

/// Overwrite the kernel cmdline with one for the container.
fn mount_kernelcmdline(container_launcher: &mut ContainerLauncher) -> Result<()> {
    let new_cmdline_path = "/run/distrod-cmdline";
    let mut new_cmdline = File::create(new_cmdline_path)
        .with_context(|| format!("Failed to create '{}'.", new_cmdline_path))?;
    nix::unistd::chown(
        new_cmdline_path,
        Some(nix::unistd::Uid::from_raw(0)),
        Some(nix::unistd::Gid::from_raw(0)),
    )
    .with_context(|| format!("Failed to chown '{}'", new_cmdline_path))?;

    let mut cmdline_cont =
        std::fs::read("/proc/cmdline").with_context(|| "Failed to read /proc/cmdline.")?;
    if cmdline_cont.ends_with("\n".as_bytes()) {
        cmdline_cont.truncate(cmdline_cont.len() - 1);
    }

    // Set default environment vairables for the systemd services.
    for setenv in to_systemd_setenv_args(
        collect_wsl_env_vars()
            .with_context(|| "Failed to collect WSL envs.")?
            .into_iter(),
    ) {
        cmdline_cont.extend(" ".as_bytes());
        cmdline_cont.extend(setenv.as_bytes());
    }
    cmdline_cont.extend("\n".as_bytes());

    new_cmdline
        .write_all(&cmdline_cont)
        .with_context(|| "Failed to write the new cmdline.")?;

    container_launcher.with_mount(
        Some(HostPath::new(new_cmdline_path)?), // /run is bind-mounted
        ContainerPath::new("/proc/cmdline")?,
        None,
        nix::mount::MsFlags::MS_BIND,
        None,
        true,
    );
    Ok(())
}

fn to_systemd_setenv_args<I>(env: I) -> Vec<OsString>
where
    I: Iterator<Item = (OsString, OsString)>,
{
    let mut args = vec![];
    for (name, value) in env {
        let mut arg = OsString::from("systemd.setenv=");
        arg.push(name);
        arg.push("=");
        arg.push(value);
        args.push(arg);
    }
    args
}

pub struct Distro {
    rootfs: HostPath,
    container: Container,
}

#[derive(Serialize, Deserialize)]
pub struct DistroRunInfo {
    rootfs: PathBuf,
    init_pid: u32,
}

impl Distro {
    pub fn exec_command<I, S, T1, T2, P>(
        &self,
        command: S,
        args: I,
        wd: Option<P>,
        arg0: Option<T2>,
        cred: Option<&Credential>,
    ) -> Result<Waiter>
    where
        I: IntoIterator<Item = T1>,
        S: AsRef<OsStr>,
        T1: AsRef<OsStr>,
        T2: AsRef<OsStr>,
        P: AsRef<Path>,
    {
        log::debug!("Distro::exec_command.");
        let mut command = Command::new(command.as_ref());
        command
            .args(args)
            // Adding the path to distrod bin allows us to hook "chsh" command to show the message
            // to ask users to run "distrod enable" command.
            .env(
                "PATH",
                add_distrod_bin_to_path(std::env::var("PATH").unwrap_or_default()),
            );
        if let Some(wd) = wd {
            command.current_dir(wd.as_ref());
        }
        if let Some(arg0) = arg0 {
            command.arg0(arg0.as_ref());
        }
        self.container
            .exec_command(command, cred)
            .with_context(|| "Failed to exec command in the container")
    }

    pub fn stop(self, sigkill: bool) -> Result<()> {
        if let Err(e) = cleanup_etc_environment_file(&self.rootfs) {
            log::warn!("Failed to clean up /etc/environment. {:#?}", e);
        }
        self.container.stop(sigkill)
    }
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

pub fn initialize_distro_rootfs<P: AsRef<Path>>(
    rootfs: P,
    overwrites_potential_userfiles: bool,
) -> Result<()> {
    let rootfs = HostPath::new(rootfs.as_ref())?;
    // Remove systemd network configurations
    for path in glob::glob(
        &ContainerPath::new("/etc/systemd/network/*.network")?
            .to_host_path(&rootfs)
            .as_os_str()
            .to_str()
            .ok_or_else(|| anyhow!("Failed to convert systemd network file paths."))?,
    )? {
        let path = path?;
        fs::remove_file(&path).with_context(|| format!("Failed to remove '{:?}'.", &path))?;
    }

    // echo hostname to /etc/hostname
    let hostname_path = ContainerPath::new("/etc/hostname")?.to_host_path(&rootfs);
    let mut hostname_buf = vec![0; 64];
    let hostname =
        nix::unistd::gethostname(&mut hostname_buf).with_context(|| "Failed to get hostname.")?;
    fs::write(&hostname_path, hostname.to_str()?.as_bytes())
        .with_context(|| format!("Failed to write hostname to '{:?}'.", &hostname_path))?;

    // Remove /etc/resolv.conf
    if overwrites_potential_userfiles {
        let resolv_conf_path = &ContainerPath::new("/etc/resolv.conf")?.to_host_path(&rootfs);
        fs::remove_file(&resolv_conf_path)
            .with_context(|| format!("Failed to remove '{:?}'.", &resolv_conf_path))?;
        // Touch /etc/resolv.conf so that WSL over-writes it or we can do bind-mount on it
        File::create(&resolv_conf_path)
            .with_context(|| format!("Failed to touch '{:?}'", &resolv_conf_path))?;
    }

    // Disable or mask incompatible systemd services
    let to_be_disabled = [
        "dhcpcd.service",
        "NetworkManager.service",
        "multipathd.service",
    ];
    for unit in &to_be_disabled {
        if let Err(err) = SystemdUnitDisabler::new(&rootfs.as_path(), unit).disable() {
            log::warn!("Faled to disable {}. Error: {:?}", unit, err);
        }
    }
    let to_be_masked = ["systemd-remount-fs.service", "systemd-modules-load.service"];
    for unit in &to_be_masked {
        if let Err(err) = SystemdUnitDisabler::new(&rootfs.as_path(), unit).mask() {
            log::warn!("Faled to mask {}. Error: {:?}", unit, err);
        }
    }

    Ok(())
}

pub fn cleanup_distro_rootfs<P: AsRef<Path>>(rootfs: P) -> Result<()> {
    cleanup_etc_environment_file(&rootfs).with_context(|| "Failed to cleanup /etc/environment")?;

    Ok(())
}

fn setup_etc_environment_file<P: AsRef<Path>>(rootfs: P) -> Result<()> {
    let rootfs = HostPath::new(rootfs.as_ref())?;
    let env_file_path = &ContainerPath::new("/etc/environment")?.to_host_path(&rootfs);
    let mut env_file = EnvFile::open(&env_file_path)
        .with_context(|| format!("Failed to open '{:?}'.", &&env_file_path))?;

    // Set the WSL envs in the default environment variables
    let wsl_envs = collect_wsl_env_vars().with_context(|| "Failed to collect WSL envs.")?;
    for (key, value) in &wsl_envs {
        env_file.put(&key.to_string_lossy(), value.to_string_lossy().to_string());
    }

    // Put the Distrod's bin dir in PATH
    // This allows us to hook "chsh" command to show the message to ask users to run "distrod enable" command.
    // Do nothing if the environment file has no PATH since it may affect the system path in a bad way.
    if env_file
        .get("PATH")
        .map(|path| path.contains(distrod_config::get_distrod_bin_dir_path()))
        == Some(false)
    {
        env_file.put(
            "PATH",
            add_distrod_bin_to_path(env_file.get("PATH").unwrap_or(""))
                .to_string_lossy()
                .to_string(),
        );
    }

    env_file
        .save()
        .with_context(|| "Failed to save the environment file.")?;
    Ok(())
}

fn cleanup_etc_environment_file<P: AsRef<Path>>(rootfs: P) -> Result<()> {
    let rootfs = HostPath::new(rootfs.as_ref())?;
    let env_file_path = ContainerPath::new("/etc/environment")?.to_host_path(&rootfs);
    if !env_file_path.exists() {
        return Ok(());
    }
    let mut env_file = EnvFile::open(&env_file_path)
        .with_context(|| format!("Failed to open '{:?}'.", &&env_file_path))?;

    // Set the WSL envs in the default environment variables
    let wsl_envs = collect_wsl_env_vars().with_context(|| "Failed to collect WSL envs.")?;
    let keys: Vec<_> = wsl_envs.keys().collect();
    for key in keys {
        env_file.remove(&key.to_string_lossy());
    }

    // Put the Distrod's bin dir in PATH
    // This allows us to hook "chsh" command to show the message to ask users to run "distrod enable" command
    if env_file
        .get("PATH")
        .map(|path| path.contains(distrod_config::get_distrod_bin_dir_path()))
        == Some(true)
    {
        env_file.put(
            "PATH",
            remove_distrod_bin_from_path(env_file.get("PATH").unwrap_or("")),
        );
    }

    env_file
        .save()
        .with_context(|| "Failed to save the environment file.")?;
    Ok(())
}

fn add_distrod_bin_to_path<S: AsRef<OsStr>>(path: S) -> OsString {
    let mut result = OsString::from(distrod_config::get_distrod_bin_dir_path());
    result.push(":");
    result.push(path);
    result
}

fn remove_distrod_bin_from_path(path: &str) -> String {
    let mut distrod_bin_path = distrod_config::get_distrod_bin_dir_path().to_owned();
    distrod_bin_path.push(':');
    let result = path.replace(&distrod_bin_path, "");
    distrod_bin_path.pop();
    distrod_bin_path.insert(0, ':');
    result.replace(&distrod_bin_path, "")
}

fn export_distro_run_info(rootfs: &Path, init_pid: u32) -> Result<()> {
    if let Ok(Some(_)) = get_distro_run_info_file(false, false) {
        fs::remove_file(&DISTRO_RUN_INFO_PATH)
            .with_context(|| "Failed to remove the existing run info file.")?;
    }
    let mut file = BufWriter::new(
        get_distro_run_info_file(true, true)
            .with_context(|| "Failed to create a run info file.")?
            .expect("[BUG] get_distro_run_info_file shuold return Some when create:true"),
    );
    let run_info = DistroRunInfo {
        rootfs: rootfs.to_owned(),
        init_pid,
    };
    file.write_all(&serde_json::to_vec(&run_info)?)
        .with_context(|| "Failed to write to a distro run info file.")?;
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
