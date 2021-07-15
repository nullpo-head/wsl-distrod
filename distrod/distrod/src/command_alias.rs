use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};

pub struct CommandAlias {
    source_path: PathBuf,
    link_path: PathBuf,
}

static DISTROD_ALIAS_ROOT: &str = "/opt/distrod/alias";

impl CommandAlias {
    pub fn is_alias<P: AsRef<Path>>(path: P) -> bool {
        path.as_ref().starts_with(DISTROD_ALIAS_ROOT)
    }

    pub fn open_from_source<P: AsRef<Path>>(
        source: P,
        creates: bool,
    ) -> Result<Option<CommandAlias>> {
        let link_path = Path::new(DISTROD_ALIAS_ROOT).join(
            source.as_ref().strip_prefix("/").with_context(|| {
                format!(
                    "The given path is not an absolute path: {:?}",
                    source.as_ref()
                )
            })?,
        );
        if !source.as_ref().exists() {
            if creates {
                std::fs::hard_link(source.as_ref(), &link_path).with_context(|| {
                    format!("Failed to create a new hard link at {:?}", &link_path)
                })?;
            } else {
                return Ok(None);
            }
        }
        Ok(Some(CommandAlias {
            source_path: source.as_ref().to_owned(),
            link_path,
        }))
    }

    pub fn open_from_link<P: AsRef<Path>>(link_path: P) -> Result<CommandAlias> {
        let source_path = link_path
            .as_ref()
            .strip_prefix(DISTROD_ALIAS_ROOT)
            .with_context(|| {
                format!(
                    "The given link does not exist in the alias directory.: '{:?}'",
                    link_path.as_ref()
                )
            })?
            .to_owned();
        // Please note that we do not check if the source path exists, because
        // user may have deleted it.
        Ok(CommandAlias {
            source_path,
            link_path: link_path.as_ref().to_owned(),
        })
    }

    pub fn get_source_path(&self) -> &Path {
        &self.source_path
    }

    pub fn get_link_path(&self) -> &Path {
        &self.link_path
    }
}
