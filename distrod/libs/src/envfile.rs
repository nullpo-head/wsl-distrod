use nom::{
    branch::alt,
    bytes::complete::{is_not, tag, take, take_while, take_while1},
    character::{
        complete::{char, line_ending, none_of, space0, space1},
        is_alphanumeric, is_newline,
    },
    combinator::{map_res, opt, recognize},
    error::VerboseError,
    multi::{many1, separated_list0},
    sequence::{pair, separated_pair, terminated, tuple},
};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{BufReader, BufWriter, Read, Write},
    ops::{Deref, DerefMut},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, bail, Context, Result};

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

/// EnvFile understands /etc/environment at about the same level as pam_env.so,
/// so that it can modify the value of existing environment variables or add new ones.
/// (See https://github.com/linux-pam/linux-pam/blob/master/modules/pam_env/pam_env.c)
#[derive(Debug, Clone)]
pub struct EnvFile {
    pub file_path: PathBuf,
    envs: HashMap<String, usize>,
    env_file_lines: EnvFileLines,
}

#[derive(Debug, Clone, Default)]
struct EnvFileLines(Vec<EnvFileLine>);

#[derive(Debug, Clone)]
enum EnvFileLine {
    Env(EnvStatement),
    Other(String),
}

#[derive(Debug, Clone)]
struct EnvStatement {
    key: String,
    value: String,
    leading_characters: String,
    following_characters: String,
}

impl EnvFile {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<EnvFile> {
        let (envs, env_file_lines) = match File::open(path.as_ref()) {
            Ok(file) => {
                let mut reader = BufReader::new(file);
                let mut buf = vec![];
                reader
                    .read_to_end(&mut buf)
                    .with_context(|| format!("Failed to read {:?}", path.as_ref()))?;
                let (_, env_file_lines) = EnvFileLines::parse(&buf)
                    .map_err(|e| anyhow!("Failed to parse a line: {:?}", e))?;
                let mut envs = HashMap::<String, usize>::default();
                env_file_lines.iter().enumerate().for_each(|(i, line)| {
                    if let EnvFileLine::Env(env) = line {
                        envs.insert(env.key.clone(), i);
                    };
                });
                (envs, env_file_lines)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                (HashMap::<String, usize>::default(), EnvFileLines::default())
            }
            Err(e) => bail!(e),
        };
        Ok(EnvFile {
            file_path: path.as_ref().to_owned(),
            envs,
            env_file_lines,
        })
    }

    pub fn get_env(&self, key: &str) -> Option<&str> {
        let val = match self.env_file_lines[*self.envs.get(key)?] {
            EnvFileLine::Env(ref env_statement) => env_statement.value.as_str(),
            _ => unreachable!(),
        };
        Some(val)
    }

    pub fn put_env(&mut self, key: String, value: String) {
        // we don't allow to put values for safety, otherwise it will confuse pam_env.so and
        // let other variables be overwritten.
        assert!(!value.contains('\n') && !value.contains('\\'));

        let line_index = self.envs.get(&key);
        let value = single_quote_str_for_shell(&value);
        match line_index {
            Some(index) => {
                let line = &mut self.env_file_lines[*index];
                match *line {
                    EnvFileLine::Env(ref mut env_statement) => {
                        env_statement.value = value;
                    }
                    _ => unreachable!(),
                }
            }
            None => {
                let line = EnvFileLine::Env(EnvStatement {
                    key: key.clone(),
                    value,
                    leading_characters: String::new(),
                    following_characters: String::new(),
                });
                self.env_file_lines.push(line);
                self.envs.insert(key, self.env_file_lines.len() - 1);
            }
        }
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

    pub fn write(&mut self) -> Result<()> {
        let mut file = BufWriter::new(
            File::create(&self.file_path)
                .with_context(|| format!("Failed to create {:?}.", &self.file_path))?,
        );
        file.write_all(self.env_file_lines.serialize().as_bytes())?;
        Ok(())
    }
}

type IResult<I, O> = nom::IResult<I, O, VerboseError<I>>;

impl EnvFileLines {
    pub fn parse(input: &[u8]) -> IResult<&[u8], EnvFileLines> {
        if input.is_empty() {
            return Ok((&[], EnvFileLines(vec![])));
        }
        map_res::<_, _, _, _, nom::Err<&[u8]>, _, _>(many1(EnvFileLine::parse), |lines| {
            Ok(EnvFileLines(lines))
        })(input)
    }

