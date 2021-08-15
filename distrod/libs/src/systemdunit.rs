use anyhow::{anyhow, bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

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

    pub fn disable(&mut self) -> Result<()> {
        if self.is_masked()? {
            bail!("{} is masked.", self.name);
        }
        let company_units = self.get_company_units()?;
        self.remove_unit_symlinks()?;
        for mut company_unit in company_units {
            company_unit.disable().with_context(|| {
                format!(
                    "Failed to disable a company unit of {}, '{}'.",
                    &self.name, &company_unit.name
                )
            })?;
        }

        Ok(())
    }

    pub fn mask(&mut self) -> Result<()> {
        self.make_masked_unit_symlink()
    }

    pub fn is_masked(&self) -> Result<bool> {
        let standard_path = self.get_standard_unit_path();

        if !standard_path.exists() {
            return Ok(false);
        }

        let file_type = fs::symlink_metadata(&standard_path)
            .with_context(|| format!("Failed to get the symlink_metadata of {:?}", &standard_path))?
            .file_type();
        if !file_type.is_symlink() {
            return Ok(false);
        }

        Ok(fs::read_link(&standard_path)
            .with_context(|| format!("Failed to read link: {:?}", &standard_path))?
            == Path::new("/dev/null"))
    }

    fn make_masked_unit_symlink(&mut self) -> Result<()> {
        let standard_path = &self.get_standard_unit_path();
        if standard_path.exists() {
            fs::remove_file(&standard_path)
                .with_context(|| format!("Failed to remove {:?}", &standard_path))?;
        }
        std::os::unix::fs::symlink("/dev/null", &self.get_standard_unit_path()).with_context(
            || format!("Failed to symlink '{:?}'.", &self.get_standard_unit_path()),
        )?;
        Ok(())
    }

    fn remove_unit_symlinks(&mut self) -> Result<()> {
        let links = self.collect_unit_symlinks()?;
        for link in links {
            let link = link?;
            fs::remove_file(&link).with_context(|| format!("Failed to remove '{:?}'.", &link))?;
        }
        Ok(())
    }

    fn collect_unit_symlinks(&self) -> Result<glob::Paths> {
        let standard_unit_path = self.get_standard_unit_path();
        glob::glob(&format!(
            "{}/**/{}",
            standard_unit_path
                .parent()
                .ok_or_else(|| anyhow!(
                    "The unit '{:?}' doesn't have parent.",
                    &standard_unit_path
                ))?
                .to_string_lossy(),
            standard_unit_path
                .file_name()
                .ok_or_else(|| anyhow!(
                    "The unit '{:?}' doesn't have file name.",
                    &standard_unit_path
                ))?
                .to_string_lossy()
        ))
        .with_context(|| "Glob pattern error.")
    }

    fn get_company_units(&mut self) -> Result<Vec<SystemdUnitDisabler>> {
        let service_file = self.collect_unit_symlinks()?.next();
        if service_file.is_none() {
            return Ok(vec![]);
        }
        let unit_path = service_file.unwrap()?;
        let unit = fs::read_to_string(&unit_path)
            .with_context(|| format!("Failed to read {:?}.", &unit_path))?;
        let parsed_systemd_unit = systemd_parser::parse_string(&unit)
            .with_context(|| format!("Failed to parse unit file '{:?}'.", &unit_path))?;

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

    fn get_standard_unit_path(&self) -> PathBuf {
        self.rootfs_path
            .join("etc/systemd/system/")
            .join(&self.name)
    }
}

#[cfg(test)]
mod tests {
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

        let mut disabler = SystemdUnitDisabler::new(&tempdir, simple_unit);
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

        let mut disabler = SystemdUnitDisabler::new(&tempdir, unit);
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

        let mut disabler = SystemdUnitDisabler::new(&tempdir, unit);
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

        let mut disabler = SystemdUnitDisabler::new(&tempdir, also_references[0]);
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

        let mut disabler = SystemdUnitDisabler::new(&tempdir, also_references[0]);
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

        let mut existing_unit_disabler = SystemdUnitDisabler::new(&tempdir, existing_unit);
        let mut nonexisting_unit_disabler = SystemdUnitDisabler::new(&tempdir, nonexisting_unit);
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
        std::fs::create_dir_all(&unit_dir).unwrap();

        let tar = include_bytes!("../tests/resources/systemdunit/unit_dir.tar.gz");
        let mut tar = tar::Archive::new(GzDecoder::new(std::io::Cursor::new(tar)));
        tar.unpack(&unit_dir.join("..")).unwrap();

        Ok((temp_dir, unit_dir))
    }
}
