//! Only bounded regular payloads are ever extracted, into private staging.
use crate::{
    Result,
    manifest::{self, MAX_BYTES, Manifest},
    publication,
};
use flate2::read::GzDecoder;
use std::{collections::BTreeSet, fs, io::Read, path::Path};

pub fn unpack(source: &Path, staging: &Path, part: &str, target: &str) -> Result<Manifest> {
    let name = format!("{part}-{target}.tar.gz");
    let sums = String::from_utf8(manifest::read_bounded(
        &source.join("SHA256SUMS"),
        1024 * 1024,
    )?)?;
    let matches: Vec<_> = sums
        .lines()
        .filter_map(|line| {
            let mut pieces = line.split_whitespace();
            let hash = pieces.next()?;
            let file = pieces.next()?;
            (file == name && pieces.next().is_none()).then_some(hash)
        })
        .collect();
    let path = source.join(&name);
    if matches.len() != 1
        || manifest::regular(&path)?.len() > MAX_BYTES
        || manifest::hash(&path)? != matches[0]
    {
        return Err(format!("Missing or mismatched archive checksum: {name}").into());
    }
    let root = staging.join(part);
    fs::create_dir(&root)?;
    let input = GzDecoder::new(fs::File::open(path)?).take(MAX_BYTES + 1024 * 1024);
    let mut archive = tar::Archive::new(input);
    let mut names = BTreeSet::new();
    let mut bytes = 0_u64;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let name = entry
            .path()?
            .to_str()
            .ok_or("Non-UTF8 archive path")?
            .to_owned();
        let size = entry.size();
        bytes = bytes.checked_add(size).ok_or("Archive size overflow")?;
        if !manifest::safe_path(&name)
            || !entry.header().entry_type().is_file()
            || !names.insert(name.clone())
            || names.len() > 129
            || bytes > MAX_BYTES
        {
            return Err("Archive contains unsafe, duplicate or oversized entries".into());
        }
        let mode = entry.header().mode()?;
        if ![0o644, 0o755].contains(&mode) {
            return Err("Unsafe payload mode".into());
        }
        let file = root.join(&name);
        fs::create_dir_all(file.parent().ok_or("Missing payload parent")?)?;
        publication::stream(&file, mode, &mut entry, size, &atomic_fs::DefaultBarriers)?;
    }
    let manifest: Manifest = serde_json::from_slice(&manifest::read_bounded(
        &root.join("manifest.json"),
        1024 * 1024,
    )?)?;
    manifest.validate(target)?;
    if manifest.part != part
        || names
            != manifest
                .files
                .keys()
                .cloned()
                .chain(["manifest.json".into()])
                .collect()
    {
        return Err("Manifest and archive payload differ".into());
    }
    manifest.check_files(&root)?;
    manifest.check_versions(&root)?;
    Ok(manifest)
}
