use anyhow::{anyhow, bail, Context, Result};
use nix::unistd::{Gid, Uid};
use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Write};
use std::os::linux::fs::MetadataExt;
use std::os::unix::prelude::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::container::{Container, ContainerLauncher, ContainerPath, HostPath};
use crate::distrod_config::{self, DistrodConfig};
use crate::envfile::{EnvFile, EnvShellScript};
use crate::mount_info::get_mount_entries;
pub use crate::multifork::Waiter;
use crate::passwd::{get_real_credential, Credential};
use crate::procfile::ProcFile;
use crate::systemdunit::{get_existing_systemd_unit, SystemdUnitDisabler, SystemdUnitOverride};
use crate::template::Template;
use crate::wsl_interop::{collect_wsl_env_vars, collect_wsl_paths};
use serde::{Deserialize, Serialize};

const DISTRO_OLD_ROOT_PATH: &str = "/mnt/distrod_root";

pub struct DistroLauncher {
    rootfs: Option<PathBuf>,
    system_envs: HashMap<String, String>,
    system_paths: HashSet<String>,
    container_launcher: ContainerLauncher,
}

impl DistroLauncher {
    pub fn new() -> Result<Self> {
        let mut distro_launcher = DistroLauncher {
            rootfs: None,
            system_envs: HashMap::new(),
            system_paths: HashSet::new(),
            container_launcher: ContainerLauncher::new(),
        };
        set_wsl_interop_envs_in_system_envs(&mut distro_launcher)
            .with_context(|| "failed to set up WSL interop env vars")?;
        mount_per_user_wsl_envs_script(&mut distro_launcher)
            .with_context(|| "failed to mount WSL environment variables init script.")?;
        mount_slash_run_static_files(&mut distro_launcher)
            .with_context(|| "Failed to mount /run files.")?;
        distro_launcher.with_system_path(distrod_config::get_distrod_bin_dir_path().to_owned());
        Ok(distro_launcher)
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

    pub fn with_system_env(&mut self, key: String, val: String) -> &mut Self {
        self.system_envs.insert(key, val);
        self
    }

    pub fn with_system_path(&mut self, path: String) -> &mut Self {
        self.system_paths.insert(path);
        self
    }

    pub fn with_init_arg<O: AsRef<OsStr>>(&mut self, arg: O) -> &mut Self {
        self.container_launcher.with_init_arg(arg);
        self
    }

    pub fn with_init_env<K, V>(&mut self, key: K, value: V) -> &mut Self
    where
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.container_launcher.with_init_env(key, value);
        self
    }

    pub fn with_mount(
        &mut self,
        source: Option<HostPath>,
        target: ContainerPath,
        fstype: Option<OsString>,
        flags: nix::mount::MsFlags,
        data: Option<OsString>,
        is_file: bool,
    ) -> &mut Self {
        self.container_launcher
            .with_mount(source, target, fstype, flags, data, is_file);
        self
    }

    pub fn launch(mut self) -> Result<Distro> {
        let rootfs = self
            .rootfs
            .as_ref()
            .ok_or_else(|| anyhow!("rootfs is not initialized."))?
            .clone();

        if rootfs != Path::new("/") {
            mount_wsl_mountpoints(&mut self).with_context(|| "Failed to mount WSL mountpoints.")?;
        }

        write_system_env_files(HostPath::new(&rootfs)?, self.system_envs, self.system_paths)
            .with_context(|| "Failed to write system env file.")?;

        self.container_launcher
            .with_init_env("container", "distrod") // See https://systemd.io/CONTAINER_INTERFACE/
            .with_init_arg("--unit=multi-user.target");
        unsafe {
            self.container_launcher.with_init_pre_exec(|| {
                // Systemd requires the real uid / gid to be the root.
                nix::unistd::setuid(Uid::from_raw(0))?;
                nix::unistd::setgid(Gid::from_raw(0))?;
                Ok(())
            });
        };
        let container = self
            .container_launcher
            .launch(
                "/sbin/init",
                HostPath::new(&rootfs)?,
                ContainerPath::new(DISTRO_OLD_ROOT_PATH)?,
            )
            .with_context(|| "Failed to launch a container.")?;

        export_distro_run_info(&rootfs, container.init_pid)
            .with_context(|| "Failed to export the Distro running information.")?;

        let distro = Distro { container };
        Ok(distro)
    }
}

