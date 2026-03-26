//! Release archives: a gzip-compressed tar with a single `leanck/` root.

use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use crate::{create_dir_all, file_name, sorted_entries, write};

/// Archive the unpacked distribution `root` as `<output>/leanck-<target>.tar.gz`
/// with a `.sha256` sidecar, returning the archive path.
///
/// Entries are sorted and keep their file modes. The archive holds only
/// regular files and directories, as `leanck update` and `install.sh` require.
pub fn write_release_archive(root: &Path, output: &Path, target: &str) -> Result<PathBuf> {
    create_dir_all(output)?;
    let archive = output.join(format!("leanck-{target}.tar.gz"));
    let file = File::create(&archive)
        .with_context(|| format!("{}: cannot create archive", archive.display()))?;
    let encoder = flate2::write::GzEncoder::new(BufWriter::new(file), flate2::Compression::best());
    let mut builder = tar::Builder::new(encoder);
    append(&mut builder, root, "leanck")?;
    builder
        .into_inner()
        .and_then(|encoder| encoder.finish())
        .and_then(|mut writer| writer.flush())
        .with_context(|| format!("{}: cannot write archive", archive.display()))?;
    let mut hasher = Sha256::new();
    let mut reader =
        File::open(&archive).with_context(|| format!("{}: cannot read", archive.display()))?;
    let mut buffer = vec![0; 1 << 16];
    loop {
        match reader.read(&mut buffer)? {
            0 => break,
            read => hasher.update(&buffer[..read]),
        }
    }
    let name = file_name(&archive)?;
    write(
        &archive.with_file_name(format!("{name}.sha256")),
        format!("{}  {name}\n", hex::encode(hasher.finalize())),
    )?;
    Ok(archive)
}

fn append<W: Write>(builder: &mut tar::Builder<W>, path: &Path, name: &str) -> Result<()> {
    let meta = fs::symlink_metadata(path)
        .with_context(|| format!("{}: cannot read metadata", path.display()))?;
    if !meta.is_file() && !meta.is_dir() {
        bail!(
            "{}: archives may hold only regular files and directories",
            path.display()
        );
    }
    let mut header = tar::Header::new_ustar();
    header.set_metadata_in_mode(&meta, tar::HeaderMode::Complete);
    header.set_mode(meta.permissions().mode() & 0o7777);
    if !name.is_ascii() || header.set_path(name).is_err() {
        // Like Python's PAX format: the extended header holds the real name.
        builder.append_pax_extensions([("path", name.as_bytes())])?;
        let fallback: String = name
            .chars()
            .map(|c| if c.is_ascii() { c } else { '_' })
            .collect();
        let start = fallback.len().saturating_sub(99);
        header.set_path(fallback[start..].trim_start_matches('/'))?;
    }
    header.set_cksum();
    if meta.is_dir() {
        builder.append(&header, io::empty())?;
        for child in sorted_entries(path)? {
            append(builder, &child, &format!("{name}/{}", file_name(&child)?))?;
        }
    } else {
        let file = File::open(path).with_context(|| format!("{}: cannot read", path.display()))?;
        builder.append(&header, file)?;
    }
    Ok(())
}