    pub fn serialize(&self) -> String {
        let lines = self.0.iter().map(|l| l.serialize()).collect::<Vec<_>>();
        lines.join("")
    }
}

impl Deref for EnvFileLines {
    type Target = Vec<EnvFileLine>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for EnvFileLines {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl EnvFileLine {
    pub fn parse(line: &[u8]) -> IResult<&[u8], EnvFileLine> {
        let other_line = map_res::<_, _, _, _, nom::Err<&[u8]>, _, _>(
            alt((
                // line with a comment or other strings with or without a line ending
                terminated(recognize(many1(is_not("\n"))), opt(line_ending)),
                // empty line
                map_res::<_, _, _, _, nom::Err<&[u8]>, _, _>(line_ending, |_| {
                    Ok(<&[u8]>::default())
                }),
            )),
            |s| {
                Ok(EnvFileLine::Other(
                    String::from_utf8_lossy(s).to_string() + "\n",
                ))
            },
        );
        let env = map_res::<_, _, _, _, nom::Err<&[u8]>, _, _>(EnvStatement::parse, |s| {
            Ok(EnvFileLine::Env(s))
        });
        alt((env, other_line))(line)
    }

    pub fn serialize(&self) -> String {
        match *self {
            EnvFileLine::Env(ref env) => env.serialize(),
            EnvFileLine::Other(ref other) => other.clone(),
        }
    }
}

impl EnvStatement {
    pub fn parse(line: &[u8]) -> IResult<&[u8], EnvStatement> {
        let (rest, (leading_characters, (key, value), following_characters, _)) = tuple((
            leading_characters,
            separated_pair(declaration_key, tag("="), declaration_value),
            following_characters,
            opt(line_ending),
        ))(line)?;
        let to_string = |s: &[u8]| -> String { String::from_utf8_lossy(s).to_string() };
        Ok((
            rest,
            EnvStatement {
                key: to_string(key),
                value: to_string(value),
                leading_characters: to_string(leading_characters),
                following_characters: to_string(following_characters),
            },
        ))
    }

    pub fn serialize(&self) -> String {
        let mut serialized_line = self.leading_characters.clone();
        serialized_line.push_str(&self.key);
        serialized_line.push('=');
        serialized_line.push_str(&self.value);
        serialized_line.push_str(&self.following_characters);
        serialized_line.push('\n');
        serialized_line
    }
}

fn leading_characters(line: &[u8]) -> IResult<&[u8], &[u8]> {
    recognize(tuple((space0, opt(tag(b"export")), space0)))(line)
}

fn declaration_key(line: &[u8]) -> IResult<&[u8], &[u8]> {
    take_while1(is_alphanumeric)(line)
}

fn declaration_value(line: &[u8]) -> IResult<&[u8], &[u8]> {
    //let regular_char = take_while(|c| !is_space(c) && !is_newline(c) && c != b'#');
    let escaped_char = recognize(pair(char('\\'), take(1u32)));
    let regular_char = recognize(none_of("\n# \t\\"));
    recognize(separated_list0(
        space1,
        many1(alt((regular_char, escaped_char))),
    ))(line)
}

fn following_characters(line: &[u8]) -> IResult<&[u8], &[u8]> {
    take_while(|c| !is_newline(c))(line)
}

#[derive(Debug, Clone)]
pub struct PathVariable<'a> {
    parsed_paths: Vec<&'a str>,
    added_paths: Vec<&'a str>,
    path_set: HashSet<&'a str>,
    surrounding_quote: Option<char>,
}

impl<'a> PathVariable<'a> {
    pub fn parse(val: &'a str) -> Self {
        let mut paths: Vec<_> = val.split(':').into_iter().collect();

        // Roughly regard the whole path is surrounded by double quotes by simple logic
        let quote_candidates = vec!['"', '\''];
        let surrounding_quote = quote_candidates.into_iter().find(|quote| {
            paths.first().map_or(false, |path| {
                path.starts_with(*quote) && !path.ends_with(*quote)
            }) && paths.last().map_or(false, |path| {
                !path.starts_with(*quote) && path.ends_with(*quote)
            })
        });

        if surrounding_quote.is_some() {
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
            surrounding_quote,
        }
    }