fn set_wsl_interop_envs_in_system_envs(distro_launcher: &mut DistroLauncher) -> Result<()> {
    // Be careful to collect only harmless environment variables.
    // Distrod can be running as setuid program. So, non-root user can set arbitrary values.
    // mount_per_user_wsl_envs_script and its family should source those arbitrary values instead.
    // Actually, this doesn't matter for now because WSL allows a non-root user to be root by `wsl.exe -u root`,
    // but be prepared for this WSL spec to be improved in a safer direction someday.
    let wsl_envs = collect_wsl_env_vars().with_context(|| "Failed to collect WSL envs.")?;
    let envs_to_set = [OsStr::new("WSL_INTEROP"), OsStr::new("WSLENV")];
    for (key, value) in &wsl_envs {
        if !envs_to_set.contains(&key.as_os_str()) {
            continue;
        }
        if !sanity_check_wsl_env(key, value) {
            log::warn!("sanity check of {:?} failed.", &key);
            // stop handling this and further envs
            break;
        }
        distro_launcher.with_system_env(
            key.to_string_lossy().to_string(),
            value.to_string_lossy().to_string(),
        );
        distro_launcher
            .container_launcher
            .with_init_arg(&env_to_systemd_setenv_arg(key, value));
    }
    Ok(())
}

/// Make sure that the values of WSL_INTEROP, WSLENV, and WSL_DISTRO_NAME are harmless values that can be
/// written to /etc/environment and passed to Systemd service processes. These values may be polluted
/// because distrod-exec can be launched by any user.
fn sanity_check_wsl_env(key: &OsStr, value: &OsStr) -> bool {
    if key == OsStr::new("WSL_INTEROP") {
        sanity_check_wsl_interop(value)
    } else {
        sanity_check_general_wsl_envs(value)
    }
}

fn sanity_check_wsl_interop(value: &OsStr) -> bool {
    let inner = || -> Result<bool> {
        let safe_path = regex::Regex::new("^/run/WSL/[0-9]+_interop$")?;
        let str = value
            .to_str()
            .ok_or_else(|| anyhow!("non-UTF8 WSL_INTEROP value."))?;
        Ok(safe_path.is_match(str))
    };
    inner().unwrap_or(false)
}

fn sanity_check_general_wsl_envs(value: &OsStr) -> bool {
    // sanity check for WSLENV and WSL_INTEROP
    let inner = || -> Result<bool> {
        let harmless_pattern = regex::Regex::new("^[a-zA-Z0-9_\\-./:]*$")?;
        let str = value.to_str().ok_or_else(|| anyhow!("non-UTF8 value."))?;
        Ok(harmless_pattern.is_match(str))
    };
    inner().unwrap_or(false)
}

fn env_to_systemd_setenv_arg<K, V>(key: K, value: V) -> OsString
where
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let mut arg = OsString::from("systemd.setenv=");
    arg.push(key.as_ref());
    arg.push("=");
    arg.push(value.as_ref());
    arg
}

fn mount_per_user_wsl_envs_script(distro_launcher: &mut DistroLauncher) -> Result<()> {
    let mut env_shell_script = EnvShellScript::new();
    for (key, value) in collect_wsl_env_vars().with_context(|| "Failed to collect WSL envs.")? {
        env_shell_script.put_env(
            key.to_string_lossy().to_string(),
            value.to_string_lossy().to_string(),
        );
    }
    for path in collect_wsl_paths().with_context(|| "Failed to collect WSL paths.")? {
        env_shell_script.put_path(path);
    }

    let real_user = get_real_credential().with_context(|| "Failed to get the real credentail.")?;
    let host_sh_path = get_per_user_wsl_envs_init_script_path(&real_user)?;
    env_shell_script
        .write(&host_sh_path)
        .with_context(|| "Failed to write the EnvShellScript.")?;
    let container_sh_path =
        ContainerPath::new(get_per_user_wsl_envs_init_script_path(&real_user)?)?;

    distro_launcher.with_mount(
        Some(host_sh_path),
        container_sh_path,
        None,
        nix::mount::MsFlags::MS_BIND,
        None,
        true,
    );
    Ok(())
}

fn mount_slash_run_static_files(distro_launcher: &mut DistroLauncher) -> Result<()> {
    for path in glob::glob(&format!(
        "{}/**/*",
        distrod_config::get_distrod_run_overlay_dir()
    ))
    .with_context(|| "glob failed.")?
    {
        let path = path?;
        log::trace!("mount_distrod_run_files: path: {:?}", &path);
        if !path.is_file() {
            continue;
        }
        let dest_mount_path = ContainerPath::new(
            Path::new("/run").join(
                path.strip_prefix(distrod_config::get_distrod_run_overlay_dir())
                    .with_context(|| {
                        format!(
                            "[BUG] {:?} should starts with {:?}",
                            &path,
                            distrod_config::get_distrod_run_overlay_dir()
                        )
                    })?,
            ),
        )?;
        distro_launcher.with_mount(
            Some(HostPath::new(path)?),
            dest_mount_path,
            None,
            nix::mount::MsFlags::MS_BIND,
            None,
            true,
        );
    }
    Ok(())
}

