use anyhow::{Context, Result};
use std::collections::HashSet;
use std::ffi::OsString;
use std::io::{Cursor, Read};
use std::iter::FromIterator;
use std::path::Path;

pub fn append_tar_archive<W, R, I, P>(
    builder: &mut tar::Builder<W>,
    archive: &mut tar::Archive<R>,
    exclusion: I,
) -> Result<()>
where
    W: std::io::Write,
    R: std::io::Read,
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let exclusion_set = HashSet::<OsString>::from_iter(
        exclusion
            .into_iter()
            .flat_map(|path| gen_name_candidates_for_path(path.as_ref())),
    );

    let entries = archive
        .entries()
        .with_context(|| "Failed to open the archive")?;

    for entry in entries {
        let mut entry = entry.with_context(|| "An archive entry is an error.")?;

        let path = entry
            .path()
            .with_context(|| "Failed to get a path of a tar entry.")?
            .as_os_str()
            .to_os_string();
        if exclusion_set.contains(path.as_os_str()) {
            log::debug!("skipping tar entry: {:?}", &path);
            continue;
        }

        let mut gnu_header =
            to_gnu_header(entry.header()).unwrap_or_else(|| entry.header().clone());

        if let Some(link_name) = entry
            .link_name()
            .with_context(|| format!("Failed to get the link_name {:?}", &path))?
        {
            builder
                .append_link(&mut gnu_header, &path, link_name.as_os_str())
                .with_context(|| format!("Failed to append_link {:?}", &path))?;
        } else {
            let mut data = vec![];
            entry
                .read_to_end(&mut data)
                .with_context(|| format!("Failed to read the data of an entry: {:?}.", &path))?;
            builder
                .append_data(&mut gnu_header, &path, Cursor::new(data))
                .with_context(|| format!("Failed to add an entry to an archive. {:?}", &path))?;
        }
    }
    Ok(())
}

fn to_gnu_header(header: &tar::Header) -> Option<tar::Header> {
    if header.as_gnu().is_some() {
        return None;
    }
    if let Some(ustar) = header.as_ustar() {
        return Some(convert_ustar_to_gnu_header(ustar));
    }
    Some(convert_old_to_gnu_header(header.as_old()))
}

fn convert_ustar_to_gnu_header(header: &tar::UstarHeader) -> tar::Header {
    let mut gnu_header = convert_old_to_gnu_header(header.as_header().as_old());
    let as_gnu = gnu_header
        .as_gnu_mut()
        .expect("new_gnu should return a GNU header");

    as_gnu.typeflag = header.typeflag;
    as_gnu.uname = header.uname;
    as_gnu.gname = header.gname;
    as_gnu.dev_major = header.dev_major;
    as_gnu.dev_minor = header.dev_minor;
    gnu_header
}

fn convert_old_to_gnu_header(header: &tar::OldHeader) -> tar::Header {
    let mut gnu_header = tar::Header::new_gnu();
    let as_gnu = gnu_header
        .as_gnu_mut()
        .expect("new_gnu should return a GNU header");
    as_gnu.name = header.name;
    as_gnu.mode = header.mode;
    as_gnu.uid = header.uid;
    as_gnu.gid = header.gid;
    as_gnu.size = header.size;
    as_gnu.mtime = header.mtime;
    as_gnu.cksum = header.cksum;
    as_gnu.linkname = header.linkname;
    gnu_header
}

fn gen_name_candidates_for_path<P: AsRef<Path>>(path: P) -> Vec<OsString> {
    // There are several ways to represent an absolute path.
    let mut candidates = vec![path.as_ref().as_os_str().to_os_string()];
    let path = path.as_ref();
    if let Ok(alt_path_rel) = path.strip_prefix("/") {
        candidates.push(alt_path_rel.as_os_str().to_owned());
        let mut alt_path_dot = OsString::from(".");
        alt_path_dot.push(path);
        candidates.push(alt_path_dot.to_os_string());
    }
    candidates
}
