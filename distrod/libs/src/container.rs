use anyhow::{anyhow, bail, Context, Result};
use nix::sched::CloneFlags;
use nix::NixPath;
use passfd::FdPassingExt;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::ops::{Deref, DerefMut};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::mount_info::{get_mount_entries, MountEntry};
use crate::multifork::{CommandByMultiFork, Waiter};
use crate::passwd::Credential;
use crate::procfile::ProcFile;

#[derive(Default, Debug, Clone)]
pub struct ContainerLauncher {
    mounts: Vec<ContainerMount>,
    init_envs: Vec<(OsString, OsString)>,
    init_args: Vec<OsString>,
}

#[derive(Debug, Clone)]
pub struct ContainerMount {
    pub source: Option<HostPath>,
    pub target: ContainerPath,
    pub fstype: Option<OsString>,
    pub flags: nix::mount::MsFlags,
    pub data: Option<OsString>,
    pub is_file: bool,
}

impl ContainerLauncher {
    pub fn new() -> Self {
        ContainerLauncher::default()
    }

    pub fn from_pid(pid: u32) -> Result<Container> {
        let procfile =
            ProcFile::from_pid(pid)?.ok_or_else(|| anyhow!("The given PID does not exist."))?;
        Ok(Container {
            init_pid: pid,
            init_procfile: procfile,
        })
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
        log::debug!(
            "Container::with_mount source: {:?}, \
             target: {:?}, fstype: {:?}, flags: {:?}, is_file: {:?}",
            &source,
            &target,
            &fstype,
            &flags,
            is_file
        );
        self.mounts.push(ContainerMount {
            source,
            target,
            fstype,
            flags,
            data,
            is_file,
        });
        self
    }

    pub fn with_init_arg<O: AsRef<OsStr>>(&mut self, arg: O) -> &mut Self {
        self.init_args.push(arg.as_ref().to_owned());
        self
    }