fn mount_wsl_mountpoints(distro_launcher: &mut DistroLauncher) -> Result<()> {
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
        distro_launcher.with_mount(
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
        distro_launcher.with_mount(
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

fn write_system_env_files(
    rootfs_path: HostPath,
    envs: HashMap<String, String>,
    paths: HashSet<String>,
) -> Result<()> {
    let env_file_path = &ContainerPath::new("/etc/environment")?.to_host_path(&rootfs_path);
    let mut env_file = EnvFile::open(&env_file_path)
        .with_context(|| format!("Failed to open '{:?}'.", &env_file_path))?;
    for (name, value) in envs {
        env_file.put_env(name, value);
    }
    for path in paths {
        env_file.put_path(path);
    }
    env_file
        .write()
        .with_context(|| format!("Failed to write system env file on {:?}", env_file_path))?;
    Ok(())
}

pub struct Distro {
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
        command.args(args);
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
    do_distro_independent_initialization(&rootfs, overwrites_potential_userfiles)?;
    do_distro_specific_initialization(&rootfs, overwrites_potential_userfiles)
}

pub fn do_distro_independent_initialization(
    rootfs: &HostPath,
    overwrites_potential_userfiles: bool,
) -> Result<()> {
    fix_hostname(rootfs)?;
    disable_incompatible_systemd_network_configuration(rootfs, overwrites_potential_userfiles)?;
    disable_incompatible_systemd_services(rootfs);
    disable_incompatible_systemd_service_options(rootfs);
    create_per_user_wsl_envs_init_loader_script(rootfs)
        .with_context(|| "Failed to create per-user WSL envs load script.")?;
    Ok(())
}

fn disable_incompatible_systemd_network_configuration(
    rootfs: &HostPath,
    overwrites_potential_userfiles: bool,
) -> Result<(), anyhow::Error> {
    // Remove systemd network configurations
    for path in glob::glob(
        ContainerPath::new("/etc/systemd/network/*.network")?
            .to_host_path(rootfs)
            .as_os_str()
            .to_str()
            .ok_or_else(|| anyhow!("Failed to convert systemd network file paths."))?,
    )? {
        let path = path?;
        fs::remove_file(&path).with_context(|| format!("Failed to remove '{:?}'.", &path))?;
    }
    // Remove netplan network configurations
    for path in glob::glob(
        ContainerPath::new("/etc/netplan/*.yaml")?
            .to_host_path(rootfs)
            .as_os_str()
            .to_str()
            .ok_or_else(|| anyhow!("Failed to convert netplan network file paths."))?,
    )? {
        let path = path?;
        fs::remove_file(&path).with_context(|| format!("Failed to remove '{:?}'.", &path))?;
    }
    // Remove network-scripts configurations
    let path_to_network =
        ContainerPath::new("/etc/sysconfig/network-scripts/ifcfg-eth0")?.to_host_path(rootfs);
    if path_to_network.exists() {
        let backup_name =
            ContainerPath::new("/etc/sysconfig/network-scripts/disabled-by-distrod.ifcfg-eth0")?
                .to_host_path(rootfs);
        fs::rename(&path_to_network, &backup_name).with_context(|| {
            format!(
                "Failed to move {:?} to {:?}",
                &path_to_network, &backup_name
            )
        })?;
    }
    // Remove /etc/resolv.conf so that systemd knows it shouldn't touch resolv.conf
    if overwrites_potential_userfiles {
        let resolv_conf_path = &ContainerPath::new("/etc/resolv.conf")?.to_host_path(rootfs);
        fs::remove_file(&resolv_conf_path)
            .with_context(|| format!("Failed to remove '{:?}'.", &resolv_conf_path))?;
        // Touch /etc/resolv.conf so that WSL over-writes it or we can do bind-mount on it
        File::create(&resolv_conf_path)
            .with_context(|| format!("Failed to touch '{:?}'", &resolv_conf_path))?;
    }
    Ok(())
}

fn fix_hostname(rootfs: &HostPath) -> Result<(), anyhow::Error> {
    let hostname_path = ContainerPath::new("/etc/hostname")?.to_host_path(rootfs);
    let mut hostname_buf = vec![0; 64];
    let hostname =
        nix::unistd::gethostname(&mut hostname_buf).with_context(|| "Failed to get hostname.")?;
    fs::write(&hostname_path, hostname.to_str()?.as_bytes())
        .with_context(|| format!("Failed to write hostname to '{:?}'.", &hostname_path))?;
    Ok(())
}

fn disable_incompatible_systemd_services(rootfs: &HostPath) {
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
    let to_be_masked = [
        "systemd-remount-fs.service",
        "systemd-modules-load.service",
        "getty@tty1.service",
        "serial-getty@ttyS0.service",
        "console-getty.service",
    ];
    for unit in &to_be_masked {
        if let Err(err) = SystemdUnitDisabler::new(&rootfs.as_path(), unit).mask() {
            log::warn!("Faled to mask {}. Error: {:?}", unit, err);
        }
    }
}

fn disable_incompatible_systemd_service_options(rootfs: &HostPath) {
    let options = &[("systemd-sysusers.service", "Service", "LoadCredential")];

    for (service, section, option_directive) in options {
        let unit = match get_existing_systemd_unit(rootfs, *service).with_context(|| {
            format!(
                "Failed to get existing Systemd unit file of {:?}.",
                *service
            )
        }) {
            Ok(Some(unit)) => unit,
            Ok(None) => continue,
            Err(e) => {
                log::warn!("{:?}", e);
                continue;
            }
        };
        if unit.lookup_by_key(*option_directive).is_none() {
            continue;
        }

        let mut overrider = SystemdUnitOverride::default();
        overrider.unset_directive(*section, *option_directive);
        if let Err(e) = overrider.write(rootfs, *service).with_context(|| {
            format!(
                "Failed to disable option {:?} of {:?}",
                *option_directive, *service
            )
        }) {
            log::warn!("{:?}", e);
        }
    }
}

fn create_per_user_wsl_envs_init_loader_script(rootfs: &HostPath) -> Result<()> {
    let bytes = include_bytes!("../resources/load_per_user_wsl_envs.sh");
    let mut load_script = Template::new(String::from_utf8_lossy(bytes).into_owned());
    load_script.assign(
        "PER_USER_WSL_ENV_INIT_SCRIPT_PATH",
        &get_per_user_wsl_envs_init_script_shellexp()?,
    );
    load_script.assign(
        "ROOT_WSL_ENV_INIT_SCRIPT_PATH",
        get_per_user_wsl_envs_init_script_path(&Credential::new(
            Uid::from_raw(0),
            Gid::from_raw(0),
            vec![],
        ))?
        .to_str()
        .ok_or_else(|| {
            anyhow!("Failed to get the path to the per-user WSL env init script for root.")
        })?,
    );
    let profile_dot_d_path =
        ContainerPath::new("/etc/profile.d/distrod-user-wsl-envs.sh")?.to_host_path(rootfs);
    let mut profile_dot_d = BufWriter::new(
        File::create(&profile_dot_d_path)
            .with_context(|| format!("Failed to create {:?}", &profile_dot_d_path))?,
    );
    profile_dot_d
        .write_all(load_script.render().as_bytes())
        .with_context(|| format!("Failed to write to {:?}", rootfs))?;
    Ok(())
}

fn get_per_user_wsl_envs_init_script_shellexp() -> Result<String> {
    get_distrod_runtime_files_dir_path().map(|path| {
        let mut path_string = path.to_string_lossy().to_string();
        path_string += "/";
        path_string += &get_per_user_wsl_envs_init_script_name("$(id -u)");
        path_string
    })
}

fn get_per_user_wsl_envs_init_script_path(user: &Credential) -> Result<HostPath> {
    get_distrod_runtime_files_dir_path().map(|mut path| {
        path.push(&get_per_user_wsl_envs_init_script_name(
            &user.uid.as_raw().to_string(),
        ));
        path
    })
}

fn get_per_user_wsl_envs_init_script_name(uid: &str) -> String {
    format!("distrod_wsl_env-uid{}", uid)
}

fn do_distro_specific_initialization(
    rootfs: &HostPath,
    overwrites_potential_userfiles: bool,
) -> Result<()> {
    use DistroName::*;

    match detect_distro(rootfs).with_context(|| "Failed to detect distro.")? {
        Debian | Kali => initialize_debian_rootfs(rootfs, overwrites_potential_userfiles)
            .with_context(|| "Failed to do initialization for debian-based distros."),
        _ => Ok(()),
    }
}

enum DistroName {
    Debian,
    Kali,
    Undetected,
}

fn detect_distro(rootfs: &HostPath) -> Result<DistroName> {
    let os_release = EnvFile::open(ContainerPath::new("/etc/os-release")?.to_host_path(rootfs))
        .with_context(|| "Failed to parse /etc/os-release.");
    if let Err(ref e) = os_release {
        if e.downcast_ref::<std::io::Error>().map(|e| e.kind())
            == Some(std::io::ErrorKind::NotFound)
        {
            return Ok(DistroName::Undetected);
        }
    }
    match os_release?.get_env("ID").map(strip_quotes) {
        Some("debian") => Ok(DistroName::Debian),
        Some("kali") => Ok(DistroName::Kali),
        _ => Ok(DistroName::Undetected),
    }
}

fn strip_quotes(s: &str) -> &str {
    let mut result = s;
    if s.starts_with('"') {
        result = &result[1..result.len()];
    }
    if s.ends_with('"') {
        result = &result[0..result.len() - 1];
    }
    result
}

fn initialize_debian_rootfs(rootfs: &HostPath, overwrites_potential_userfiles: bool) -> Result<()> {
    if overwrites_potential_userfiles {
        // Ubuntu doesn't need this.
        put_readenv_in_sudo_pam(rootfs)
            .with_context(|| "Failed to put pam_env.so in /etc/pam.d/sudo.")?;
    }
    Ok(())
}

fn put_readenv_in_sudo_pam(rootfs: &HostPath) -> Result<()> {
    // Assume that the container's '/etc/pam.d/sudo' is not effective yet, so overwriting this is safe.
    // The calles must guarantee that the pam file is not currently used by the system, but it is initializing
    // a new rootfs.
    let pam_sudo_path = ContainerPath::new("/etc/pam.d/sudo")?.to_host_path(rootfs);
    let pam_cont = std::fs::read_to_string(&pam_sudo_path)
        .with_context(|| format!("Failed to read {:?}", &pam_sudo_path))?;
    if pam_cont.contains("pam_env.so") {
        return Ok(());
    }
    let mut lines: Vec<_> = pam_cont.split('\n').collect();
    lines.insert(
        2,
        "session    required   pam_env.so readenv=1 user_readenv=0",
    );
    lines.insert(
        2,
        "# The following line of pam_env.so is inserted by Distrod",
    );

    let mut pam_sudo = File::create(&pam_sudo_path)
        .with_context(|| format!("Failed to open {:?}", &pam_sudo_path))?;
    pam_sudo
        .write_all(lines.join("\n").as_bytes())
        .with_context(|| format!("Failed to update {:?}", &pam_sudo_path))?;

    Ok(())
}

fn export_distro_run_info(rootfs: &Path, init_pid: u32) -> Result<()> {
    if let Ok(Some(_)) = get_distro_run_info_file(false, false) {
        fs::remove_file(&get_distro_run_info_path()?)
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
    let json = json.open(get_distro_run_info_path()?);
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

fn get_distro_run_info_path() -> Result<HostPath> {
    get_distrod_runtime_files_dir_path().map(|mut path| {
        path.push("distrod_run_info.json");
        path
    })
}

fn get_distrod_runtime_files_dir_path() -> Result<HostPath> {
    let path = "/run/distrod";
    if !Path::new(&path).exists() {
        fs::create_dir(&path)
            .with_context(|| format!("Failed to create {:?} directory.", &path))?;
    }
    HostPath::new(path)
}

#[cfg(test)]
mod test_sanity_check {
    use super::*;

    #[test]
    fn test_sanity_check_wsl_interop() {
        assert!(sanity_check_wsl_interop(&OsString::from(
            "/run/WSL/12_interop"
        )));
        assert!(!sanity_check_wsl_interop(&OsString::from("/etc/passwd")));
        assert!(!sanity_check_wsl_interop(&OsString::from(
            "/run/WSL/some_new_socket"
        )));
        assert!(!sanity_check_wsl_interop(&OsString::from(
            "/run/WSL/12_interop_tail"
        )));
        assert!(!sanity_check_wsl_interop(&OsString::from(
            "/run/WSL/12_interop\ntest"
        )));
    }

    #[test]
    fn test_sanity_check_wsl_general_env() {
        assert!(sanity_check_general_wsl_envs(&OsString::from(
            "OneDrive/p:SOME_VAR:PATH/p"
        )));
        assert!(sanity_check_general_wsl_envs(&OsString::from(
            "Ubuntu-20.04"
        )));
        assert!(!sanity_check_general_wsl_envs(&OsString::from(
            "OneDrive/p:SOME_VAR:PATH/p\nHOME=/etc"
        )));
        assert!(!sanity_check_general_wsl_envs(&OsString::from(
            "Ubuntu-20.04\ntest"
        )));
    }
}
