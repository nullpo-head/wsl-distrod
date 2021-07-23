use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::{fs::File, io::Read};

use anyhow::{anyhow, Context, Result};

#[derive(Debug, Clone)]
pub struct PasswdFile {
    file_cont: String,
    path: PathBuf,
}

impl PasswdFile {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<PasswdFile> {
        let mut passwd_file = File::open(path.as_ref())
            .with_context(|| format!("Failed to open '{:?}'.", path.as_ref()))?;
        let mut cont = String::new();
        passwd_file
            .read_to_string(&mut cont)
            .with_context(|| format!("Failed to read the contents of '{:?}'.", path.as_ref()))?;
        Ok(PasswdFile {
            file_cont: cont,
            path: PathBuf::from(path.as_ref()),
        })
    }

    pub fn get_ent_by_name(&mut self, user_name: &str) -> Result<Option<PasswdView>> {
        for entry in self.entries() {
            let entry = entry.with_context(|| "Failed to parse '/etc/passwd'.")?;
            if entry.name == user_name {
                return Ok(Some(entry));
            }
        }
        Ok(None)
    }

    pub fn get_ent_by_uid(&mut self, uid: u32) -> Result<Option<PasswdView>> {
        for entry in self.entries() {
            let entry = entry.with_context(|| "Failed to parse '/etc/passwd'.")?;
            if entry.uid == uid {
                return Ok(Some(entry));
            }
        }
        Ok(None)
    }

    pub fn entries(&mut self) -> PasswdIterator {
        PasswdIterator {
            passwd_lines: self.file_cont.split('\n'),
        }
    }

    pub fn update(
        &mut self,
        updater: fn(passwd: PasswdView) -> Result<Option<Passwd>>,
    ) -> Result<()> {
        let mut new_cont = String::new();
        {
            for line in self.file_cont.lines() {
                let update = updater(PasswdView::deserialize(line)?);
                match update {
                    Ok(Some(passwd)) => {
                        let line = passwd.view().serialize();
                        new_cont += &line;
                        new_cont += "\n";
                    }
                    Ok(None) => {
                        new_cont += line;
                        new_cont += "\n";
                    }
                    _ => {
                        update
                            .with_context(|| format!("Failed to update the entry: '{}'.", line))?;
                    }
                }
            }
        }
        let mut passwd_file = BufWriter::new(
            File::create(&self.path).with_context(|| "Failed to create '/etc/passwd'.")?,
        );
        passwd_file
            .write_all(new_cont.as_bytes())
            .with_context(|| "Failed to write to the new /etc/passwd file.")?;
        self.file_cont = new_cont;
        Ok(())
    }
}

pub struct PasswdIterator<'a> {
    passwd_lines: std::str::Split<'a, char>,
}

impl<'a> Iterator for PasswdIterator<'a> {
    type Item = Result<PasswdView<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        let line = self.passwd_lines.next()?;
        Some(
            PasswdView::deserialize(line)
                .with_context(|| format!("Invalid format line: '{}'", line)),
        )
    }
}

#[derive(Debug, PartialEq, Clone, Eq, PartialOrd)]
pub struct Passwd {
    pub name: String,
    pub passwd: String,
    pub uid: u32,
    pub gid: u32,
    pub gecos: String,
    pub dir: String,
    pub shell: String,
}

impl Passwd {
    pub fn view(&self) -> PasswdView {
        PasswdView {
            name: &self.name,
            passwd: &self.passwd,
            uid: self.uid,
            gid: self.gid,
            gecos: &self.gecos,
            dir: &self.dir,
            shell: &self.shell,
        }
    }

    pub fn from_view(view: PasswdView) -> Self {
        Passwd {
            name: view.name.to_owned(),
            passwd: view.passwd.to_owned(),
            uid: view.uid,
            gid: view.gid,
            gecos: view.gecos.to_owned(),
            dir: view.dir.to_owned(),
            shell: view.shell.to_owned(),
        }
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd)]
pub struct PasswdView<'a> {
    pub name: &'a str,
    pub passwd: &'a str,
    pub uid: u32,
    pub gid: u32,
    pub gecos: &'a str,
    pub dir: &'a str,
    pub shell: &'a str,
}

impl PasswdView<'_> {
    pub fn serialize(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}:{}:{}",
            self.name, self.passwd, self.uid, self.gid, self.gecos, self.dir, self.shell
        )
    }

    pub fn deserialize(line: &str) -> Result<PasswdView> {
        let mut ent = line.split(':');
        Ok(PasswdView {
            name: ent
                .next()
                .ok_or_else(|| anyhow!("invalid name format /etc/passwd."))?,
            passwd: ent
                .next()
                .ok_or_else(|| anyhow!("invalid passwd format /etc/passwd."))?,
            uid: ent
                .next()
                .ok_or_else(|| anyhow!("invalid uid format /etc/passwd."))?
                .parse()
                .with_context(|| "invalid uid.")?,
            gid: ent
                .next()
                .ok_or_else(|| anyhow!("invalid gid format /etc/passwd."))?
                .parse()
                .with_context(|| "invalid gid.")?,
            gecos: ent
                .next()
                .ok_or_else(|| anyhow!("invalid gecos format /etc/passwd."))?,
            dir: ent
                .next()
                .ok_or_else(|| anyhow!("invalid dir format /etc/passwd."))?,
            shell: ent
                .next()
                .ok_or_else(|| anyhow!("invalid shell format /etc/passwd."))?,
        })
    }
}