    pub fn with_init_env<K, V>(&mut self, key: K, value: V) -> &mut Self
    where
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.init_envs
            .push((key.as_ref().to_owned(), value.as_ref().to_owned()));
        self
    }

    pub fn launch<S: AsRef<OsStr>>(
        self,
        init: S,
        rootfs: HostPath,
        old_root: ContainerPath,
    ) -> Result<Container> {
        let (fd_channel_host, fd_channel_child) = UnixStream::pair()?;
        {
            let mut command = Command::new(&init);
            // Systemd must not inherit environment variables from the parent process which may
            // be launcehd by a non-root user.
            command.env_clear();
            command.args(&self.init_args);
            command.envs(self.init_envs.iter().map(|(k, v)| (k, v)));
            let mut command = CommandByMultiFork::new(command);
            let fds_to_keep = vec![fd_channel_child.as_raw_fd()];
            command.pre_second_fork(move || {
                daemonize(&fds_to_keep)
                    .with_context(|| "The container failed to be daemonized.")?;
                enter_new_namespace().with_context(|| "Failed to initialize Linux namespaces.")?;
                Ok(())
            });
            unsafe {
                command.pre_exec(move || {
                    let inner = || -> Result<()> {
                        let procfile = ProcFile::current_proc()
                            .with_context(|| "Failed to make a ProcFile.")?;
                        fd_channel_child
                            .send_fd(procfile.as_raw_fd())
                            .with_context(|| "Failed to do send_fd.")?;
                        drop(procfile);

                        self.prepare_filesystem(&rootfs, &old_root)
                            .with_context(|| "Failed to initialize the container's filesystem.")?;
                        Ok(())
                    };
                    if let Err(err) = inner().with_context(|| "Failed to send pidfd.") {
                        log::error!("{:?}", err);
                        std::process::exit(0);
                    }
                    Ok(())
                });
            }
            command
                .spawn()
                .with_context(|| "Failed to spawn the init process.")?;
        };

        let procfile_fd = fd_channel_host
            .recv_fd()
            .with_context(|| "Failed to do recv_fd.")?;
        let mut procfile = unsafe { ProcFile::from_raw_fd(procfile_fd) };
        let init_pid = procfile
            .pid()
            .with_context(|| "Failed to get the pid of init.")?;
        let init_procfile = procfile;
        Ok(Container {
            init_pid,
            init_procfile,
        })
    }

    fn prepare_filesystem(&self, new_root: &HostPath, old_root: &ContainerPath) -> Result<()> {
        if new_root.as_path() == Path::new("/") {
            prepare_host_base_root(old_root)?;
            self.process_mounts(&ContainerPath::new("/")?)?;
        } else {
            prepare_minimum_root(new_root, old_root)?;
            self.process_mounts(old_root)?;
            let mount_entries =
                get_mount_entries().with_context(|| "Failed to retrieve mount entries")?;
            umount_host_mountpoints(old_root, &mount_entries)?;
        }
        Ok(())
    }

    fn process_mounts(&self, old_root: &ContainerPath) -> Result<()> {
        for mount in &self.mounts {
            create_mountpoint_unless_exist(mount.target.as_path(), mount.is_file)
                .with_context(|| format!("Failed to create mountpoint {:?}", mount.target))?;
            let source = mount.source.as_ref().map(|p| p.to_container_path(old_root));
            if source.as_ref() == Some(&mount.target) {
                log::trace!("skipping an identical mount: {:#?}, {:#?}", source, mount);
                continue;
            }
            log::trace!("mounting source: {:#?}, mount: {:?}", &source, &mount);
            nix::mount::mount(
                source.as_ref().map(|p| p.as_path()),
                mount.target.as_path(),
                mount.fstype.as_deref(),
                mount.flags,
                mount.data.as_deref(),
            )
            .with_context(|| format!("Failed to mount {:?}", &mount))?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub enum MountSource {
    Host(HostPath),
    Container(ContainerPath),
    None,
}

#[non_exhaustive]
pub struct Container {
    pub init_pid: u32,
    init_procfile: ProcFile,
}

impl Container {
    pub fn exec_command(&self, command: Command, cred: Option<&Credential>) -> Result<Waiter> {
        log::debug!("Container::exec_command.");

        let mut command = CommandByMultiFork::new(command);
        command.pre_second_fork(|| {
            enter_namespace(&self.init_procfile)
                .with_context(|| "Failed to enter the init's namespace")?;
            if let Some(cred) = cred {
                cred.drop_privilege();
            }
            Ok(())
        });
        command.do_triple_fork(true);
        let waiter = command
            .insert_waiter_proxy()
            .with_context(|| "Failed to request a proxy process.")?;
        command
            .spawn()
            .with_context(|| "Container::exec_command failed")?;
        log::debug!("Double fork done.");
        Ok(waiter)
    }

    pub fn stop(self, sigkill: bool) -> Result<()> {
        let signal = if sigkill {
            nix::sys::signal::SIGKILL
        } else {
            nix::sys::signal::SIGINT
        };
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(self.init_pid as i32), signal)
            .with_context(|| "Failed to kill the init process of the container.")?;
        Ok(())
    }
}

fn daemonize(fds_to_keep: &[i32]) -> Result<()> {
    nix::unistd::setsid().with_context(|| "Failed to setsid().")?;
    for i in 1..=255 {
        if fds_to_keep.contains(&i) {
            continue;
        }
        let _ = nix::fcntl::fcntl(
            i,
            nix::fcntl::FcntlArg::F_SETFD(nix::fcntl::FdFlag::FD_CLOEXEC),
        );
    }
    Ok(())
}

fn enter_namespace(proc: &ProcFile) -> Result<()> {
    for ns in &["ns/uts", "ns/pid", "ns/mnt"] {
        let ns_file = proc.open_file_at(ns)?;
        nix::sched::setns(ns_file.as_raw_fd(), CloneFlags::empty())
            .with_context(|| format!("Setns({}) failed.", ns))?;
    }
    Ok(())
}

fn enter_new_namespace() -> Result<()> {
    nix::sched::unshare(
        CloneFlags::CLONE_NEWNS | CloneFlags::CLONE_NEWPID | CloneFlags::CLONE_NEWUTS,
    )?;
    Ok(())
}

fn prepare_host_base_root(old_root: &ContainerPath) -> Result<()> {
    let saved_old_proc = old_root.join("proc");
    create_mountpoint_unless_exist(&saved_old_proc, false)?;
    nix::mount::mount::<Path, Path, Path, Path>(
        Some("/proc".as_ref()),
        &saved_old_proc,
        None,
        nix::mount::MsFlags::MS_BIND,
        None,
    )
    .with_context(|| format!("Failed to mount the old proc on {:?}.", &saved_old_proc))?;
    mount_nosource_fs("/proc", "proc")
        .with_context(|| format!("setup {:?} fail.", old_root.join("proc")))
}

fn prepare_minimum_root(new_root: &HostPath, old_root: &ContainerPath) -> Result<()> {
    let old_root_as_hostpath = old_root.to_host_path(new_root);
    if !old_root_as_hostpath.exists() {
        fs::create_dir_all(&old_root_as_hostpath).with_context(|| {
            format!(
                "Failed to create a mount point for the old_root: {:?}.",
                &old_root_as_hostpath,
            )
        })?;
    }
    nix::mount::mount::<Path, Path, Path, Path>(
        Some(new_root.as_ref()),
        new_root.as_ref(),
        None,
        nix::mount::MsFlags::MS_BIND,
        None,
    )
    .with_context(|| "Failed to bind mount the old_root")?;
    if new_root.as_path() == old_root.as_path() {
        std::env::set_current_dir(new_root.as_path())
            .with_context(|| "Failed to chdir to the new root.")?;
    }
    nix::unistd::pivot_root(new_root.as_path(), old_root_as_hostpath.as_path()).with_context(
        || {
            format!(
                "pivot_root failed. new: {:#?}, old: {:#?}",
                new_root.as_path(),
                old_root_as_hostpath.as_path()
            )
        },
    )?;
    let minimum_mounts = [
        ("/proc", "proc"),
        ("/tmp", "tmpfs"),
        ("/run", "tmpfs"),
        ("/run/shm", "tmpfs"),
    ];
    for (path, fstype) in minimum_mounts.iter() {
        mount_nosource_fs(path, fstype)?;
    }
    Ok(())
}

fn mount_nosource_fs<P: AsRef<Path>>(path: P, fstype: &str) -> Result<()> {
    create_mountpoint_unless_exist(path.as_ref(), false)?;
    nix::mount::mount::<Path, Path, Path, Path>(
        None,
        path.as_ref(),
        Some(fstype.as_ref()),
        nix::mount::MsFlags::empty(),
        None,
    )
    .with_context(|| format!("mount {:?} failed.", path.as_ref()))
}

fn create_mountpoint_unless_exist<P: AsRef<Path>>(path: P, is_file: bool) -> Result<()> {
    let path = path.as_ref();
    let exists_and_is_symlink = fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false);
    if exists_and_is_symlink && is_file {
        // Remove the symlink so that it is replaced with an empty file only if this is a file mount.
        fs::remove_file(path).with_context(|| {
            format!(
                "Failed to remove the existing symlink before mounting. '{:?}'",
                path
            )
        })?;
    }
    // this 'if' should not be 'else' because the `if` statement above could have deleted the path
    if !path.exists() {
        if is_file {
            let parent = path.parent().unwrap_or_else(|| Path::new("/"));
            if !parent.exists() {
                fs::create_dir_all(parent).with_context(|| {
                    format!(
                        "Failed to create a mount point directory for {:?} inside the container.",
                        path
                    )
                })?;
            }
            File::create(path).with_context(|| {
                format!(
                    "Failed to create a mount point file for {:?} inside the container.",
                    path
                )
            })?;
        } else {
            fs::create_dir_all(path).with_context(|| {
                format!(
                    "Failed to create a mount point directory for {:?} inside the container.",
                    path
                )
            })?;
        }
    }
    Ok(())
}

