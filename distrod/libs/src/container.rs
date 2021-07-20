use anyhow::{bail, Context, Result};
use nix::fcntl::OFlag;
use nix::sched::CloneFlags;
use nix::NixPath;
use passfd::FdPassingExt;
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};

use crate::mount_info::{get_mount_entries, MountEntry};
use crate::multifork::{CommandByMultiFork, Waiter};
use crate::passwd::drop_privilege;
use crate::procfile::ProcFile;

#[derive(Serialize, Deserialize, Debug)]
pub struct Container {
    root_fs: PathBuf, // absolute path in the host mount namespaces
    init_pid: Option<u32>,
}

impl Container {
    pub fn new<P: AsRef<Path>>(root_fs: P) -> Result<Self> {
        Ok(Container {
            root_fs: fs::canonicalize(root_fs.as_ref())
                .with_context(|| format!("invalid root_fs path: '{:?}'", root_fs.as_ref()))?,
            init_pid: None,
        })
    }

    /// Export the infromation of this container in JSON format,
    /// which can be used to restore Container struct
    pub fn run_info_as_json(&self) -> Result<String> {
        Ok(serde_json::to_string(&self)?)
    }

    /// Get Container struct of an existing container from JSON
    pub fn get_running_container_from_json(json: &str) -> Result<Option<Container>> {
        // TODO: liveness check
        Ok(Some(serde_json::from_str(json)?))
    }

