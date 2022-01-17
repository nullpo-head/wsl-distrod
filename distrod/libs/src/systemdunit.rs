use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
pub use systemd_parser::items::SystemdUnit;

pub struct SystemdUnitDisabler {
    pub name: String,
    rootfs_path: PathBuf,
}

impl SystemdUnitDisabler {
    pub fn new<P: AsRef<Path>>(rootfs_path: P, service_name: &str) -> SystemdUnitDisabler {
        SystemdUnitDisabler {
            name: service_name.to_owned(),
            rootfs_path: rootfs_path.as_ref().to_owned(),
        }
    }

    pub fn disable(&self) -> Result<()> {
        if self.is_masked()? {
            bail!("{} is already masked.", self.name);
        }
        let company_units = self.get_company_units()?;
        self.remove_unit_symlinks()?;
        for company_unit in company_units {
            company_unit.disable().with_context(|| {
                format!(
                    "Failed to disable a company unit of {}, '{}'.",
                    &self.name, &company_unit.name
                )
            })?;
        }

        Ok(())
    }

    pub fn mask(&self) -> Result<()> {
        self.make_masked_unit_symlink()
    }

    pub fn is_masked(&self) -> Result<bool> {
        let local_unit_path = self.get_local_unit_path();

        if !local_unit_path.exists() {
            return Ok(false);
        }

        let file_type = fs::symlink_metadata(&local_unit_path)
            .with_context(|| {
                format!(
                    "Failed to get the symlink_metadata of {:?}",
                    &local_unit_path
                )
            })?
            .file_type();
        if !file_type.is_symlink() {
            return Ok(false);
        }

        Ok(fs::read_link(&local_unit_path)
            .with_context(|| format!("Failed to read link: {:?}", &local_unit_path))?
            == Path::new("/dev/null"))
    }

    fn make_masked_unit_symlink(&self) -> Result<()> {
        let local_unit_path = &self.get_local_unit_path();
        if local_unit_path.exists() {
            fs::remove_file(&local_unit_path)
                .with_context(|| format!("Failed to remove {:?}", &local_unit_path))?;
        }
        std::os::unix::fs::symlink("/dev/null", &self.get_local_unit_path())
            .with_context(|| format!("Failed to symlink '{:?}'.", &self.get_local_unit_path()))?;
        Ok(())
    }

    fn remove_unit_symlinks(&self) -> Result<()> {
        for link in self
            .collect_unit_symlinks()
            .with_context(|| "Failed to collect unit symlinks to remove.")?
        {
            fs::remove_file(&link).with_context(|| format!("Failed to remove '{:?}'.", &link))?;
        }
        Ok(())
    }

    fn collect_unit_symlinks(&self) -> Result<Vec<PathBuf>> {
        let local_unit_path = self.get_local_unit_path();
        glob::glob(&format!(
            "{}/**/{}",
            local_unit_path
                .parent()
                .ok_or_else(|| anyhow!("The unit '{:?}' doesn't have parent.", &local_unit_path))?
                .to_string_lossy(),
            local_unit_path
                .file_name()
                .ok_or_else(|| anyhow!(
                    "The unit '{:?}' doesn't have file name.",
                    &local_unit_path
                ))?
                .to_string_lossy()
        ))
        .with_context(|| "Glob pattern error.")?
        .map(|link| link.with_context(|| "An iterated link is an error"))
        .collect()
    }

    fn get_company_units(&self) -> Result<Vec<SystemdUnitDisabler>> {
        let service_file = self
            .collect_unit_symlinks()
            .with_context(|| "Failed to collect symlinks to get company units from")?;
        if service_file.is_empty() {
            return Ok(vec![]);
        }
        let unit_path = service_file
            .first()
            .expect("service_file should not be empty.");
        let unit = read_symlink_content(&self.rootfs_path, unit_path)
            .with_context(|| format!("Failed to read a unit path {:?}.", unit_path))?;
        let parsed_systemd_unit = systemd_parser::parse_string(&unit)
            .with_context(|| format!("Failed to parse unit file '{:?}'.", unit_path))?;

        let install = parsed_systemd_unit.lookup_by_category("Install");
        let company_units = install
            .into_iter()
            .filter_map(|e| {
                let company_unit_directives = ["Alias", "Also"];
                match e {
                    systemd_parser::items::DirectiveEntry::Many(directives) => {
                        let key = directives
                            .first()
                            .expect("Many has at least one value.")
                            .key();
                        if company_unit_directives.contains(&key) {
                            let val = directives
                                .iter()
                                .filter_map(|d| d.value().map(|s| s.split(' ')))
                                .flatten()
                                .collect::<Vec<_>>();
                            Some(val)
                        } else {
                            None
                        }
                    }
                    systemd_parser::items::DirectiveEntry::Solo(directive) => {
                        if company_unit_directives.contains(&directive.key()) {
                            directive.value().map(|v| v.split(' ').collect())
                        } else {
                            None
                        }
                    }
                }
            })
            .flatten();

        let mut result = vec![];
        for company_unit in company_units {
            let unit = SystemdUnitDisabler::new(&self.rootfs_path, company_unit);
            result.push(unit);
        }

        Ok(result)
    }

