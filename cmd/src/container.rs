use passfd::FdPassingExt;
use serde::{Deserialize, Serialize};
use std::ffi::CString;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use nix::fcntl::OFlag;
use nix::sched::CloneFlags;
use nix::NixPath;

use anyhow::{anyhow, Context, Result};

#[derive(Serialize, Deserialize, Debug)]
pub struct Container {
    root_fs: PathBuf,  // absolute path in the host mount namespaces
    init_pid: Option<u32>,
}

impl Container {
    pub fn new<P: AsRef<Path>>(root_fs: P) -> Result<Self> {
        Ok(Container {
            root_fs: fs::canonicalize(root_fs.as_ref()).with_context(|| format!("invalid root_fs path: '{:?}'", root_fs.as_ref()))?,
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

    pub fn launch<P: AsRef<Path>>(&mut self, init: Option<Vec<String>>, old_root: P) -> Result<()> {
        let init = init.unwrap_or(vec!["/sbin/init".to_owned(), "--unit=multi-user.target".to_owned()]);
        let init = vec!["/bin/bash"];
        let pidfd_file = {
            let mut command = Command::new(&init[0]);
            command.args(&init[1..]);
            let mut command = CommandWithDoubleFork::new(command);
            command.pre_second_fork(|| {
                daemonize().with_context(|| "The container failed to be daemonized.")?;
                self.prepare_namespace().with_context(|| "Failed to initialize Linux namespaces.")?;
                self.prepare_filesystem(&old_root).with_context(|| "Failed to initialize the container's filesystem.")?;
                Ok(())
            });
            command.spawn()?
        };
        let stat_fd = nix::fcntl::openat(pidfd_file.as_raw_fd(),
                                         "stat",
                                         OFlag::O_RDONLY,
                                         nix::sys::stat::Mode::empty()).with_context(|| "Failed to open the stat file.")?;
        let mut stat_file = unsafe { File::from_raw_fd(stat_fd) };
        let mut stat_cont = String::new();
        stat_file.read_to_string(&mut stat_cont)?;
        let pid = stat_cont.split(' ').nth(0).ok_or_else(|| anyhow!("Failed to read pid from the stat file."))?
                                             .parse().with_context(|| "Failed to parse the pid.")?;
        self.init_pid = Some(pid);
        Ok(())
    }

    fn prepare_namespace(&self) -> Result<()> {
        nix::sched::unshare(CloneFlags::CLONE_NEWNS | CloneFlags::CLONE_NEWPID | CloneFlags::CLONE_NEWUTS)?;
        Ok(())
    }

    fn prepare_filesystem<P: AsRef<Path>>(&self, old_root: P) -> Result<()> {
        self.prepare_minimum_root(old_root.as_ref())?;
        let mount_entries = get_mount_entries().with_context(|| "Failed to retrieve mount entries")?;
        self.mount_wsl_mountpoints(old_root.as_ref(), &mount_entries)?;
        self.umount_host_mountpoints(old_root.as_ref(), &mount_entries)?;
        Ok(())
    }

    fn prepare_minimum_root<P: AsRef<Path>>(&self, old_root: P) -> Result<()> {
        let old_root_as_hostpath = self.root_fs.join(old_root.as_ref().strip_prefix("/")?);
        nix::mount::mount::<Path, Path, Path, Path>(Some(&self.root_fs), &self.root_fs, None, nix::mount::MsFlags::MS_BIND, None).with_context(|| "Failed to bind mount the old_root")?;
        nix::unistd::pivot_root(&self.root_fs, &old_root_as_hostpath).with_context(|| format!("pivot_root failed. new: {:#?}, old: {:#?}", self.root_fs.as_path(), old_root_as_hostpath.as_path()))?;
        nix::mount::mount::<Path, Path, Path, Path>(None, "/proc".as_ref(), Some("proc".as_ref()), nix::mount::MsFlags::empty(), None).with_context(|| "mount /proc failed.")?;
        nix::mount::mount::<Path, Path, Path, Path>(None, "/tmp".as_ref(), Some("tmpfs".as_ref()), nix::mount::MsFlags::empty(), None).with_context(|| "mount /proc failed.")?;
        Ok(())
    }

    fn mount_wsl_mountpoints<P: AsRef<Path>>(&self, old_root: P, mount_entries: &Vec<MountEntry>) -> Result<()> {
        let mut old_root = PathBuf::from(old_root.as_ref());
        let binds = ["/init", "/proc/sys/fs/binfmt_misc", "/run", "/run/lock", "/run/shm", "/run/user", "/mnt/wsl", "/tmp"];
        for bind in binds.iter() {
            let num_dirs = bind.matches('/').count();
            old_root.push(&bind[1..]);
            if !old_root.exists() {
                log::debug!("WSL path {:?} does not exist", old_root.to_str());
                continue;
            }
            nix::mount::mount::<Path, Path, Path, Path>(Some(old_root.as_path()), bind.as_ref(), None, nix::mount::MsFlags::MS_BIND, None).with_context(|| format!("Failed to mount the WSL's special dir: {:?} -> {}", old_root.as_path(), bind))?;
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
            if *path == init {  // /init is also mounted by 9p, but we have already mounted it.
                continue;
            }
            let path_inside_container = root.join(path.strip_prefix(&old_root).with_context(|| format!("Unexpected error. strip_prefix failed for {:?}", &path))?);
            if !path_inside_container.exists() {
                fs::create_dir_all(&path_inside_container).with_context(|| format!("Failed to create a mount point directory for {:?} inside the container.", &path_inside_container))?;
            }
            nix::mount::mount::<Path, Path, Path, Path>(Some(path), path_inside_container.as_ref(), None, nix::mount::MsFlags::MS_BIND, None).with_context(|| format!("Failed to mount the Windows drives: {:?} -> {:?}", path.as_path(), path_inside_container))?;
        }
        Ok(())
    }

    fn umount_host_mountpoints<P: AsRef<Path>>(&self, old_root: P, mount_entries: &Vec<MountEntry>) -> Result<()> {
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
}

fn daemonize() -> Result<()> {
    Ok(())
}

struct CommandWithDoubleFork<'a> {
    command: Command,
    pre_second_fork: Option<Box<dyn FnMut() -> Result<()> + 'a>>
}

impl<'a> CommandWithDoubleFork<'a> {
    fn new(command: Command) -> CommandWithDoubleFork<'a> {
        CommandWithDoubleFork { command, pre_second_fork: None }
    }

    fn pre_second_fork<F>(&mut self, f: F) -> &mut CommandWithDoubleFork<'a>
      where F: FnMut() -> Result<()> + 'a {
        self.pre_second_fork = Some(Box::new(f));
        self
    }

    fn spawn(&mut self) -> Result<File> {
        let (fd_channel_host, fd_channel_child) = UnixStream::pair()?;
        unsafe {
            self.command.pre_exec(move || {
                let inner = || -> Result<()> {
                    let pidfd = nix::fcntl::open("/proc/self", 
                        OFlag::O_RDONLY | OFlag::O_CLOEXEC,
                        nix::sys::stat::Mode::empty())?;
                    fd_channel_child.send_fd(pidfd).with_context(|| "Failed to do send_fd.")?;
                    Ok(())
                };
                if let Err(err) = inner().with_context(|| "Failed to send pidfd.") {
                    log::error!("{:?}", err);
                    std::process::exit(0);
                }
                Ok(())
            });
        }
        if unsafe { nix::unistd::fork().with_context(|| "The first fork failed")? }.is_child() {
            let mut inner = || -> Result<()> {
                if let Some(ref mut f) = self.pre_second_fork {
                    f().with_context(|| "Pre_second_fork failed.")?;
                }
                self.command.spawn().with_context(|| "Failed to spawn command.")?;
                Ok(())
            };
            if let Err(err) = inner() {
                log::error!("{:?}", err);
            }
            std::process::exit(0);
        }
        let pidfd_fd = fd_channel_host.recv_fd().with_context(|| "Failed to do recv_fd.")?;
        let pidfd_file = unsafe { File::from_raw_fd(pidfd_fd) };
        Ok(pidfd_file)
    }
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
        mount_entries.push(MountEntry { path: PathBuf::from(path), fstype });
    }

    Ok(mount_entries)
}
