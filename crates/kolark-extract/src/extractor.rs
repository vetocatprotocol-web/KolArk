//! Safe extraction

use kolark_codec::blake3_hash;
use kolark_core::{ArchiveEntry, ArchiveReader};
use kolark_format::FormatError;
use std::fs;
use std::io::Write;
use std::path::Path;
use tempfile::NamedTempFile;

#[derive(Debug, Clone, Default)]
pub struct ExtractOptions {
    pub overwrite: bool,
    pub preserve_mode: bool,
    pub preserve_time: bool,
    pub strip_components: usize,
    pub dry_run: bool,
    pub max_output_bytes: Option<u64>,
    pub max_files: Option<u32>,
    pub max_memory: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct ExtractReport {
    pub file_count: u32,
    pub total_bytes: u64,
    pub skipped: u32,
}

pub fn extract_all<R: std::io::Read + std::io::Seek>(
    reader: &mut ArchiveReader<R>,
    destination: &Path,
    options: &ExtractOptions,
) -> Result<ExtractReport, FormatError> {
    let entries = reader.entries().to_vec();
    if options
        .max_files
        .is_some_and(|limit| entries.len() > limit as usize)
    {
        return Err(FormatError::ResourceLimitExceeded(
            "file count exceeds extraction limit".to_string(),
        ));
    }
    if let Some(limit) = options.max_memory {
        if entries.iter().any(|entry| entry.size > limit) {
            return Err(FormatError::ResourceLimitExceeded(
                "file size exceeds extraction memory limit".to_string(),
            ));
        }
    }
    let total_size = entries.iter().try_fold(0u64, |total, entry| {
        total
            .checked_add(entry.size)
            .ok_or_else(|| FormatError::IntegerOverflow("extraction output size".to_string()))
    })?;
    if options
        .max_output_bytes
        .is_some_and(|limit| total_size > limit)
    {
        return Err(FormatError::ResourceLimitExceeded(
            "extraction output exceeds byte limit".to_string(),
        ));
    }
    let destination = if options.dry_run {
        destination.to_path_buf()
    } else {
        fs::create_dir_all(destination)?;
        fs::canonicalize(destination)?
    };

    let mut report = ExtractReport {
        file_count: 0,
        total_bytes: 0,
        skipped: 0,
    };

    for entry in &entries {
        if extract_file(reader, entry, &destination, options)? {
            report.file_count += 1;
            report.total_bytes += entry.size;
        } else {
            report.skipped += 1;
        }
    }

    Ok(report)
}

fn extract_file<R: std::io::Read + std::io::Seek>(
    reader: &mut ArchiveReader<R>,
    entry: &ArchiveEntry,
    destination: &Path,
    options: &ExtractOptions,
) -> Result<bool, FormatError> {
    let path = entry
        .path
        .to_str()
        .ok_or_else(|| FormatError::InvalidPath("path is not valid UTF-8".to_string()))?;
    let components: Vec<&str> = path.split('/').collect();
    if path.is_empty()
        || path.len() > 4096
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains('\0')
        || components
            .iter()
            .any(|part| part.is_empty() || *part == "." || *part == "..")
        || components.first().is_some_and(|part| part.contains(':'))
    {
        return Err(FormatError::InvalidPath(format!(
            "unsafe archive path: {path}"
        )));
    }

    let data = reconstruct_file(reader, entry)?;
    let computed_hash = blake3_hash(&data);
    if computed_hash != entry.content_hash {
        return Err(FormatError::HashMismatch {
            expected: entry.content_hash.to_vec(),
            actual: computed_hash.to_vec(),
        });
    }

    let remaining = components
        .iter()
        .skip(options.strip_components)
        .copied()
        .collect::<Vec<_>>();
    if remaining.is_empty() {
        return Ok(false);
    }
    if options.dry_run {
        return Ok(true);
    }

    let mut parent = destination.to_path_buf();
    for component in &remaining[..remaining.len() - 1] {
        parent.push(component);
        match fs::symlink_metadata(&parent) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(FormatError::InvalidPath(format!(
                    "unsafe extraction directory: {}",
                    parent.display()
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&parent)?;
            }
            Err(error) => return Err(error.into()),
        }
    }

    let target_path = parent.join(remaining[remaining.len() - 1]);
    match fs::symlink_metadata(&target_path) {
        Ok(_) if !options.overwrite => return Ok(false),
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(FormatError::InvalidPath(format!(
                "refusing to overwrite symlink: {}",
                target_path.display()
            )));
        }
        Ok(metadata) if !metadata.is_file() => {
            return Err(FormatError::DestinationExists(
                target_path.display().to_string(),
            ));
        }
        Ok(_) => {
            return Err(FormatError::DestinationExists(
                target_path.display().to_string(),
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    let mut temp = NamedTempFile::new_in(&parent)?;
    temp.write_all(&data)?;
    temp.as_file_mut().sync_all()?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if options.preserve_mode {
            let mode = entry.mode & 0o777;
            temp.as_file()
                .set_permissions(fs::Permissions::from_mode(mode as u32))?;
        }
    }

    if options.preserve_time {
        let modified = if entry.mtime >= 0 {
            std::time::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(entry.mtime as u64))
        } else {
            std::time::UNIX_EPOCH
                .checked_sub(std::time::Duration::from_secs(entry.mtime.unsigned_abs()))
        }
        .ok_or_else(|| FormatError::InvalidMetadata("timestamp out of range".to_string()))?;
        let times = std::fs::FileTimes::new().set_modified(modified);
        temp.as_file().set_times(times)?;
    }

    if options.overwrite {
        temp.persist(&target_path)
            .map_err(|error| FormatError::Io(error.error))?;
    } else {
        temp.persist_noclobber(&target_path).map_err(|error| {
            if error.error.kind() == std::io::ErrorKind::AlreadyExists {
                FormatError::DestinationExists(target_path.display().to_string())
            } else {
                FormatError::Io(error.error)
            }
        })?;
    }
    Ok(true)
}

fn reconstruct_file<R: std::io::Read + std::io::Seek>(
    reader: &mut ArchiveReader<R>,
    entry: &ArchiveEntry,
) -> Result<Vec<u8>, FormatError> {
    let capacity = usize::try_from(entry.size).map_err(|_| {
        FormatError::ResourceLimitExceeded("file size exceeds address space".to_string())
    })?;
    let mut output = Vec::new();
    output.try_reserve_exact(capacity).map_err(|_| {
        FormatError::ResourceLimitExceeded("unable to reserve file output".to_string())
    })?;

    for &chunk_idx in &entry.chunk_refs {
        let chunk = reader.read_chunk(chunk_idx)?;
        let new_len = output
            .len()
            .checked_add(chunk.len())
            .ok_or_else(|| FormatError::IntegerOverflow("reconstructed file size".to_string()))?;
        if new_len > capacity {
            return Err(FormatError::InvalidMetadata(
                "chunk references exceed declared file size".to_string(),
            ));
        }
        output.extend_from_slice(&chunk);
    }

    if output.len() != capacity {
        return Err(FormatError::InvalidMetadata(format!(
            "reconstructed file has {} bytes, expected {}",
            output.len(),
            capacity
        )));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kolark_core::{create_archive, open_archive, CreateOptions, InputEntry, ReaderOptions};
    use std::io::Cursor;

    #[test]
    fn extract_round_trip_and_dry_run_verify_contents() {
        let current_dir = std::env::current_dir().unwrap();
        let source_dir = tempfile::Builder::new()
            .prefix("kolark-extract-source-")
            .tempdir_in(&current_dir)
            .unwrap();
        let relative_dir = source_dir.path().strip_prefix(&current_dir).unwrap();
        let contents = b"safe extraction round trip".repeat(12);
        std::fs::write(source_dir.path().join("payload.bin"), &contents).unwrap();
        let input = InputEntry {
            path: relative_dir.join("payload.bin"),
            ..InputEntry::default()
        };
        let mut archive = Cursor::new(Vec::new());
        create_archive(&mut archive, &[input], &CreateOptions::default()).unwrap();
        let bytes = archive.into_inner();

        let destination = tempfile::tempdir().unwrap();
        let mut reader =
            open_archive(Cursor::new(bytes.clone()), &ReaderOptions::default()).unwrap();
        let dry_run = extract_all(
            &mut reader,
            destination.path(),
            &ExtractOptions {
                dry_run: true,
                ..ExtractOptions::default()
            },
        )
        .unwrap();
        assert_eq!(dry_run.file_count, 1);
        assert_eq!(dry_run.total_bytes, contents.len() as u64);

        let mut reader = open_archive(Cursor::new(bytes), &ReaderOptions::default()).unwrap();
        let report =
            extract_all(&mut reader, destination.path(), &ExtractOptions::default()).unwrap();
        assert_eq!(report.file_count, 1);
        assert_eq!(
            std::fs::read(destination.path().join(&reader.entries()[0].path)).unwrap(),
            contents
        );
    }

    #[test]
    fn output_limit_is_checked_before_writing() {
        let current_dir = std::env::current_dir().unwrap();
        let source_dir = tempfile::Builder::new()
            .prefix("kolark-extract-limit-")
            .tempdir_in(&current_dir)
            .unwrap();
        let relative_dir = source_dir.path().strip_prefix(&current_dir).unwrap();
        std::fs::write(source_dir.path().join("payload.bin"), b"larger than limit").unwrap();
        let mut archive = Cursor::new(Vec::new());
        create_archive(
            &mut archive,
            &[InputEntry {
                path: relative_dir.join("payload.bin"),
                ..InputEntry::default()
            }],
            &CreateOptions::default(),
        )
        .unwrap();

        let destination = tempfile::tempdir().unwrap();
        let mut reader =
            open_archive(Cursor::new(archive.into_inner()), &ReaderOptions::default()).unwrap();
        let result = extract_all(
            &mut reader,
            destination.path(),
            &ExtractOptions {
                max_output_bytes: Some(1),
                ..ExtractOptions::default()
            },
        );
        assert!(matches!(result, Err(FormatError::ResourceLimitExceeded(_))));
        assert_eq!(std::fs::read_dir(destination.path()).unwrap().count(), 0);
    }

    #[test]
    fn aggregate_output_limit_is_preflighted_before_writing() {
        let current_dir = std::env::current_dir().unwrap();
        let source_dir = tempfile::Builder::new()
            .prefix("kolark-extract-total-limit-")
            .tempdir_in(&current_dir)
            .unwrap();
        let relative_dir = source_dir.path().strip_prefix(&current_dir).unwrap();
        std::fs::write(source_dir.path().join("first.bin"), b"123").unwrap();
        std::fs::write(source_dir.path().join("second.bin"), b"4567").unwrap();

        let inputs = ["first.bin", "second.bin"]
            .iter()
            .map(|name| InputEntry {
                path: relative_dir.join(name),
                ..InputEntry::default()
            })
            .collect::<Vec<_>>();
        let mut archive = Cursor::new(Vec::new());
        create_archive(&mut archive, &inputs, &CreateOptions::default()).unwrap();

        let destination = tempfile::tempdir().unwrap();
        let mut reader =
            open_archive(Cursor::new(archive.into_inner()), &ReaderOptions::default()).unwrap();
        let result = extract_all(
            &mut reader,
            destination.path(),
            &ExtractOptions {
                max_output_bytes: Some(6),
                ..ExtractOptions::default()
            },
        );
        assert!(matches!(result, Err(FormatError::ResourceLimitExceeded(_))));
        assert_eq!(std::fs::read_dir(destination.path()).unwrap().count(), 0);
    }
}