    fn get_local_unit_path(&self) -> PathBuf {
        get_local_unit_path(&self.rootfs_path, &self.name)
    }
}

fn read_symlink_content(rootfs: &Path, symlink_path: &Path) -> Result<String> {
    let symlink_target = fs::read_link(symlink_path)
        .with_context(|| format!("Failed to read symlink {:?}.", symlink_path))?;
    let symlink_target = if symlink_target.is_absolute() {
        rootfs.join(symlink_target.strip_prefix("/")?)
    } else {
        symlink_target
    };
    fs::read_to_string(&symlink_target).with_context(|| {
        format!(
            "Failed to read the contents of the symlink target {:?}.",
            &symlink_target
        )
    })
}

#[derive(Debug, Clone, Default)]
pub struct SystemdUnitOverride {
    sections: HashMap<String, SystemdUnitSection>,
}

#[derive(Debug, Clone, Default)]
struct SystemdUnitSection {
    directives: HashMap<String, Vec<String>>,
}

impl SystemdUnitOverride {
    pub fn put_section(&mut self, section_name: String) -> &mut Self {
        self.sections
            .entry(section_name)
            .or_insert_with(SystemdUnitSection::default);
        self
    }

    pub fn push_directive(
        &mut self,
        section_name: &str,
        directive_name: &str,
        value: String,
    ) -> &mut Self {
        self.get_mut_section(section_name)
            .push_directive(directive_name, value);
        self
    }

    pub fn unset_directive(&mut self, section_name: &str, directive_name: &str) -> &mut Self {
        self.get_mut_section(section_name)
            .unset_directive(directive_name);
        self
    }

    fn get_mut_section(&mut self, section_name: &str) -> &mut SystemdUnitSection {
        if !self.sections.contains_key(section_name) {
            self.put_section(section_name.to_owned());
        }
        self.sections
            .get_mut(section_name)
            .expect("[BUG] put_section should be callsed beforehand.")
    }

    pub fn write<P: AsRef<Path>>(&mut self, rootfs_path: P, service_name: &str) -> Result<()> {
        let serialized = self.serialize();
        let override_path = get_override_conf_path(rootfs_path, service_name);
        let override_conf_dir = override_path
            .parent()
            .expect("[BUG] get_override_conf_path should return a dir.");
        fs::create_dir_all(override_conf_dir)
            .with_context(|| format!("Failed to create dir all {:?}", &override_conf_dir))?;
        fs::write(&override_path, serialized)
            .with_context(|| format!("Failed to write to {:?}", &override_path))?;
        Ok(())
    }

    fn serialize(&self) -> String {
        let mut result = String::new();
        let mut sections = self.sections.iter().collect::<Vec<_>>();
        sections.sort_by(|(a, _), (b, _)| a.cmp(b));
        for (section_name, section) in sections {
            result.push_str(&format!("[{}]\n", section_name));
            result.push_str(&section.serialize());
        }
        result
    }
}

impl SystemdUnitSection {
    pub fn push_directive(&mut self, directive_name: &str, value: String) -> &mut Self {
        // To override a value, you need to unset it first.
        if !self.has_directive(directive_name) {
            self.unset_directive(directive_name);
        }
        self.insert_directive(directive_name, value)
    }

    pub fn unset_directive(&mut self, directive_name: &str) -> &mut Self {
        if let Some(directives) = self.directives.get_mut(directive_name) {
            directives.clear();
        }
        self.insert_directive(directive_name, "".to_owned())
    }

    fn has_directive(&self, directive_name: &str) -> bool {
        self.directives.contains_key(directive_name)
    }

    fn insert_directive(&mut self, directive_name: &str, value: String) -> &mut Self {
        if !self.directives.contains_key(directive_name) {
            self.directives.insert(directive_name.to_owned(), vec![]);
        }
        let directives = self
            .directives
            .get_mut(directive_name)
            .expect("[BUG] directives should have at least vec![].");
        directives.push(value);
        self
    }

