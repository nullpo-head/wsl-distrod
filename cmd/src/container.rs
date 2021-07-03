use anyhow::{bail, Context, Result};
use nix::fcntl::OFlag;
use nix::sched::CloneFlags;
use nix::NixPath;
use passfd::FdPassingExt;
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};

use crate::multifork::{CommandByMultiFork, Waiter};
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

    pub fn exec_command<I, S, T, P>(&self, program: S, args: I, wd: Option<P>) -> Result<Waiter>
    where
        I: IntoIterator<Item = T>,
        S: AsRef<OsStr>,
        T: AsRef<OsStr>,
        P: AsRef<Path>,
    {
        log::debug!("Container::exec_command.");
        if self.init_pid.is_none() {
            bail!("This container is not launched yet.");
        }

        let mut command = CommandByMultiFork::new(&program);
        command.args(args);
        if let Some(wd) = wd {
            command.current_dir(wd);
        }
        command.pre_second_fork(|| {
            enter_namespace(self.init_pid.unwrap())
                .with_context(|| "Failed to enter the init's namespace")?;
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
    prepare_minimum_root(new_root.as_ref(), old_root.as_ref())?;
    let mount_entries = get_mount_entries().with_context(|| "Failed to retrieve mount entries")?;
    mount_wsl_mountpoints(old_root.as_ref(), &mount_entries)?;
    umount_host_mountpoints(old_root.as_ref(), &mount_entries)?;
    Ok(())
}

fn prepare_minimum_root<P1, P2>(new_root: P1, old_root: P2) -> Result<()>
where
    P1: AsRef<Path>,
    P2: AsRef<Path>,
{
    let old_root_as_hostpath = new_root.as_ref().join(old_root.as_ref().strip_prefix("/")?);
    nix::mount::mount::<Path, Path, Path, Path>(
        Some(new_root.as_ref()),
        &new_root.as_ref(),
        None,
        nix::mount::MsFlags::MS_BIND,
        None,
    )
    .with_context(|| "Failed to bind mount the old_root")?;
    nix::unistd::pivot_root(new_root.as_ref(), &old_root_as_hostpath).with_context(|| {
        format!(
            "pivot_root failed. new: {:#?}, old: {:#?}",
            new_root.as_ref(),
            old_root_as_hostpath.as_path()
        )
    })?;
    nix::mount::mount::<Path, Path, Path, Path>(
        None,
        "/proc".as_ref(),
        Some("proc".as_ref()),
        nix::mount::MsFlags::empty(),
        None,
    )
    .with_context(|| "mount /proc failed.")?;
    nix::mount::mount::<Path, Path, Path, Path>(
        None,
        "/tmp".as_ref(),
        Some("tmpfs".as_ref()),
        nix::mount::MsFlags::empty(),
        None,
    )
    .with_context(|| "mount /proc failed.")?;
    Ok(())
}

fn mount_wsl_mountpoints<P: AsRef<Path>>(old_root: P, mount_entries: &[MountEntry]) -> Result<()> {
    let mut old_root = PathBuf::from(old_root.as_ref());
    let binds = [
        "/init",
        "/proc/sys/fs/binfmt_misc",
        "/run",
        "/run/lock",
        "/run/shm",
        "/run/user",
        "/mnt/wsl",
        "/sys",
    ];
    for bind in binds.iter() {
        let num_dirs = bind.matches('/').count();
        old_root.push(&bind[1..]);
        if !old_root.exists() {
            log::debug!("WSL path {:?} does not exist", old_root.to_str());
            continue;
        }
        nix::mount::mount::<Path, Path, Path, Path>(
            Some(old_root.as_path()),
            bind.as_ref(),
            None,
            nix::mount::MsFlags::MS_BIND,
            None,
        )
        .with_context(|| {
            format!(
                "Failed to mount the WSL's special dir: {:?} -> {}",
                old_root.as_path(),
                bind
            )
        })?;
        for _ in 0..num_dirs {
            old_root.pop();
        }
    }

    let mut init = old_root.clone();
    init.push("init");
    let root = PathBuf::from("/");
    for mount_entry in mount_entries {
        let path = &mount_entry.path;
        if !path.starts_with(&old_root) {
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
            root.join(path.strip_prefix(&old_root).with_context(|| {
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

fn umount_host_mountpoints<P: AsRef<Path>>(
    old_root: P,
    mount_entries: &Vec<MountEntry>,
) -> Result<()> {
    let mut mount_paths: Vec<&PathBuf> = mount_entries.iter().map(|e| &e.path).collect();
    mount_paths.sort_by(|a, b| b.len().cmp(&a.len()));
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

struct MountEntry {
    path: PathBuf,
    fstype: String,
}

fn get_mount_entries() -> Result<Vec<MountEntry>> {
    let mounts = File::open("/proc/mounts").with_context(|| "Failed to open '/proc/mounts'")?;
    let reader = BufReader::new(mounts);

    let mut mount_entries = vec![];
    for (_, line) in reader.lines().enumerate() {
        let line = line?;
        let row: Vec<&str> = line.split(' ').take(3).collect();
        let (path, fstype) = (row[1].to_owned(), row[2].to_owned());
        mount_entries.push(MountEntry {
            path: PathBuf::from(path),
            fstype,
        });
    }

    Ok(mount_entries)
}