    pub fn launch<P: AsRef<Path>>(
        &mut self,
        init: Option<Vec<String>>,
        old_root: P,
    ) -> Result<ProcFile> {
        let init = init.unwrap_or_else(|| {
            vec![
                "/sbin/init".to_owned(),
                "--unit=multi-user.target".to_owned(),
            ]
        });

        let (fd_channel_host, fd_channel_child) = UnixStream::pair()?;
        {
            let mut command = CommandByMultiFork::new(&init[0]);
            command.args(&init[1..]);
            let fds_to_keep = vec![fd_channel_child.as_raw_fd()];
            command.pre_second_fork(move || {
                daemonize(&fds_to_keep)
                    .with_context(|| "The container failed to be daemonized.")?;
                enter_new_namespace().with_context(|| "Failed to initialize Linux namespaces.")?;
                Ok(())
            });
            let rootfs = self.root_fs.clone();
            let old_root = PathBuf::from(old_root.as_ref());
            unsafe {
                command.pre_exec(move || {
                    let inner = || -> Result<()> {
                        let procfile = ProcFile::current_proc()
                            .with_context(|| "Failed to make a ProcFile.")?;
                        fd_channel_child
                            .send_fd(procfile.as_raw_fd())
                            .with_context(|| "Failed to do send_fd.")?;
                        drop(procfile);
                        prepare_filesystem(&rootfs, &old_root)
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
        self.init_pid = Some(
            procfile
                .pid()
                .with_context(|| "Failed to get the pid of init.")?,
        );
        Ok(procfile)
    }

    pub fn exec_command<I, S, T1, T2, P>(
        &self,
        program: S,
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
        log::debug!("Container::exec_command.");
        if self.init_pid.is_none() {
            bail!("This container is not launched yet.");
        }

        let mut command = CommandByMultiFork::new(&program);
        command.args(args);
        if let Some(arg0) = arg0 {
            command.arg0(arg0);
        }
        if let Some(wd) = wd {
            command.current_dir(wd);
        }
        command.pre_second_fork(|| {
            enter_namespace(self.init_pid.unwrap())
                .with_context(|| "Failed to enter the init's namespace")?;
            if let Some((uid, gid)) = ids {
                drop_privilege(uid, gid);
            }
            Ok(())
        });
        command.do_triple_fork(true);
        let waiter = command
            .insert_waiter_proxy()
            .with_context(|| "Failed to request a proxy process.")?;
        command
            .spawn()
            .with_context(|| format!("Container::exec_command failed: {:?}", &program.as_ref()))?;
        log::debug!("Double fork done.");
        Ok(waiter)
    }

    pub fn stop(self, sigkill: bool) -> Result<()> {
        let signal = if sigkill {
            nix::sys::signal::SIGKILL
        } else {
            nix::sys::signal::SIGINT
        };
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(self.init_pid.expect("[BUG] no init pid.") as i32),
            signal,
        )
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

fn enter_namespace(pid: u32) -> Result<()> {
    for ns in &["uts", "pid", "mnt"] {
        let ns_path = format!("/proc/{}/ns/{}", pid, ns);
        let nsdir_fd = nix::fcntl::open(
            ns_path.as_str(),
            OFlag::O_RDONLY,
            nix::sys::stat::Mode::empty(),
        )
        .with_context(|| format!("Failed to open {}", &ns_path))?;
        nix::sched::setns(nsdir_fd, CloneFlags::empty())
            .with_context(|| format!("Setns({}) failed.", &ns_path))?;
        nix::unistd::close(nsdir_fd)?;
    }
    Ok(())
}

fn enter_new_namespace() -> Result<()> {
    nix::sched::unshare(
        CloneFlags::CLONE_NEWNS | CloneFlags::CLONE_NEWPID | CloneFlags::CLONE_NEWUTS,
    )?;
    Ok(())
}

fn prepare_filesystem<P1, P2>(new_root: P1, old_root: P2) -> Result<()>
where
    P1: AsRef<Path>,
    P2: AsRef<Path>,
{
    if new_root.as_ref() == Path::new("/") {
        prepare_host_base_root(old_root.as_ref())?;
    } else {
        prepare_minimum_root(new_root.as_ref(), old_root.as_ref())?;
        let mount_entries =
            get_mount_entries().with_context(|| "Failed to retrieve mount entries")?;
        mount_wsl_mountpoints(old_root.as_ref(), &mount_entries)?;
        umount_host_mountpoints(old_root.as_ref(), &mount_entries)?;
    }
    Ok(())
}

fn prepare_host_base_root<P: AsRef<Path>>(old_root: P) -> Result<()> {
    let saved_old_proc = old_root.as_ref().join("proc");
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
        .with_context(|| format!("setup {:?} fail.", old_root.as_ref().join("proc")))
}

fn prepare_minimum_root<P1, P2>(new_root: P1, old_root: P2) -> Result<()>
where
    P1: AsRef<Path>,
    P2: AsRef<Path>,
{
    let old_root_as_hostpath = new_root.as_ref().join(old_root.as_ref().strip_prefix("/")?);
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
        &new_root.as_ref(),
        None,
        nix::mount::MsFlags::MS_BIND,
        None,
    )
    .with_context(|| "Failed to bind mount the old_root")?;
    if new_root.as_ref() == old_root.as_ref() {
        std::env::set_current_dir(new_root.as_ref())
            .with_context(|| "Failed to chdir to the new root.")?;
    }
    nix::unistd::pivot_root(new_root.as_ref(), &old_root_as_hostpath).with_context(|| {
        format!(
            "pivot_root failed. new: {:#?}, old: {:#?}",
            new_root.as_ref(),
            old_root_as_hostpath.as_path()
        )
    })?;
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

fn mount_wsl_mountpoints<P: AsRef<Path>>(old_root: P, mount_entries: &[MountEntry]) -> Result<()> {
    let mut bind_source = PathBuf::from(old_root.as_ref());
    let binds = [
        ("/init", true),
        ("/sys", false),
        ("/dev", false),
        ("/mnt/wsl", false),
        ("/run/WSL", false),
        ("/etc/wsl.conf", true),
        ("/etc/resolv.conf", true),
        ("/proc/sys/fs/binfmt_misc", false),
    ];
    for (bind_target, is_file) in binds.iter() {
        let num_dirs = bind_target.matches('/').count();
        bind_source.push(&bind_target[1..]);
        if !bind_source.exists() {
            log::warn!("WSL path {:?} does not exist.", bind_source.to_str());
            for _ in 0..num_dirs {
                bind_source.pop();
            }
            continue;
        }
        let bind_target: &Path = bind_target.as_ref();
        create_mountpoint_unless_exist(bind_target, *is_file)?;
        nix::mount::mount::<Path, Path, Path, Path>(
            Some(bind_source.as_path()),
            bind_target,
            None,
            nix::mount::MsFlags::MS_BIND,
            None,
        )
        .with_context(|| {
            format!(
                "Failed to mount the WSL's special dir: {:?} -> {:?}",
                bind_source.as_path(),
                bind_target
            )
        })?;
        for _ in 0..num_dirs {
            bind_source.pop();
        }
    }

    // Mount 9p drives, that is, Windows drives.
    let mut init = bind_source.clone();
    init.push("init");
    let root = PathBuf::from("/");
    for mount_entry in mount_entries {
        let path = &mount_entry.path;
        if !path.starts_with(&bind_source) {
            continue;
        }
        if mount_entry.fstype.as_str() != "9p" {
            continue;
        }
        if *path == init {
            // /init is also mounted by 9p, but we have already mounted it.
            continue;
        }
        let path_inside_container =
            root.join(path.strip_prefix(&bind_source).with_context(|| {
                format!("Unexpected error. strip_prefix failed for {:?}", &path)
            })?);
        if !path_inside_container.exists() {
            fs::create_dir_all(&path_inside_container).with_context(|| {
                format!(
                    "Failed to create a mount point directory for {:?} inside the container.",
                    &path_inside_container
                )
            })?;
        }
        nix::mount::mount::<Path, Path, Path, Path>(
            Some(path),
            path_inside_container.as_ref(),
            None,
            nix::mount::MsFlags::MS_BIND,
            None,
        )
        .with_context(|| {
            format!(
                "Failed to mount the Windows drives: {:?} -> {:?}",
                path.as_path(),
                path_inside_container
            )
        })?;
    }
    Ok(())
}

fn create_mountpoint_unless_exist<P: AsRef<Path>>(path: P, is_file: bool) -> Result<()> {
    let path = path.as_ref();
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("Failed to retrieve the metadata of {:?}", path));
    let bind_target_exists = metadata.is_ok();
    if bind_target_exists {
        let file_type = metadata?.file_type();
        // Replace the symlink with an empty file only if this is a file mount.
        if file_type.is_symlink() && is_file {
            fs::remove_file(path).with_context(|| {
                format!(
                    "Failed to remove the existing symlink before mounting. '{:?}'",
                    path
                )
            })?;
        }
    }
    // this 'if' should not be 'else' because the `if` statement above could have deleted the path
    if !path.exists() {
        if is_file {
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
fn umount_host_mountpoints<P: AsRef<Path>>(
    old_root: P,
    mount_entries: &[MountEntry],
) -> Result<()> {
    let mut mount_paths: Vec<&PathBuf> = mount_entries.iter().map(|e| &e.path).collect();
    #[allow(clippy::clippy::unnecessary_sort_by)]
    mount_paths.sort_by(|a, b| b.len().cmp(&a.len())); // reverse sort
    for mount_path in mount_paths {
        if !mount_path.starts_with(&old_root) || mount_path.as_path() == old_root.as_ref() {
            continue;
        }
        let err = nix::mount::umount(mount_path.as_path());
        if err.is_err() {
            log::warn!("Failed to unmount '{:?}'", mount_path.as_path());
        }
    }
    Ok(())
}
