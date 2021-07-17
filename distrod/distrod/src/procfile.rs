use anyhow::{anyhow, Context, Result};
use nix::fcntl::OFlag;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::io::{AsRawFd, FromRawFd};

pub struct ProcFile {
    stat: File,
}

impl ProcFile {
    pub fn current_proc() -> Result<ProcFile> {
        let procfile =
            ProcFile::from_proc_dir("self").with_context(|| "Failed to open /proc/self/stat.")?;
        Ok(procfile.ok_or_else(|| anyhow!("/proc/self/stat doesn't exist."))?)
    }

    #[allow(dead_code)]
    pub fn from_pid(pid: u32) -> Result<Option<ProcFile>> {
        ProcFile::from_proc_dir(pid.to_string().as_str())
    }

    pub unsafe fn from_raw_fd(pidfd: i32) -> ProcFile {
        ProcFile {
            stat: File::from_raw_fd(pidfd),
        }
    }

    pub fn as_raw_fd(&self) -> i32 {
        self.stat.as_raw_fd()
    }

    pub fn pid(&mut self) -> Result<u32> {
        let mut stat_cont = String::new();
        self.stat.read_to_string(&mut stat_cont)?;
        self.stat.seek(SeekFrom::Start(0))?;
        let pid = stat_cont
            .split(' ')
            .next() // 0: PID
            .ok_or_else(|| anyhow!("Failed to read pid from the stat file."))?
            .parse()
            .with_context(|| "Failed to parse the pid.")?;
        Ok(pid)
    }

    fn from_proc_dir(proc_dir: &str) -> Result<Option<ProcFile>> {
        let pidfd = nix::fcntl::open(
            format!("/proc/{}/stat", proc_dir).as_str(),
            OFlag::O_RDONLY | OFlag::O_CLOEXEC,
            nix::sys::stat::Mode::empty(),
        )
        .with_context(|| "Failed to open /proc/self/stat")?;
        if pidfd < 0 {
            return Ok(None);
        }
        Ok(Some(ProcFile {
            stat: unsafe { File::from_raw_fd(pidfd) },
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
        child.args(&["3"]);
        let child = child.spawn().unwrap();
        let mut child_procfile = ProcFile::from_pid(child.id()).unwrap().unwrap();
        assert_eq!(child.id(), child_procfile.pid().unwrap());
    }
}
