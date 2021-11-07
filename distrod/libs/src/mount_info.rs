use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::PathBuf,
};

use anyhow::{Context, Result};

pub struct MountEntry {
    pub source: String,
    pub path: PathBuf,
    pub fstype: String,
    pub attributes: String,
}

pub fn get_mount_entries() -> Result<Vec<MountEntry>> {
    let mounts = File::open("/proc/mounts").with_context(|| "Failed to open '/proc/mounts'")?;
    let reader = BufReader::new(mounts);

    let mut mount_entries = vec![];
    for (_, line) in reader.lines().enumerate() {
        let line = line?;
        let row: Vec<&str> = line.split(' ').take(4).collect();
        let (source, path, fstype, attributes) = (
            row[0].to_owned(),
            row[1].to_owned(),
            row[2].to_owned(),
            row[3].to_owned(),
        );
        mount_entries.push(MountEntry {
            source,
            path: PathBuf::from(path),
            fstype,
            attributes,
        });
    }

    Ok(mount_entries)
}
