use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

use crate::distrod_config;

pub struct CommandAlias {
    source_path: PathBuf,
    link_path: PathBuf,
}

impl CommandAlias {
    pub fn is_alias<P: AsRef<Path>>(path: P) -> bool {
        path.as_ref().starts_with(distrod_config::get_alias_dir())
    }

    pub fn open_from_source<P: AsRef<Path>>(
        source: P,
        creates: bool,
    ) -> Result<Option<CommandAlias>> {
        let link_path = Path::new(distrod_config::get_alias_dir()).join(
            source.as_ref().strip_prefix("/").with_context(|| {
                format!(
                    "The given path is not an absolute path: {:?}",
                    source.as_ref()
                )
            })?,
        );
        if !link_path.exists() {
            if creates {
                let link_path_dir = link_path
                    .parent()
                    .ok_or_else(|| anyhow!("Failed to get the parent of '{:?}'", &link_path))?;
                if !link_path_dir.exists() {
                    std::fs::create_dir_all(link_path_dir)?;
                }
                let distrod_path = std::env::current_exe()
                    .with_context(|| anyhow!("Failed to get the current_exe."))?;
                std::fs::hard_link(&distrod_path, &link_path).with_context(|| {
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
            .strip_prefix(distrod_config::get_alias_dir())
            .with_context(|| {
                format!(
                    "The given link does not exist in the alias directory.: '{:?}'",
                    link_path.as_ref()
                )
            })?
            .to_owned();
        let source_path = Path::new("/").join(source_path);
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
