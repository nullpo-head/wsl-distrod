use anyhow::{anyhow, Context, Result};
use nix::fcntl::OFlag;
use std::convert::From;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{Read, Write};
use std::process::Command;
use std::ops::{Deref, DerefMut};
use std::os::unix::io::FromRawFd;

pub struct CommandByMultiFork<'a> {
    command: Command,
    pre_second_fork: Option<Box<dyn FnMut() -> Result<()> + 'a>>,
    proxy_process: Option<ProxyProcess>,
    does_triple_fork: bool,
}

impl<'a> CommandByMultiFork<'a> {
    pub fn new<S: AsRef<OsStr>>(program: S) -> CommandByMultiFork<'a> {
        CommandByMultiFork {
            command: Command::new(program),
            pre_second_fork: None,
            proxy_process: None,
            does_triple_fork: false,
        }
    }

    pub fn do_triple_fork(&mut self, does_triple_fork: bool) -> &mut CommandByMultiFork<'a> {
        self.does_triple_fork = does_triple_fork;
        self
    }

    pub fn pre_second_fork<F>(&mut self, f: F) -> &mut CommandByMultiFork<'a>
      where F: FnMut() -> Result<()> + 'a {
        self.pre_second_fork = Some(Box::new(f));
        self
    }

    pub fn insert_waiter_proxy(&mut self) -> Result<Waiter> {
        let (proxy, waiter)  = ProxyProcess::make_pair().with_context(|| "Failed to make a proxy process.")?;
        self.proxy_process = Some(proxy);
        Ok(waiter)
    }

    pub fn spawn(&mut self) -> Result<()> {
        if unsafe { nix::unistd::fork().with_context(|| "The first fork failed")? }.is_child() {
            let mut inner = || -> Result<()> {
                if let Some(ref mut f) = self.pre_second_fork {
                    f().with_context(|| "Pre_second_fork failed.")?;
                }
                if self.does_triple_fork && unsafe { nix::unistd::fork().with_context(|| "The third fork failed.")?.is_parent() } {
                    log::debug!("The parent of the second of three forks exits.");
                    std::process::exit(0);
                }
                log::debug!("Spawning the command or the waiter.");
                match self.proxy_process {
                    None => {
                        self.command.spawn().with_context(|| "Failed to spawn the command.")?; 
                    },
                    Some(ref mut proxy_process) => {
                        log::debug!("Spawning the waiter.");
                        proxy_process.spawn(&mut self.command).with_context(|| "Failed to spawn the command.")?; 
                    },
                };
                Ok(())
            };
            if let Err(err) = inner() {
                log::error!("{:?}", err);
            }
            std::process::exit(0);
        }
        Ok(())
    }
}

impl<'a> Deref for CommandByMultiFork<'a> {
    type Target = Command;

    fn deref(&self) -> &Self::Target {
        &self.command
    }
}

impl<'a> DerefMut for CommandByMultiFork<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.command
    }
}

impl<'a> From<Command> for CommandByMultiFork<'a> {
    fn from(command: Command) -> Self {
        CommandByMultiFork {
            command,
            pre_second_fork: None,
            proxy_process: None,
            does_triple_fork: false,
        }
    }
}

pub struct Waiter {
    pipe_for_exitcode: File
}

impl Waiter {
    pub fn wait(&mut self) -> Result<u32> {
        let mut exit_code = vec![0];
        self.pipe_for_exitcode.read_exact(&mut exit_code).with_context(|| "Failed to read the exit code from the pipe.")?;
        Ok(exit_code[0] as u32)
    }
}

pub struct ProxyProcess {
    pipe_for_exitcode: File
}

impl ProxyProcess {
    pub fn make_pair() -> Result<(ProxyProcess, Waiter)> {
        let (waiter_pipe_host, waiter_pipe_child) = nix::unistd::pipe2(OFlag::O_CLOEXEC)
                                                    .with_context(|| "Failed to make a pipe.")?;
        unsafe {
            Ok((
                ProxyProcess { pipe_for_exitcode: File::from_raw_fd(waiter_pipe_child) },
                Waiter { pipe_for_exitcode: File::from_raw_fd(waiter_pipe_host) }
            ))
        }
    }

    pub fn spawn(&mut self, command: &mut Command) -> Result<()> {
        if unsafe { nix::unistd::fork().with_context(|| "The proxy_process's fork failed")? }.is_child() {
            let status = command.status().with_context(|| "Failed to run a command.")?;
            let exit_code = status.code().ok_or_else(|| anyhow!("status.code() is None unexpectedly."))? as u8;
            let exit_code = vec![exit_code];
            self.pipe_for_exitcode.write_all(&&exit_code).with_context(|| "Failed to write the exit code to the pipe.")?;
            std::process::exit(0);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_proxy() {
        let mut doublefork = CommandByMultiFork::new("/bin/bash");
        doublefork.args(&["-c", "sleep 1; exit 42"]);
        let mut waiter = doublefork.insert_waiter_proxy().unwrap();
        let _ = doublefork.spawn().unwrap();
        let exit_code = waiter.wait().unwrap();
        assert_eq!(42, exit_code);
    }
}