    fn serialize(&self) -> String {
        let mut result = String::new();
        for (directive_name, values) in &self.directives {
            result.push_str(
                &values
                    .iter()
                    .map(|value| format!("{}={}", directive_name, value))
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
        if !result.is_empty() {
            result.push('\n');
        }
        result
    }
}

pub fn get_existing_systemd_unit<P: AsRef<Path>>(
    rootfs_path: P,
    service_name: &str,
) -> Result<Option<SystemdUnit>> {
    Ok(match get_existing_unit_path(rootfs_path, service_name) {
        Some(path) => Some(
            systemd_parser::parse_string(
                &fs::read_to_string(&path)
                    .with_context(|| format!("Failed to read {:?}.", &path))?,
            )
            .with_context(|| format!("Failed to parse Systemd Unit file {:?}", &path))?,
        ),
        None => None,
    })
}

fn get_override_conf_path<P: AsRef<Path>>(rootfs_path: P, service_name: &str) -> PathBuf {
    get_local_unit_path(rootfs_path, &format!("{}.d/override.conf", service_name))
}

fn get_local_unit_path<P: AsRef<Path>>(rootfs_path: P, service_name: &str) -> PathBuf {
    rootfs_path
        .as_ref()
        .join("etc/systemd/system/")
        .join(service_name)
}

fn get_existing_unit_path<P: AsRef<Path>>(rootfs_path: P, service_name: &str) -> Option<PathBuf> {
    let candidates = [
        "etc/systemd/system/",
        "usr/lib/systemd/system/",
        "lib/systemd/system/",
        "run/systemd/system/",
    ];
    for candidate in candidates.iter() {
        let path = rootfs_path.as_ref().join(candidate).join(service_name);
        if path.exists() {
            return Some(path);
        }
    }
    None
}

#[cfg(test)]
mod test_systemd_unit_override {
    use super::*;

    #[test]
    fn test_simple_override() {
        let mut overrider = SystemdUnitOverride::default();
        overrider.put_section("Service".to_owned());
        assert_eq!("[Service]\n", overrider.serialize());

        // put again
        overrider.put_section("Service".to_owned());
        assert_eq!("[Service]\n", overrider.serialize());

        overrider.push_directive("Service", "Environment", "test1".to_owned());
        assert_eq!(
            "[Service]\nEnvironment=\nEnvironment=test1\n",
            overrider.serialize()
        );
        overrider.push_directive("Service", "Environment", "test2".to_owned());
        assert_eq!(
            "[Service]\nEnvironment=\nEnvironment=test1\nEnvironment=test2\n",
            overrider.serialize()
        );

        overrider.unset_directive("Service2", "Test");
        assert_eq!(
            "[Service]\nEnvironment=\nEnvironment=test1\nEnvironment=test2\n[Service2]\nTest=\n",
            overrider.serialize()
        );
        overrider.unset_directive("Service", "Environment");
        assert_eq!(
            "[Service]\nEnvironment=\n[Service2]\nTest=\n",
            overrider.serialize()
        );
    }
}

#[cfg(test)]
mod test_systemd_unit_disabler {
    use super::*;
    use flate2::bufread::GzDecoder;
    use tempfile::*;

    static SYSTEMD_DIR: &str = "etc/systemd/system";
    static MULTI_USER_UNIT_NAME: &str = "multi-user.target.wants";

    #[test]
    fn test_simple_unit() {
        let simple_unit = "simple_unit.service";
        let (tempdir, unitdir_path) = setup_unit_dir().unwrap();

        assert!(unitdir_path.join(simple_unit).exists());
        assert!(unitdir_path
            .join(MULTI_USER_UNIT_NAME)
            .join(simple_unit)
            .exists());

        let disabler = SystemdUnitDisabler::new(&tempdir, simple_unit);
        disabler.disable().unwrap();

        assert!(!unitdir_path.join(simple_unit).exists());
        assert!(!unitdir_path
            .join(MULTI_USER_UNIT_NAME)
            .join(simple_unit)
            .exists());
    }

    #[test]
    fn test_simple_alias_unit() {
        let unit = "simple_alias.service";
        let aliases = vec!["aliased.service"];

        let (tempdir, unitdir_path) = setup_unit_dir().unwrap();

        for alias in &aliases {
            assert!(unitdir_path.join(MULTI_USER_UNIT_NAME).join(alias).exists());
        }

        let disabler = SystemdUnitDisabler::new(&tempdir, unit);
        disabler.disable().unwrap();

        for alias in &aliases {
            assert!(!unitdir_path.join(MULTI_USER_UNIT_NAME).join(alias).exists());
        }
    }

    #[test]
    fn test_multiple_alias_unit() {
        let unit = "multiple_alias.service";
        let aliases = vec![
            "multiple_alias1.service",
            "multiple_alias2.service",
            "multiple_alias3.service",
        ];
        let not_to_be_touched = vec!["unrelated.service"];

        let (tempdir, unitdir_path) = setup_unit_dir().unwrap();

        for alias in &aliases {
            assert!(unitdir_path.join(MULTI_USER_UNIT_NAME).join(alias).exists());
        }
        for should_exist in &not_to_be_touched {
            assert!(unitdir_path.join(should_exist).exists());
        }

        let disabler = SystemdUnitDisabler::new(&tempdir, unit);
        disabler.disable().unwrap();

        for alias in &aliases {
            assert!(!unitdir_path.join(MULTI_USER_UNIT_NAME).join(alias).exists());
            for should_exist in &not_to_be_touched {
                assert!(unitdir_path.join(should_exist).exists());
            }
        }
    }

    #[test]
    fn test_simple_also_unit() {
        let also_references = vec!["simple_also_unit.service", "referenced_by_also1.service"];

        let (tempdir, unitdir_path) = setup_unit_dir().unwrap();

        for also in &also_references {
            assert!(unitdir_path.join(also).exists());
            assert!(unitdir_path.join(MULTI_USER_UNIT_NAME).join(also).exists());
        }

        let disabler = SystemdUnitDisabler::new(&tempdir, also_references[0]);
        disabler.disable().unwrap();

        for also in &also_references {
            assert!(!unitdir_path.join(also).exists());
            assert!(!unitdir_path.join(MULTI_USER_UNIT_NAME).join(also).exists());
        }
    }

    #[test]
    fn test_multiple_also_unit() {
        let also_references = vec![
            "multiple_also_unit.service",
            "referenced_by_also2.service",
            "referenced_by_also3.service",
            "referenced_by_also4.service",
        ];
        let not_to_be_touched = vec!["unrelated.service"];

        let (tempdir, unitdir_path) = setup_unit_dir().unwrap();

        for also in &also_references {
            assert!(unitdir_path.join(also).exists());
            assert!(unitdir_path.join(MULTI_USER_UNIT_NAME).join(also).exists());
        }
        for should_exist in &not_to_be_touched {
            assert!(unitdir_path.join(should_exist).exists());
        }

        let disabler = SystemdUnitDisabler::new(&tempdir, also_references[0]);
        disabler.disable().unwrap();

        for also in &also_references {
            assert!(!unitdir_path.join(also).exists());
            assert!(!unitdir_path.join(MULTI_USER_UNIT_NAME).join(also).exists());
        }
        for should_exist in &not_to_be_touched {
            assert!(unitdir_path.join(should_exist).exists());
        }
    }

    #[test]
    fn test_mask() {
        let existing_unit = "systemd-system1.service";
        let nonexisting_unit = "systemd-system0.service";

        let (tempdir, unitdir_path) = setup_unit_dir().unwrap();
        assert!(unitdir_path.join(existing_unit).exists());
        assert!(!unitdir_path.join(nonexisting_unit).exists());

        let existing_unit_disabler = SystemdUnitDisabler::new(&tempdir, existing_unit);
        let nonexisting_unit_disabler = SystemdUnitDisabler::new(&tempdir, nonexisting_unit);
        existing_unit_disabler.mask().unwrap();
        nonexisting_unit_disabler.mask().unwrap();

        assert!(unitdir_path.join(existing_unit).exists());
        assert!(unitdir_path.join(nonexisting_unit).exists());
        assert!(
            fs::read_link(unitdir_path.join(existing_unit)).unwrap() == PathBuf::from("/dev/null")
        );
        assert!(
            fs::read_link(unitdir_path.join(nonexisting_unit)).unwrap()
                == PathBuf::from("/dev/null")
        );
    }

    fn setup_unit_dir() -> Result<(TempDir, PathBuf)> {
        let temp_dir = tempdir()?;
        let unit_dir = temp_dir.path().join(SYSTEMD_DIR);
        fs::create_dir_all(&unit_dir).unwrap();

        let tar = include_bytes!("../tests/resources/systemdunit/unit_dir.tar.gz");
        let mut tar = tar::Archive::new(GzDecoder::new(std::io::Cursor::new(tar)));
        tar.unpack(&unit_dir.join("..")).unwrap();

        Ok((temp_dir, unit_dir))
    }
}
