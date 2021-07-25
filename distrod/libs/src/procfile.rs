use anyhow::{anyhow, bail, Context, Result};
use nix::fcntl::OFlag;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::io::{AsRawFd, FromRawFd};

#[derive(Debug)]
pub struct ProcFile {
    dir: File,
}

impl ProcFile {
    pub fn current_proc() -> Result<ProcFile> {
        let procfile =
            ProcFile::from_proc_dir("self").with_context(|| "Failed to open /proc/self.")?;
        Ok(procfile.ok_or_else(|| anyhow!("/proc/self doesn't exist."))?)
    }

    #[allow(dead_code)]
    pub fn from_pid(pid: u32) -> Result<Option<ProcFile>> {
        ProcFile::from_proc_dir(pid.to_string().as_str())
    }

    pub fn is_live(&mut self) -> bool {
        self.pid().is_ok()
    }

    pub unsafe fn from_raw_fd(pidfd: i32) -> ProcFile {
        ProcFile {
            dir: File::from_raw_fd(pidfd),
        }
    }

    pub fn as_raw_fd(&self) -> i32 {
        self.dir.as_raw_fd()
    }

    pub fn pid(&mut self) -> Result<u32> {
        let statfd = nix::fcntl::openat(
            self.dir.as_raw_fd(),
            "stat",
            OFlag::O_RDONLY | OFlag::O_CLOEXEC,
            nix::sys::stat::Mode::empty(),
        )
        .with_context(|| "Failed to open /proc/self/stat")?;
        if statfd < 0 {
            bail!("The process doesn't exist");
        }
        let mut stat = unsafe { File::from_raw_fd(statfd) };
        let mut stat_cont = String::new();
        stat.read_to_string(&mut stat_cont)?;
        stat.seek(SeekFrom::Start(0))?;
        let pid = stat_cont
            .split(' ')
            .next() // 0: PID
            .ok_or_else(|| anyhow!("Failed to read pid from the stat file."))?
            .parse()
            .with_context(|| "Failed to parse the pid.")?;
        Ok(pid)
    }

    pub fn open_file_at(&self, name: &str) -> Result<File> {
        let nsdir_fd = nix::fcntl::openat(
            self.dir.as_raw_fd(),
            name,
            OFlag::O_RDONLY,
            nix::sys::stat::Mode::empty(),
        )
        .with_context(|| format!("Failed to open {}", name))?;
        Ok(unsafe { File::from_raw_fd(nsdir_fd) })
    }

    fn from_proc_dir(proc_dir: &str) -> Result<Option<ProcFile>> {
        let piddirfd = nix::fcntl::open(
            format!("/proc/{}", proc_dir).as_str(),
            OFlag::O_RDONLY | OFlag::O_CLOEXEC,
            nix::sys::stat::Mode::empty(),
        );
        if let Err(nix::Error::Sys(nix::errno::Errno::ENOENT)) = piddirfd {
            return Ok(None);
        }
        Ok(Some(ProcFile {
            dir: unsafe {
                File::from_raw_fd(
                    piddirfd.with_context(|| format!("Failed to open /proc/{}", proc_dir))?,
                )
            },
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn test_current_proc() {
        assert!(ProcFile::current_proc().is_ok());
    }

    #[test]
    fn test_current_pid() {
        let mut current = ProcFile::current_proc().unwrap();
        assert_eq!(std::process::id(), current.pid().unwrap());
    }

    #[test]
    fn test_child_pid() {
        let mut child = Command::new("/bin/sleep");
        child.args(&["2"]);
        let child = child.spawn().unwrap();
        let mut child_procfile = ProcFile::from_pid(child.id()).unwrap().unwrap();
        assert_eq!(child.id(), child_procfile.pid().unwrap());
    }

    #[test]
    fn test_proc_liveness() {
        let mut child = Command::new("/bin/sleep");
        child.args(&["2"]);
        let mut child = child.spawn().unwrap();
        let mut child_procfile = ProcFile::from_pid(child.id()).unwrap().unwrap();
        assert!(child_procfile.is_live());
        let _ = child.wait();
        assert!(!child_procfile.is_live());
        assert!(ProcFile::from_pid(child.id()).unwrap().is_none());
    }
}