pub fn drop_privilege(uid: u32, gid: u32) {
    let inner = || -> Result<()> {
        let uid = nix::unistd::Uid::from_raw(uid);
        let gid = nix::unistd::Gid::from_raw(gid);

        nix::unistd::setgroups(&[gid])?;
        nix::unistd::setresgid(gid, gid, gid)?;
        nix::unistd::setresuid(uid, uid, uid)?;

        Ok(())
    };
    if inner().is_err() {
        log::error!("Failed to drop_privilege. Aborting.");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use crate::distrod_config;

    use super::*;
    use std::{
        io::{Seek, SeekFrom},
        path::Path,
    };
    use tempfile::*;

    static ROOT: PasswdView = PasswdView {
        name: "root",
        passwd: "x",
        uid: 0,
        gid: 0,
        gecos: "root",
        dir: "/root",
        shell: "/bin/bash",
    };

    static NULLPO: PasswdView = PasswdView {
        name: "nullpo",
        passwd: "x",
        uid: 1000,
        gid: 1000,
        gecos: ",,,",
        dir: "/home/nullpo",
        shell: "/bin/bash",
    };

    static FOO: PasswdView = PasswdView {
        name: "foo",
        passwd: "x",
        uid: 1000,
        gid: 1000,
        gecos: ",,,",
        dir: "",
        shell: "/sbin/nologin",
    };

    #[test]
    fn test_serialize_passwd() {
        assert_eq!("root:x:0:0:root:/root:/bin/bash", ROOT.serialize());
        assert_eq!(
            "nullpo:x:1000:1000:,,,:/home/nullpo:/bin/bash",
            NULLPO.serialize()
        );
        assert_eq!("foo:x:1000:1000:,,,::/sbin/nologin", FOO.serialize());
    }

    #[test]
    fn test_deserialize_passwd() -> Result<()> {
        assert_eq!(
            ROOT,
            PasswdView::deserialize("root:x:0:0:root:/root:/bin/bash")?
        );
        assert_eq!(
            NULLPO,
            PasswdView::deserialize("nullpo:x:1000:1000:,,,:/home/nullpo:/bin/bash")?
        );
        assert_eq!(
            FOO,
            PasswdView::deserialize("foo:x:1000:1000:,,,::/sbin/nologin")?
        );
        Ok(())
    }

    #[test]
    fn test_read_passwd_file() -> Result<()> {
        let mut tmp = NamedTempFile::new()?;
        writeln!(&mut tmp, "root:x:0:0:root:/root:/bin/bash")?;
        writeln!(&mut tmp, "nullpo:x:1000:1000:,,,:/home/nullpo:/bin/bash")?;
        writeln!(&mut tmp, "foo:x:1000:1000:,,,::/sbin/nologin")?;
        let mut passwd_file = PasswdFile::open(tmp.path())?;
        let mut entries = passwd_file.entries();
        assert_eq!(ROOT, entries.next().unwrap()?);
        assert_eq!(NULLPO, entries.next().unwrap()?);
        assert_eq!(FOO, entries.next().unwrap()?);
        Ok(())
    }

    #[test]
    fn test_update_passwd_file_no_update() -> Result<()> {
        let mut tmp = NamedTempFile::new()?;
        writeln!(&mut tmp, "root:x:0:0:root:/root:/bin/bash")?;
        writeln!(&mut tmp, "nullpo:x:1000:1000:,,,:/home/nullpo:/bin/bash")?;
        writeln!(&mut tmp, "foo:x:1000:1000:,,,::/sbin/nologin")?;
        tmp.seek(SeekFrom::Start(0))?;
        let mut orig_cont = String::new();
        tmp.read_to_string(&mut orig_cont)?;

        let mut passwd_file = PasswdFile::open(tmp.path())?;
        passwd_file.update(|_| Ok(None))?;

        let mut entries = passwd_file.entries();
        assert_eq!(ROOT, entries.next().unwrap()?);
        assert_eq!(NULLPO, entries.next().unwrap()?);
        assert_eq!(FOO, entries.next().unwrap()?);

        let mut new_cont = String::new();
        tmp.seek(SeekFrom::Start(0))?;
        tmp.read_to_string(&mut new_cont)?;
        assert_eq!(orig_cont, new_cont);

        Ok(())
    }

    #[test]
    fn test_update_passwd_file() -> Result<()> {
        let mut tmp = NamedTempFile::new()?;
        writeln!(&mut tmp, "root:x:0:0:root:/root:/bin/bash")?;
        writeln!(&mut tmp, "nullpo:x:1000:1000:,,,:/home/nullpo:/bin/bash")?;
        writeln!(&mut tmp, "foo:x:1000:1000:,,,::/sbin/nologin")?;

        let mut passwd_file = PasswdFile::open(tmp.path())?;
        passwd_file.update(|passwd| {
            let mut new_shell = PathBuf::from(distrod_config::get_alias_dir());
            new_shell.push(Path::new(passwd.shell).strip_prefix("/").unwrap());
            Ok(Some(Passwd {
                name: passwd.name.to_owned(),
                passwd: passwd.passwd.to_owned(),
                uid: passwd.uid,
                gid: passwd.gid,
                gecos: passwd.gecos.to_owned(),
                dir: passwd.dir.to_owned(),
                shell: new_shell.to_str().map(|s| s.to_owned()).unwrap(),
            }))
        })?;

        let expected = "root:x:0:0:root:/root:/opt/distrod/alias/bin/bash\n\
                             nullpo:x:1000:1000:,,,:/home/nullpo:/opt/distrod/alias/bin/bash\n\
                             foo:x:1000:1000:,,,::/opt/distrod/alias/sbin/nologin\n";
        let mut new_cont = String::new();
        let mut file = File::open(tmp.path())?;
        file.read_to_string(&mut new_cont)?;
        assert_eq!(expected, new_cont);

        Ok(())
    }
}
