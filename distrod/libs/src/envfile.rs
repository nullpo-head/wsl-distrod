use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};

pub struct EnvFile {
    pub path: PathBuf,
    // Vec for abnormal files which contains duplicated env definitions.
    // u64 for the index of the line.
    envs: HashMap<String, Vec<(usize, String)>>,
}

impl EnvFile {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<EnvFile> {
        let mut envs: HashMap<String, Vec<(usize, String)>> = HashMap::new();
        let file = BufReader::new(
            File::open(path.as_ref())
                .with_context(|| format!("Failed to open {:?}", path.as_ref()))?,
        );
        for (i, line) in file.lines().enumerate() {
            let line = line.with_context(|| "Failed to read a line.")?;
            let sep_i = line.find('=');
            if sep_i.is_none() {
                bail!(format!(
                    "invalid /etc/environment file. No '=' is found. line: {}.",
                    i
                ));
            }
            let sep_i = sep_i.unwrap();
            let env_name = &line[0..sep_i];
            let env_value = &line[sep_i + 1..];

            match envs.get_mut(env_name) {
                Some(vals) => {
                    vals.push((i, env_value.to_owned()));
                }
                None => {
                    envs.insert(env_name.to_owned(), vec![(i, env_value.to_owned())]);
                }
            }
        }

        Ok(EnvFile {
            path: path.as_ref().to_owned(),
            envs,
        })
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        let val = self.envs.get(key)?;
        // return the value of the last line.
        Some(val.last()?.1.as_str())
    }

    pub fn put(&mut self, key: &str, value: String) {
        if let Some(existing_vals) = self.envs.get_mut(key) {
            if !existing_vals.is_empty() {
                let (line, _) = existing_vals.pop().expect("val is not empty");
                existing_vals.push((line, value));
                return;
            }
        }
        self.envs.insert(key.to_owned(), vec![(usize::MAX, value)]);
    }

    pub fn save(&mut self) -> Result<()> {
        let mut lines = self
            .envs
            .iter()
            .flat_map(|(key, vals)| {
                vals.iter()
                    .map(|(line, val)| (*line, format!("{}={}\n", key, val)))
                    .collect::<Vec<(usize, String)>>()
            })
            .collect::<Vec<(usize, String)>>();
        lines.sort();

        let mut file = BufWriter::new(
            File::create(&self.path)
                .with_context(|| format!("Failed to create {:?}.", &self.path))?,
        );
        for line in lines {
            file.write_all(line.1.as_bytes())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::*;

    #[test]
    fn test_get() {
        let mut tmp = NamedTempFile::new().unwrap();
        let cont = "\
		    PATH=test:foo:bar\n\
			FOO=foo\n\
			BAR=bar\n\
			BAZ=baz=baz\n\
			FOO=foo2\n\
		";
        write!(&mut tmp, "{}", cont).unwrap();
        let env = EnvFile::open(tmp.path()).unwrap();

        assert_eq!(env.get("None"), None);
        assert_eq!(env.get("PATH"), Some("test:foo:bar"));
        assert_eq!(env.get("BAZ"), Some("baz=baz"));
        assert_eq!(
            env.get("FOO"),
            Some("foo2"),
            "The last value is obtained if the environment has multiple values."
        );
    }

    #[test]
    fn test_put_and_save() {
        let mut tmp = NamedTempFile::new().unwrap();
        let cont = "\
		    PATH=test:foo:bar\n\
			FOO=foo\n\
			BAR=bar\n\
			BAZ=baz=baz\n\
			FOO=foo2\n\
		";
        write!(&mut tmp, "{}", cont).unwrap();
        let mut env = EnvFile::open(tmp.path()).unwrap();

        env.put("NEW1", "NEW1".to_owned());
        env.put("PATH", format!("path:{}", env.get("PATH").unwrap()));
        env.put("FOO", "foo3".to_owned());

        assert_eq!(env.get("None"), None);
        assert_eq!(env.get("NEW1"), Some("NEW1"));
        assert_eq!(env.get("PATH"), Some("path:test:foo:bar"));
        assert_eq!(env.get("FOO"), Some("foo3"));

        env.save().unwrap();
        let expected = "\
		    PATH=path:test:foo:bar\n\
			FOO=foo\n\
			BAR=bar\n\
			BAZ=baz=baz\n\
			FOO=foo3\n\
			NEW1=NEW1\n\
		";
        let new_cont = std::fs::read_to_string(tmp.path()).unwrap();
        assert_eq!(new_cont, expected);
    }
}