    pub fn serialize(&self) -> String {
        let mut paths = self.iter().collect::<Vec<_>>().join(":");
        if let Some(quote) = self.surrounding_quote {
            paths.insert(0, quote);
            paths.push(quote);
        }
        paths
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

fn single_quote_str_for_shell(s: &str) -> String {
    format!("'{}'", s.replace("'", "'\"'\"'"))
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
        assert_eq!(path_value, path.serialize());
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

        // single quote
        let path_value = "'/usr/local/bin:/usr/bin:/sbin:/bin'";
        let mut path = PathVariable::parse(path_value);
        path.put_path("/new/path1/bin");
        assert_eq!(
            "'/new/path1/bin:/usr/local/bin:/usr/bin:/sbin:/bin'",
            path.serialize()
        );
        assert_eq!(
            vec![
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
    fn test_value_not_quoted_as_a_whole() {
        let path_value = "\"/mnt/c/Program Files/foo\":/usr/local/bin:/usr/bin:/sbin:/bin";
        let path = PathVariable::parse(path_value);
        assert_eq!(path_value, path.serialize());

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
        assert_eq!(path_value, path.serialize());

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
        assert_eq!(path_value, path.serialize());

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
        assert_eq!(path_value, path.serialize());

        assert_eq!(vec!["\"/bin\""], path.iter().collect::<Vec<&str>>());

        path.put_path("/new/path1/bin");
        path.put_path("/new/path2/bin");
        assert_eq!("/new/path2/bin:/new/path1/bin:\"/bin\"", path.serialize());

        // Don't support too tricky values
        let path_value =
            "\"/mnt/c/Program Files\"/foo:/usr/bin:/sbin:/bin:/some/path/include/quote\\\"";
        let mut path = PathVariable::parse(path_value);
        path.put_path("/usr/local/bin");
        assert_ne!("/usr/local/bin:\"/mnt/c/Program Files\"/foo:/usr/bin:/sbin:/bin:/some/path/include/quote\\\"", path.serialize());
    }
}

#[cfg(test)]
mod test_env_file_parsers {
    use super::*;

    #[test]
    fn test_parse_env_statement_simple() {
        let (_, statement) = EnvStatement::parse("PATH=hoge:fuga:piyo".as_bytes()).unwrap();
        eprintln!("Statement: {:#?}", &statement);
        assert_eq!("PATH", statement.key);
        assert_eq!("hoge:fuga:piyo", statement.value);
        assert_eq!("", statement.leading_characters);
        assert_eq!("", statement.following_characters);
        assert_eq!("PATH=hoge:fuga:piyo\n", statement.serialize());

        // same value with new line
        let (_, statement) = EnvStatement::parse("PATH=hoge:fuga:piyo\n".as_bytes()).unwrap();
        eprintln!("Statement: {:#?}", &statement);
        assert_eq!("PATH", statement.key);
        assert_eq!("hoge:fuga:piyo", statement.value);
        assert_eq!("", statement.leading_characters);
        assert_eq!("", statement.following_characters);
        assert_eq!("PATH=hoge:fuga:piyo\n", statement.serialize());

        // with comment and exprot
        let (_, statement) =
            EnvStatement::parse(" export  PATH=hoge:fuga:piyo  # comment".as_bytes()).unwrap();
        eprintln!("Statement: {:#?}", &statement);
        assert_eq!("PATH", statement.key);
        assert_eq!("hoge:fuga:piyo", statement.value);
        assert_eq!(" export  ", statement.leading_characters);
        assert_eq!("  # comment", statement.following_characters);
        assert_eq!(
            " export  PATH=hoge:fuga:piyo  # comment\n",
            statement.serialize()
        );
    }

    #[test]
    fn test_parse_env_statement_empty() {
        assert!(EnvStatement::parse("".as_bytes()).is_err());

        let (_, statement) = EnvStatement::parse("PATH=".as_bytes()).unwrap();
        eprintln!("Statement: {:#?}", &statement);
        assert_eq!("PATH", statement.key);
        assert_eq!("", statement.value);
        assert_eq!("", statement.leading_characters);
        assert_eq!("", statement.following_characters);
        assert_eq!("PATH=\n", statement.serialize());

        let (_, statement) = EnvStatement::parse("export PATH=  # no value".as_bytes()).unwrap();
        eprintln!("Statement: {:#?}", &statement);
        assert_eq!("PATH", statement.key);
        assert_eq!("", statement.value);
        assert_eq!("export ", statement.leading_characters);
        assert_eq!("  # no value", statement.following_characters);
        assert_eq!("export PATH=  # no value\n", statement.serialize());
    }

    #[test]
    fn test_parse_env_statement_continued_line() {
        let val = "hoge:fuga:piyo\\\n\
                         :new_line";
        let line = format!("PATH={}  # and comment\n", val);
        let (_, statement) = EnvStatement::parse(line.as_bytes()).unwrap();
        eprintln!("Statement: {:#?}", &statement);
        assert_eq!("PATH", statement.key);
        assert_eq!(val, statement.value);
        assert_eq!("", statement.leading_characters);
        assert_eq!("  # and comment", statement.following_characters);
        assert_eq!(line, statement.serialize());
    }

    #[test]
    fn test_parse_env_statement_strange() {
        let (_, statement) = EnvStatement::parse("VAR=A=B=C".as_bytes()).unwrap();
        eprintln!("Statement: {:#?}", &statement);
        assert_eq!("VAR", statement.key);
        assert_eq!("A=B=C", statement.value);
        assert_eq!("", statement.leading_characters);
        assert_eq!("", statement.following_characters);
        assert_eq!("VAR=A=B=C\n", statement.serialize());

        let (_, statement) = EnvStatement::parse("VAR=A B C # comment".as_bytes()).unwrap();
        eprintln!("Statement: {:#?}", &statement);
        assert_eq!("VAR", statement.key);
        assert_eq!("A B C", statement.value);
        assert_eq!("", statement.leading_characters);
        assert_eq!(" # comment", statement.following_characters);
        assert_eq!("VAR=A B C # comment\n", statement.serialize());

        let (_, statement) = EnvStatement::parse("export VAR=😀 # emoji 😀".as_bytes()).unwrap();
        eprintln!("Statement: {:#?}", &statement);
        assert_eq!("VAR", statement.key);
        assert_eq!("😀", statement.value);
        assert_eq!("export ", statement.leading_characters);
        assert_eq!(" # emoji 😀", statement.following_characters);
        assert_eq!("export VAR=😀 # emoji 😀\n", statement.serialize());
    }

    #[test]
    fn test_parse_env_file_line() {
        let (_, line) = EnvFileLine::parse("# this is comment".as_bytes()).unwrap();
        eprintln!("line: {:#?}", &line);
        assert!(matches!(line, EnvFileLine::Other(_)));
        if let EnvFileLine::Other(str) = &line {
            assert_eq!("# this is comment\n", str);
        }
        assert_eq!("# this is comment\n", line.serialize());

        // empty line
        let (_, line) = EnvFileLine::parse("\n".as_bytes()).unwrap();
        eprintln!("line: {:#?}", &line);
        assert!(matches!(line, EnvFileLine::Other(_)));
        assert_eq!("\n", line.serialize());

        // abnormal line
        let (_, line) = EnvFileLine::parse("==fawe=f= =".as_bytes()).unwrap();
        eprintln!("line: {:#?}", &line);
        assert!(matches!(line, EnvFileLine::Other(_)));
        assert_eq!("==fawe=f= =\n", line.serialize());
    }

    #[test]
    fn test_parse_env_file_lines() {
        let src = "\
        # This is comment\n\
        VAR=VALUE\n\
        \n\
        \n\
        # another comment \n\
        PATH=path1:path2\\\n\
        path3";
        let (_, lines) = EnvFileLines::parse(src.as_bytes()).unwrap();
        eprintln!("lines: {:#?}", &lines);
        assert_eq!(lines.len(), 6);
        assert!(matches!(lines[0], EnvFileLine::Other(_)));
        assert!(matches!(lines[1], EnvFileLine::Env(_)));
        assert!(matches!(lines[2], EnvFileLine::Other(_)));
        assert!(matches!(lines[3], EnvFileLine::Other(_)));
        assert!(matches!(lines[4], EnvFileLine::Other(_)));
        assert!(matches!(lines[5], EnvFileLine::Env(_)));
        assert_eq!(format!("{}\n", src), lines.serialize())
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

        eprintln!("EnvFile: {:#?}", &env);
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
            # This is a comment line
		    PATH=test:foo:bar  #comment preserved \n\
			FOO=foo\n\
            # This is another comment line
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
        assert_eq!(env.get_env("NEW1"), Some("'NEW1'"));
        assert_eq!(env.get_env("PATH"), Some("'path:test:foo:bar'"));
        assert_eq!(env.get_env("FOO"), Some("'foo3'"));

        env.write().unwrap();
        let expected = "\
            # This is a comment line
		    PATH='path:test:foo:bar'  #comment preserved \n\
			FOO=foo\n\
            # This is another comment line
			BAR=bar\n\
			BAZ=baz=baz\n\
			FOO='foo3'\n\
			NEW1='NEW1'\n\
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
		    TEST='VALUE'\n\
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
		    TEST='VALUE'\n\
		";
        let new_cont = std::fs::read_to_string(tmpdir.path().join("dont_exist")).unwrap();
        assert_eq!(new_cont, expected);
    }
}
