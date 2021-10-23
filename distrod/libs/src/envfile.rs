use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{BufRead, BufReader, BufWriter, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};

#[derive(Debug, Clone)]
pub struct ProfileDotDScript {
    script_name: String,
    dot_profile_dir_path: PathBuf,
    env_shell_script: EnvShellScript,
}

impl ProfileDotDScript {
    pub fn open<P: AsRef<Path>>(script_name: String, rootfs_path: P) -> Option<Self> {
        let dot_profile_dir_path = rootfs_path.as_ref().join("etc/profile.d");
        if !dot_profile_dir_path.exists() {
            return None;
        }
        Some(ProfileDotDScript {
            script_name,
            dot_profile_dir_path,
            env_shell_script: EnvShellScript::new(),
        })
    }

    pub fn put_env(&mut self, name: String, value: String) {
        self.env_shell_script.put_env(name, value);
    }

    pub fn put_path(&mut self, path: String) {
        self.env_shell_script.put_path(path);
    }

    pub fn write(&self) -> Result<()> {
        let script_path = self.dot_profile_dir_path.join(&self.script_name);
        self.env_shell_script
            .write(&script_path)
            .with_context(|| format!("Failed to write envsh script to {:?}", &script_path))?;
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct EnvShellScript {
    envs: HashMap<String, String>,
    paths: HashSet<String>,
}

impl EnvShellScript {
    pub fn new() -> Self {
        EnvShellScript::default()
    }

    pub fn put_env(&mut self, key: String, value: String) {
        self.envs.insert(key, value);
    }

    pub fn put_path(&mut self, path: String) {
        self.paths.insert(path);
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        let mut file = BufWriter::new(
            std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .mode(0o755)
                .open(path)
                .with_context(|| format!("Failed to create {:?}.", path))?,
        );
        let script = self.gen_shell_script();
        file.write_all(script.as_bytes())?;

        Ok(())
    }

    fn gen_shell_script(&self) -> String {
        let mut script = String::new();
        let mut envs: Vec<(_, _)> = self.envs.iter().collect();
        envs.sort_by(|(key_a, _), (key_b, _)| key_a.cmp(key_b));
        for (key, value) in envs {
            script.push_str(&format!(
                "if [ -z \"${{{}:-}}\" ]; then export {}={}; fi\n",
                key,
                key,
                single_quote_str_for_shell(value)
            ));
        }
        let mut paths: Vec<_> = self.paths.iter().collect();
        paths.sort();
        for path in paths {
            script.push_str(&format!(
                "__CANDIDATE_PATH={}\n\
                 __COLON_PATH=\":${{PATH}}:\"\n\
                 if [ \"${{__COLON_PATH#*:${{__CANDIDATE_PATH}}:}}\" = \"${{__COLON_PATH}}\" ]; then export PATH=\"${{__CANDIDATE_PATH}}:${{PATH}}\"; fi\n\
                 unset __CANDIDATE_PATH\n\
                 unset __COLON_PATH\n",
                single_quote_str_for_shell(path)
            ));
        }
        script
    }
}

fn single_quote_str_for_shell(s: &str) -> String {
    format!("'{}'", s.replace("'", "'\"'\"'"))
}
#[derive(Debug, Clone)]
pub struct EnvFile {
    pub file_path: PathBuf,
    // Vec for abnormal files which contains duplicated env definitions.
    // u64 for the index of the line.
    envs: HashMap<String, Vec<(usize, String)>>,
}

impl EnvFile {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<EnvFile> {
        let envs =
            match parse_env_file(path.as_ref()).with_context(|| "Failed to parse an env file.") {
                Ok(envs) => envs,
                Err(e)
                    if e.downcast_ref::<std::io::Error>().map(|e| e.kind())
                        == Some(std::io::ErrorKind::NotFound) =>
                {
                    HashMap::<String, Vec<(usize, String)>>::default()
                }
                Err(e) => bail!(e),
            };
        Ok(EnvFile {
            file_path: path.as_ref().to_owned(),
            envs,
        })
    }

    pub fn get_env(&self, key: &str) -> Option<&str> {
        let val = self.envs.get(key)?;
        // return the value of the last line.
        Some(val.last()?.1.as_str())
    }

    pub fn put_env(&mut self, key: String, value: String) {
        if let Some(existing_vals) = self.envs.get_mut(&key) {
            if !existing_vals.is_empty() {
                let (line, _) = existing_vals.pop().expect("val is not empty");
                existing_vals.push((line, value));
                return;
            }
        }
        self.envs.insert(key, vec![(usize::MAX, value)]);
    }

    pub fn put_path(&mut self, path_val: String) {
        const DEFAULT_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/usr/games:/usr/local/games";
        let pathenv_value = {
            let mut path_variable =
                PathVariable::parse(self.get_env("PATH").unwrap_or(DEFAULT_PATH));
            path_variable.put_path(&path_val);
            path_variable.serialize()
        };
        self.put_env("PATH".to_owned(), pathenv_value);
    }

    pub fn remove_env(&mut self, key: &str) {
        self.envs.remove(key);
    }

    pub fn write(&mut self) -> Result<()> {
        let lines = self.serialize_to_env_file();
        let mut file = BufWriter::new(
            File::create(&self.file_path)
                .with_context(|| format!("Failed to create {:?}.", &self.file_path))?,
        );
        for line in lines {
            file.write_all(line.1.as_bytes())?;
        }
        Ok(())
    }

    fn serialize_to_env_file(&mut self) -> Vec<(usize, String)> {
        let serialize_env = |key: &str, vals: &Vec<(usize, String)>| -> Vec<(usize, String)> {
            vals.iter()
                .map(|(line_num, val)| (*line_num, format!("{}={}\n", key, val)))
                .collect::<Vec<(usize, String)>>()
        };
        let mut lines = self
            .envs
            .iter()
            .flat_map(|(key, vals)| serialize_env(key, vals))
            .collect::<Vec<(usize, String)>>();
        lines.sort();
        lines
    }
}

fn parse_env_file<P: AsRef<Path>>(path: P) -> Result<HashMap<String, Vec<(usize, String)>>> {
    let mut envs: HashMap<String, Vec<(usize, String)>> = HashMap::new();
    let reader = BufReader::new(
        File::open(path.as_ref()).with_context(|| format!("Failed to open {:?}", path.as_ref()))?,
    );
    for (i, line) in reader.lines().enumerate() {
        let line = line.with_context(|| "Failed to read a line.")?;
        if line.trim().is_empty() {
            continue;
        }
        let sep_i = line.find('=');
        if sep_i.is_none() {
            log::debug!(
                "invalid /etc/environment file. No '=' is found. line: {}.",
                i
            );
            continue;
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
    Ok(envs)
}

#[derive(Debug, Clone)]
pub struct PathVariable<'a> {
    parsed_paths: Vec<&'a str>,
    added_paths: Vec<&'a str>,
    path_set: HashSet<&'a str>,
    has_surrounding_quote: bool,
}

impl<'a> PathVariable<'a> {
    pub fn parse(val: &'a str) -> Self {
        let mut paths: Vec<_> = val.split(':').into_iter().collect();

        // Roughly regard the whole path is surrounded by double quotes by simple logic
        let has_surrounding_quote = paths
            .first()
            .map_or(false, |path| path.starts_with('"') && !path.ends_with('"'))
            && paths
                .last()
                .map_or(false, |path| !path.starts_with('"') && path.ends_with('"'));

        if has_surrounding_quote {
            paths[0] = &paths[0][1..];
            let len = paths.len();
            paths[len - 1] = &paths[len - 1][..paths[len - 1].len() - 1];
        }

        let mut path_set = HashSet::<&str>::new();
        for path in paths.iter() {
            path_set.insert(*path);
        }

        PathVariable {
            parsed_paths: paths,
            added_paths: vec![],
            path_set: HashSet::<&str>::new(),
            has_surrounding_quote,
        }
    }

    pub fn serialize(&self) -> String {
        let paths = self.iter().collect::<Vec<_>>().join(":");
        if self.has_surrounding_quote {
            format!("\"{}\"", &paths)
        } else {
            paths
        }
    }

    pub fn put_path(&mut self, path_val: &'a str) {
        if self.path_set.contains(path_val) {
            return;
        }
        self.added_paths.push(path_val);
        self.path_set
            .insert(self.added_paths[self.added_paths.len() - 1]);
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.added_paths
            .iter()
            .rev()
            .chain(self.parsed_paths.iter())
            .copied()
    }
}

#[cfg(test)]
mod test_env_shell_script {
    use super::*;

    #[test]
    fn test_simple_env_shell_script() {
        let mut env_shell_script = EnvShellScript::new();
        env_shell_script.put_env("var1".to_owned(), "val1".to_owned());
        env_shell_script.put_env("var2".to_owned(), "val2".to_owned());
        env_shell_script.put_env("var_space".to_owned(), "value with space".to_owned());
        env_shell_script.put_env("var2".to_owned(), "val2 again".to_owned());

        env_shell_script.put_path("/path/to/somewhere".to_owned());
        env_shell_script.put_path("/path/with space/somewhere".to_owned());
        env_shell_script.put_path("/path/to/somewhere".to_owned());

        let script = env_shell_script.gen_shell_script();
        assert_eq!(
            "if [ -z \"${var1:-}\" ]; then export var1='val1'; fi\n\
             if [ -z \"${var2:-}\" ]; then export var2='val2 again'; fi\n\
             if [ -z \"${var_space:-}\" ]; then export var_space='value with space'; fi\n\
             __CANDIDATE_PATH='/path/to/somewhere'\n\
             __COLON_PATH=\":${PATH}:\"\n\
             if [ \"${__COLON_PATH#*:${__CANDIDATE_PATH}:}\" = \"${__COLON_PATH}\" ]; then export PATH=\"${__CANDIDATE_PATH}:${PATH}\"; fi\n\
             unset __CANDIDATE_PATH\n\
             unset __COLON_PATH\n\
             __CANDIDATE_PATH='/path/with space/somewhere'\n\
             __COLON_PATH=\":${PATH}:\"\n\
             if [ \"${__COLON_PATH#*:${__CANDIDATE_PATH}:}\" = \"${__COLON_PATH}\" ]; then export PATH=\"${__CANDIDATE_PATH}:${PATH}\"; fi\n\
             unset __CANDIDATE_PATH\n\
             unset __COLON_PATH\n",
            &script
        );
    }

    #[test]
    fn test_script_by_shell() {
        let mut env_shell_script = EnvShellScript::new();
        env_shell_script.put_env("var_space".to_owned(), "value with space".to_owned());
        env_shell_script.put_env("existing_var".to_owned(), "updated".to_owned());
        env_shell_script.put_path("/path/to/somewhere".to_owned());
        env_shell_script.put_path("/path/with space/somewhere".to_owned());
        env_shell_script.put_path("/path/with space/somewhere".to_owned());
        env_shell_script.put_path("/bin".to_owned());

        let mut script = env_shell_script.gen_shell_script();
        script.push_str(
            "\
            echo $var_space\n\
            echo $existing_var\n\
            echo $PATH\n\
        ",
        );

        let mut shell = std::process::Command::new("sh");
        shell.arg("-c");
        shell.arg(&script);
        shell.env("existing_var", "not updated");
        shell.env("PATH", "/usr/local/bin:/sbin:/bin");
        let output = shell.output().unwrap();
        eprintln!("stderr: {}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(
            "value with space\nnot updated\n/path/with space/somewhere:/path/to/somewhere:/usr/local/bin:/sbin:/bin\n",
            &String::from_utf8_lossy(&output.stdout)
        );
    }
}

#[cfg(test)]
mod test_path_variable {
    use super::*;

    #[test]
    fn test_simple_variable() {
        let path_value = "/usr/local/bin:/usr/bin:/sbin:/bin";
        let mut path = PathVariable::parse(path_value);
        assert_eq!(path_value, path.serialize().as_str());

        path.put_path("/new/path1/bin");
        path.put_path("/new/path2/bin");
        path.put_path("/new/path2/bin"); // Put the same path again
        assert_eq!(
            format!("/new/path2/bin:/new/path1/bin:{}", path_value),
            path.serialize()
        );

        assert_eq!(
            vec![
                "/new/path2/bin",
                "/new/path1/bin",
                "/usr/local/bin",
                "/usr/bin",
                "/sbin",
                "/bin"
            ],
            path.iter().collect::<Vec<&str>>()
        );
    }

    #[test]
    fn test_quoted_variable() {
        // quoted simple value
        let path_value = "\"/usr/local/bin:/usr/bin:/sbin:/bin\"";
        let mut path = PathVariable::parse(path_value);
        assert_eq!(path_value, path.serialize().as_str());

        assert_eq!(
            vec!["/usr/local/bin", "/usr/bin", "/sbin", "/bin"],
            path.iter().collect::<Vec<&str>>()
        );

        path.put_path("/new/path1/bin");
        path.put_path("/new/path2/bin");
        assert_eq!(
            format!(
                "\"/new/path2/bin:/new/path1/bin:{}\"",
                &path_value[1..path_value.len() - 1]
            ),
            path.serialize()
        );
    }

    #[test]
    fn test_value_not_quoted_as_a_whole() {
        let path_value = "\"/mnt/c/Program Files/foo\":/usr/local/bin:/usr/bin:/sbin:/bin";
        let path = PathVariable::parse(path_value);
        assert_eq!(path_value, path.serialize().as_str());

        assert_eq!(
            vec![
                "\"/mnt/c/Program Files/foo\"",
                "/usr/local/bin",
                "/usr/bin",
                "/sbin",
                "/bin",
            ],
            path.iter().collect::<Vec<&str>>()
        );

        let path_value = "/usr/local/bin:/usr/bin:/sbin:/bin:\"/mnt/c/Program Files/foo\"";
        let path = PathVariable::parse(path_value);
        assert_eq!(path_value, path.serialize().as_str());

        assert_eq!(
            vec![
                "/usr/local/bin",
                "/usr/bin",
                "/sbin",
                "/bin",
                "\"/mnt/c/Program Files/foo\"",
            ],
            path.iter().collect::<Vec<&str>>()
        );

        let path_value = "\"/usr/local/bin\":/usr/bin:/sbin:/bin:\"/mnt/c/Program Files/foo\"";
        let path = PathVariable::parse(path_value);
        assert_eq!(path_value, path.serialize().as_str());

        assert_eq!(
            vec![
                "\"/usr/local/bin\"",
                "/usr/bin",
                "/sbin",
                "/bin",
                "\"/mnt/c/Program Files/foo\"",
            ],
            path.iter().collect::<Vec<&str>>()
        );

        // quoted single value is treated as "a value the first value of which is quoted", so it's not
        // quoted "as a whole"
        let path_value = "\"/bin\"";
        let mut path = PathVariable::parse(path_value);
        assert_eq!(path_value, path.serialize().as_str());

        assert_eq!(vec!["\"/bin\""], path.iter().collect::<Vec<&str>>());

        path.put_path("/new/path1/bin");
        path.put_path("/new/path2/bin");
        assert_eq!(
            "/new/path2/bin:/new/path1/bin:\"/bin\"",
            path.serialize().as_str()
        );

        // Don't support too tricky values
        let path_value =
            "\"/mnt/c/Program Files\"/foo:/usr/bin:/sbin:/bin:/some/path/include/quote\\\"";
        let mut path = PathVariable::parse(path_value);
        path.put_path("/usr/local/bin");
        assert_ne!("/usr/local/bin:\"/mnt/c/Program Files\"/foo:/usr/bin:/sbin:/bin:/some/path/include/quote\\\"", path.serialize().as_str());
    }
}

#[cfg(test)]
mod test_env_file {
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

        assert_eq!(env.get_env("None"), None);
        assert_eq!(env.get_env("PATH"), Some("test:foo:bar"));
        assert_eq!(env.get_env("BAZ"), Some("baz=baz"));
        assert_eq!(
            env.get_env("FOO"),
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

        env.put_env("NEW1".to_owned(), "NEW1".to_owned());
        env.put_env(
            "PATH".to_owned(),
            format!("path:{}", env.get_env("PATH").unwrap()),
        );
        env.put_env("FOO".to_owned(), "foo3".to_owned());

        assert_eq!(env.get_env("None"), None);
        assert_eq!(env.get_env("NEW1"), Some("NEW1"));
        assert_eq!(env.get_env("PATH"), Some("path:test:foo:bar"));
        assert_eq!(env.get_env("FOO"), Some("foo3"));

        env.write().unwrap();
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

    #[test]
    fn test_empty_env_file() {
        let tmp = NamedTempFile::new().unwrap();
        let env = EnvFile::open(tmp.path());
        assert!(env.is_ok());

        let mut env = env.unwrap();
        env.put_env("TEST".to_owned(), "VALUE".to_owned());
        env.write().unwrap();
        let expected = "\
		    TEST=VALUE\n\
		";
        let new_cont = std::fs::read_to_string(tmp.path()).unwrap();
        assert_eq!(new_cont, expected);
    }

    #[test]
    fn test_open_nonexistential_env_file() {
        let tmpdir = TempDir::new().unwrap();
        let env = EnvFile::open(tmpdir.path().join("dont_exist"));
        assert!(env.is_ok());

        let mut env = env.unwrap();
        env.put_env("TEST".to_owned(), "VALUE".to_owned());
        env.write().unwrap();
        let expected = "\
		    TEST=VALUE\n\
		";
        let new_cont = std::fs::read_to_string(tmpdir.path().join("dont_exist")).unwrap();
        assert_eq!(new_cont, expected);
    }
}