#[allow(clippy::unnecessary_wraps)]
fn umount_host_mountpoints(old_root: &ContainerPath, mount_entries: &[MountEntry]) -> Result<()> {
    let mut mount_paths: Vec<&PathBuf> = mount_entries.iter().map(|e| &e.path).collect();
    mount_paths.sort_by_key(|b| std::cmp::Reverse(b.len())); // reverse sort
    for mount_path in mount_paths {
        if !mount_path.starts_with(&old_root) || mount_path.as_path() == old_root.as_path() {
            continue;
        }
        let err = nix::mount::umount(mount_path.as_path());
        if err.is_err() {
            log::warn!(
                "Failed to unmount '{:?}'. {}",
                mount_path.as_path(),
                err.unwrap_err()
            );
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerPath(PathBuf);

impl ContainerPath {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        if !path.as_ref().has_root() {
            bail!(
                "Non-absolute path is given to ContainerPath::new {:?}",
                path.as_ref()
            );
        }
        Ok(ContainerPath(path.as_ref().to_owned()))
    }

    pub fn to_host_path(&self, container_rootfs: &HostPath) -> HostPath {
        let host_path = container_rootfs.join(
            self.0
                .strip_prefix("/")
                .expect("[BUG] ContainerPath should be an absolute path."),
        );
        HostPath(host_path)
    }
}

impl AsRef<Path> for ContainerPath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

impl AsRef<ContainerPath> for ContainerPath {
    fn as_ref(&self) -> &ContainerPath {
        self
    }
}

impl Deref for ContainerPath {
    type Target = PathBuf;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for ContainerPath {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPath(PathBuf);

impl HostPath {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        if !path.as_ref().has_root() {
            bail!(
                "Non-absolute path is given to HostPath::new {:?}",
                path.as_ref()
            );
        }
        Ok(HostPath(path.as_ref().to_owned()))
    }

    pub fn to_container_path(&self, host_rootfs: &ContainerPath) -> ContainerPath {
        let container_path = host_rootfs.join(
            self.0
                .strip_prefix("/")
                .expect("[BUG] ContainerPath should be an absolute path."),
        );
        ContainerPath(container_path)
    }
}

impl AsRef<Path> for HostPath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

impl AsRef<HostPath> for HostPath {
    fn as_ref(&self) -> &HostPath {
        self
    }
}

impl Deref for HostPath {
    type Target = PathBuf;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for HostPath {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
