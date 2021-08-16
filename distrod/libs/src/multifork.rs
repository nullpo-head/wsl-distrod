use anyhow::{anyhow, Context, Result};
use nix::fcntl::OFlag;
use nix::libc::c_int;
use nix::sys::signal;
use std::convert::From;
use std::fs::File;
use std::io::{Read, Write};
use std::ops::Deref;
use std::os::unix::io::FromRawFd;
use std::os::unix::prelude::CommandExt;
use std::process::Command;

pub struct CommandByMultiFork<'a> {
    command: Command,
    pre_second_fork: Option<Box<dyn FnMut() -> Result<()> + 'a>>,
    proxy_process: Option<ProxyProcess>,
    does_triple_fork: bool,
}

impl<'a> CommandByMultiFork<'a> {
    pub fn new(command: Command) -> CommandByMultiFork<'a> {
        CommandByMultiFork {
            command,
            pre_second_fork: None,
            proxy_process: None,
            does_triple_fork: false,
        }
    }

    pub fn do_triple_fork(&mut self, does_triple_fork: bool) -> &mut CommandByMultiFork<'a> {
        self.does_triple_fork = does_triple_fork;
        self
    }

    // Define proxy function to allow it to be called before pre_second_fork for readability.
    pub unsafe fn pre_exec<F>(&mut self, f: F) -> &mut CommandByMultiFork<'a>
    where
        F: FnMut() -> std::io::Result<()> + Send + Sync + 'static,
    {
        self.command.pre_exec(f);
        self
    }

    pub fn pre_second_fork<F>(&mut self, f: F) -> &mut CommandByMultiFork<'a>
    where
        F: FnMut() -> Result<()> + 'a,
    {
        self.pre_second_fork = Some(Box::new(f));
        self
    }

    pub fn insert_waiter_proxy(&mut self) -> Result<Waiter> {
        let (proxy, waiter) =
            ProxyProcess::make_pair().with_context(|| "Failed to make a proxy process.")?;
        self.proxy_process = Some(proxy);
        Ok(waiter)
    }

    pub fn spawn(mut self) -> Result<()> {
        if unsafe { nix::unistd::fork().with_context(|| "The first fork failed")? }.is_child() {
            let inner = || -> Result<()> {
                if let Some(ref mut f) = self.pre_second_fork {
                    f().with_context(|| "Pre_second_fork failed.")?;
                }
                if self.does_triple_fork
                    && unsafe {
                        nix::unistd::fork()
                            .with_context(|| "The third fork failed.")?
                            .is_parent()
                    }
                {
                    log::debug!("The parent of the second of three forks exits.");
                    std::process::exit(0);
                }
                log::debug!("Spawning the command or the waiter.");
                match self.proxy_process {
                    None => {
                        self.command
                            .spawn()
                            .with_context(|| "Failed to spawn the command.")?;
                    }
                    Some(proxy_process) => {
                        log::debug!("Spawning the waiter.");
                        proxy_process
                            .spawn(&mut self.command)
                            .with_context(|| "Failed to spawn the command.")?;
                    }
                };
                Ok(())
            };
            if let Err(err) = inner() {
                log::error!("{:?}", err);
            }
            std::process::exit(0);
        }
        self.proxy_process = None; // Drop the proxy process in the parent process and drop the writer pipe.
        Ok(())
    }
}

impl<'a> Deref for CommandByMultiFork<'a> {
    type Target = Command;

    fn deref(&self) -> &Self::Target {
        &self.command
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
    pipe_for_exitcode: File,
}

impl Waiter {
    pub fn wait(&mut self) -> u32 {
        let mut exit_code = vec![137]; // The exit code for SIGKILL
        let res = self
            .pipe_for_exitcode
            .read_exact(&mut exit_code)
            .with_context(|| "Failed to read the exit code from the pipe.");
        if res.is_err() {
            log::debug!(
                "The pipe for wait has been closed. Possibly the proxy process has been killed by SIGKILL."
            );
        }
        exit_code[0] as u32
    }
}

pub struct ProxyProcess {
    pipe_for_exitcode: File,
}

impl ProxyProcess {
    pub fn make_pair() -> Result<(ProxyProcess, Waiter)> {
        let (waiter_pipe_host, waiter_pipe_child) =
            nix::unistd::pipe2(OFlag::O_CLOEXEC).with_context(|| "Failed to make a pipe.")?;
        unsafe {
            Ok((
                ProxyProcess {
                    pipe_for_exitcode: File::from_raw_fd(waiter_pipe_child),
                },
                Waiter {
                    pipe_for_exitcode: File::from_raw_fd(waiter_pipe_host),
                },
            ))
        }
    }

    pub fn spawn(mut self, command: &mut Command) -> Result<()> {
        if unsafe { nix::unistd::fork().with_context(|| "The proxy_process's fork failed")? }
            .is_child()
        {
            set_noninheritable_sig_ign();
            let mut child = command
                .spawn()
                .with_context(|| "Failed to run a command.")?;
            let status = child
                .wait()
                .with_context(|| "Failed to wait wthe command.")?;
            let exit_code = status
                .code()
                .ok_or_else(|| anyhow!("status.code() is None unexpectedly."))?
                as u8;
            let exit_code = vec![exit_code];
            if let Err(e) = self.pipe_for_exitcode.write_all(&&exit_code) {
                log::debug!("Failed to write the exit code to the pipe. {}", e);
            }
            std::process::exit(0);
        }
        Ok(())
    }
}

pub fn set_noninheritable_sig_ign() {
    for signal in signal::Signal::iterator() {
        // Ignore signals by a function instead of SIG_IGN so that the child doesn't inherit it.
        if let Err(e) = unsafe { signal::signal(signal, signal::SigHandler::Handler(do_nothing)) } {
            if signal != signal::Signal::SIGSYS {
                log::debug!("Failed to ignore signal {:?}", e);
            }
        }
    }
}

extern "C" fn do_nothing(_sig: c_int) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_proxy() {
        let mut command = Command::new("/bin/bash");
        command.args(&["-c", "sleep 1; exit 42"]);
        let mut doublefork = CommandByMultiFork::new(command);
        let mut waiter = doublefork.insert_waiter_proxy().unwrap();
        let _ = doublefork.spawn().unwrap();
        let exit_code = waiter.wait();
        assert_eq!(42, exit_code);
    }

    #[test]
    fn test_inserted_proxy_ignore_signal() {
        let mut command = Command::new("/bin/bash");
        command.args(&[
            "-c",
            "trap '' SIGINT; kill -SIGINT $PPID; sleep 1; exit 42;",
        ]);
        let mut doublefork = CommandByMultiFork::new(command);
        let mut waiter = doublefork.insert_waiter_proxy().unwrap();
        let _ = doublefork.spawn().unwrap();
        let exit_code = waiter.wait();
        assert_eq!(42, exit_code);
    }
}
