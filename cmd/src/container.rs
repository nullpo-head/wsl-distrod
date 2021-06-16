use std::ffi::CString;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use nix::sched::CloneFlags;
use nix::NixPath;

use anyhow::{bail, Context, Result};

pub struct Container {
    root_fs: PathBuf,
}

impl Container {
    pub fn new<P: AsRef<Path>>(root_fs: P) -> Result<Self> {
        Ok(Container {
            root_fs: PathBuf::from(root_fs.as_ref()),
        })
    }

    pub fn launch<P: AsRef<Path>>(&mut self, init: Option<Vec<String>>, old_root: P) -> Result<()> {
        let old_root = self.root_fs.join(old_root.as_ref().strip_prefix("/")?);
        let fork_result = unsafe {nix::unistd::fork().with_context(|| "Fork failed")?};
        if fork_result.is_child() {
            let init = init.unwrap_or(vec!["/sbin/init".to_owned(), "--unit=multi-user.target".to_owned()]);
            daemonize().with_context(|| "The container failed to be daemonized.")?;
            self.prepare_namespace().with_context(|| "Failed to initialize Linux namespaces.")?;
            self.prepare_filesystem(&old_root).with_context(|| "Failed to initialize the container's filesystem.")?;
            self.launch_init(init);
        }
        std::thread::sleep(std::time::Duration::from_secs(30));
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
        nix::mount::mount::<Path, Path, Path, Path>(Some(&self.root_fs), &self.root_fs, None, nix::mount::MsFlags::MS_BIND, None).with_context(|| "Failed to bind mount the old_root")?;
        let b = PathBuf::from(old_root.as_ref());
        std::env::set_current_dir(&self.root_fs).with_context(|| format!("Failed to cd {:#?}", &self.root_fs))?;
        nix::unistd::pivot_root(".", old_root.as_ref()).with_context(|| format!("pivot_root failed. new: {:#?}, old: {:#?}", &self.root_fs, &old_root.as_ref()))?;
        nix::mount::mount::<Path, Path, Path, Path>(None, "/proc".as_ref(), Some("proc".as_ref()), nix::mount::MsFlags::empty(), None).with_context(|| "mount /proc failed.")?;
        Ok(())
    }

    fn mount_wsl_mountpoints<P: AsRef<Path>>(&self, old_root: P, mount_entries: &Vec<MountEntry>) -> Result<()> {
        let mut old_root = PathBuf::from(old_root.as_ref());
        let binds = ["init", "proc/sys/fs/binfmt_misc", "run", "tmp"];
        for bind in binds.iter() {
            let num_dirs = bind.matches('/').count();
            old_root.push(bind);
            if !old_root.exists() {
                log::debug!("WSL path {:?} does not exist", old_root.to_str());
                continue;
            }
            nix::mount::mount::<Path, Path, Path, Path>(Some(old_root.as_path()), bind.as_ref(), None, nix::mount::MsFlags::MS_BIND, None).with_context(|| "Failed to mount the WSL's special dirs")?;
            for _ in 0..num_dirs {
                old_root.pop();
            }
        }

        let mut init = old_root.clone();
        init.push("init");
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
            let path = path.strip_prefix(&old_root).with_context(|| format!("Unexpected error. strip_prefix failed for {:?}", &path))?;
            nix::mount::mount::<Path, Path, Path, Path>(Some(path), path.as_ref(), None, nix::mount::MsFlags::MS_BIND, None).with_context(|| "Failed to mount the Windows drives")?;
        }
        Ok(())
    }

    fn umount_host_mountpoints<P: AsRef<Path>>(&self, old_root: P, mount_entries: &Vec<MountEntry>) -> Result<()> {
        Ok(())
    }

    fn launch_init(&self, init: Vec<String>) -> ! {
        nix::unistd::execv(&CString::new("/bin/bash").unwrap(), &[CString::new("/bin/bash").unwrap()]).unwrap();
        std::process::exit(1);
    }
}

fn daemonize() -> Result<()> {
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
        mount_entries.push(MountEntry { path: PathBuf::from(path), fstype });
    }

    Ok(mount_entries)
}
