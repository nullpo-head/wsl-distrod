use anyhow::{bail, Context, Result};
use nix::sched::CloneFlags;
use nix::unistd::{chown, Gid, Uid};
use nix::NixPath;
use passfd::FdPassingExt;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use std::os::unix::prelude::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::mount_info::{get_mount_entries, MountEntry};
use crate::multifork::{CommandByMultiFork, Waiter};
use crate::passwd::Credential;
use crate::procfile::ProcFile;
use crate::wsl_interop::collect_wsl_env_vars;

#[non_exhaustive]
pub struct Container {
    pub init_pid: Option<u32>,
    init_procfile: Option<ProcFile>,
}

impl Container {
    pub fn new() -> Self {
        Container {
            init_pid: None,
            init_procfile: None,
        }
    }

    pub fn from_pid(pid: u32) -> Result<Self> {
        let procfile = ProcFile::from_pid(pid)?;
        if procfile.is_none() {
            bail!("The given pid does not exist");
        }
        Ok(Container {
            init_pid: Some(pid),
            init_procfile: procfile,
        })
    }

    pub fn launch<P1, P2>(
        &mut self,
        init: Option<Vec<OsString>>,
        rootfs: P1,
        old_root: P2,
    ) -> Result<()>
    where
        P1: AsRef<Path>,
        P2: AsRef<Path>,
    {
        let init = init.unwrap_or_else(|| {
            vec![
                OsString::from("/sbin/init"),
                OsString::from("--unit=multi-user.target"),
            ]
        });

        let (fd_channel_host, fd_channel_child) = UnixStream::pair()?;
        {
            let mut command = Command::new(&init[0]);
            command.args(&init[1..]);
            let mut command = CommandByMultiFork::new(command);
            let fds_to_keep = vec![fd_channel_child.as_raw_fd()];
            command.pre_second_fork(move || {
                daemonize(&fds_to_keep)
                    .with_context(|| "The container failed to be daemonized.")?;
                enter_new_namespace().with_context(|| "Failed to initialize Linux namespaces.")?;
                Ok(())
            });
            let new_root = PathBuf::from(rootfs.as_ref());
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
                        prepare_filesystem(&new_root, &old_root)
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
        self.init_procfile = Some(procfile);
        Ok(())
    }

    pub fn exec_command(&self, command: Command, cred: Option<&Credential>) -> Result<Waiter> {
        log::debug!("Container::exec_command.");
        if self.init_pid.is_none() {
            bail!("This container is not launched yet.");
        }

        let mut command = CommandByMultiFork::new(command);
        command.pre_second_fork(|| {
            enter_namespace(self.init_procfile.as_ref().unwrap())
                .with_context(|| "Failed to enter the init's namespace")?;
            if let Some(ref cred) = cred {
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

fn prepare_filesystem<P1, P2>(new_root: P1, old_root: P2) -> Result<()>
where
    P1: AsRef<Path>,
    P2: AsRef<Path>,
{
    if new_root.as_ref() == Path::new("/") {
        prepare_host_base_root(old_root.as_ref())?;
        mount_kernelcmdline().with_context(|| "Failed to overwrite the kernel commandline.")?;
    } else {
        prepare_minimum_root(new_root.as_ref(), old_root.as_ref())?;
        let mount_entries =
            get_mount_entries().with_context(|| "Failed to retrieve mount entries")?;
        mount_wsl_mountpoints(old_root.as_ref(), &mount_entries)?;
        mount_kernelcmdline().with_context(|| "Failed to overwrite the kernel commandline.")?;
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

/// Overwrite the kernel cmdline with one for the container.
fn mount_kernelcmdline() -> Result<()> {
    let new_cmdline_path = "/run/distrod-cmdline";
    let mut new_cmdline = File::create(new_cmdline_path)
        .with_context(|| format!("Failed to create '{}'.", new_cmdline_path))?;
    chown(
        new_cmdline_path,
        Some(Uid::from_raw(0)),
        Some(Gid::from_raw(0)),
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

    nix::mount::mount::<Path, Path, Path, Path>(
        Some(new_cmdline_path.as_ref()),
        "/proc/cmdline".as_ref(),
        None,
        nix::mount::MsFlags::MS_BIND,
        None,
    )
    .with_context(|| "Failed to do bind mount at the cmdline.")?;
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
