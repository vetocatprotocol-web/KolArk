//! KolArk command line interface.

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use kolark_codec::{blake3_hash, lz_compress, lz_decompress};
use kolark_core::{
    create_archive, open_archive, CreateOptions, InputEntry, ReaderOptions, VerifyLevel,
};
use kolark_extract::{extract_all, ExtractOptions};
use kolark_format::FormatError;
use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;
use tempfile::NamedTempFile;

#[derive(Parser)]
#[command(name = "kolark", version, about = "KolArk - Columnar Archive")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(short, long, global = true)]
    quiet: bool,

    #[arg(short, long, global = true)]
    verbose: bool,

    #[arg(long, global = true)]
    json: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a new archive
    Create {
        /// Output archive path
        #[arg(value_name = "ARCHIVE")]
        archive: PathBuf,

        /// Input paths to archive
        #[arg(value_name = "PATHS", required = true, num_args = 1..)]
        paths: Vec<PathBuf>,

        /// Force overwrite
        #[arg(short, long)]
        force: bool,

        /// Chunk minimum size
        #[arg(long, value_name = "BYTES")]
        chunk_min: Option<u32>,

        /// Chunk average size
        #[arg(long, value_name = "BYTES")]
        chunk_avg: Option<u32>,

        /// Chunk maximum size
        #[arg(long, value_name = "BYTES")]
        chunk_max: Option<u32>,
    },

    /// List archive contents
    List {
        /// Archive path
        #[arg(value_name = "ARCHIVE")]
        archive: PathBuf,
    },

    /// Extract archive
    Extract {
        /// Archive path
        #[arg(value_name = "ARCHIVE")]
        archive: PathBuf,

        /// Target directory
        #[arg(short, long, value_name = "DIR")]
        directory: Option<PathBuf>,

        /// Overwrite existing files
        #[arg(long)]
        overwrite: bool,

        /// Don't preserve file modes
        #[arg(long)]
        no_preserve_mode: bool,

        /// Don't preserve timestamps
        #[arg(long)]
        no_preserve_time: bool,

        /// Strip path components
        #[arg(long, value_name = "N")]
        strip_components: Option<usize>,

        /// Maximum number of files to read or extract
        #[arg(long, value_name = "COUNT")]
        max_files: Option<u32>,

        /// Maximum total extracted bytes
        #[arg(long, value_name = "BYTES")]
        max_output_bytes: Option<u64>,

        /// Maximum memory used for archive and file data
        #[arg(long, value_name = "BYTES")]
        max_memory: Option<u64>,

        /// Dry run (validate only)
        #[arg(long)]
        dry_run: bool,
    },

    /// Verify archive integrity
    Verify {
        /// Archive path
        #[arg(value_name = "ARCHIVE")]
        archive: PathBuf,
    },

    /// Show archive info
    Info {
        /// Archive path
        #[arg(value_name = "ARCHIVE")]
        archive: PathBuf,
    },

    /// Run built-in codec checks (not the full conformance suite)
    #[command(name = "test-vector")]
    TestVector,

    /// Show version
    Version,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("kolark: {error:#}");
            ExitCode::from(error_exit_code(&error))
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let quiet = cli.quiet;
    let json_output = cli.json;
    let _verbose = cli.verbose;

    match cli.command {
        Commands::Create {
            archive,
            paths,
            force,
            chunk_min,
            chunk_avg,
            chunk_max,
        } => {
            let current_dir =
                std::env::current_dir().context("unable to determine current directory")?;
            let entries = paths
                .iter()
                .map(|path| {
                    Ok(InputEntry {
                        path: archive_input_path(path, &current_dir)?,
                        ..InputEntry::default()
                    })
                })
                .collect::<Result<Vec<_>>>()?;

            let defaults = CreateOptions::default();
            let options = CreateOptions {
                force_overwrite: force,
                chunk_size_min: chunk_min.unwrap_or(defaults.chunk_size_min),
                chunk_size_avg: chunk_avg.unwrap_or(defaults.chunk_size_avg),
                chunk_size_max: chunk_max.unwrap_or(defaults.chunk_size_max),
                ..defaults
            };

            let parent = archive
                .parent()
                .filter(|path| !path.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            let mut temp = NamedTempFile::new_in(parent).with_context(|| {
                format!("unable to create temporary archive in {}", parent.display())
            })?;
            let summary = create_archive(&mut temp, &entries, &options)?;
            temp.as_file_mut().sync_all()?;

            if force {
                temp.persist(&archive)
                    .map_err(|error| error.error)
                    .with_context(|| format!("unable to replace {}", archive.display()))?;
            } else {
                temp.persist_noclobber(&archive).map_err(|error| {
                    if error.error.kind() == std::io::ErrorKind::AlreadyExists {
                        anyhow!(FormatError::DestinationExists(
                            archive.display().to_string()
                        ))
                    } else {
                        anyhow!(error.error)
                    }
                })?;
            }

            let output = json!({
                "archive": archive,
                "files": summary.file_count,
                "chunks": summary.chunk_count,
                "archive_size": summary.archive_size,
            });
            emit(json_output, quiet, output, || {
                format!(
                    "Created archive: {} files, {} chunks, {} bytes",
                    summary.file_count, summary.chunk_count, summary.archive_size
                )
            })?;
        }
        Commands::List { archive } => {
            let file = std::fs::File::open(&archive)
                .with_context(|| format!("unable to open {}", archive.display()))?;
            let reader = open_archive(file, &ReaderOptions::default())?;
            let entries: Vec<Value> = reader
                .entries()
                .iter()
                .map(|entry| {
                    json!({
                        "path": entry.path,
                        "size": entry.size,
                        "mtime": entry.mtime,
                        "mode": entry.mode,
                    })
                })
                .collect();
            if json_output {
                print_json(&json!(entries))?;
            } else if !quiet {
                for entry in reader.entries() {
                    println!("{} {} {}", entry.size, entry.mtime, entry.path.display());
                }
            }
        }
        Commands::Extract {
            archive,
            directory,
            overwrite,
            no_preserve_mode,
            no_preserve_time,
            strip_components,
            max_files,
            max_output_bytes,
            max_memory,
            dry_run,
        } => {
            let file = std::fs::File::open(&archive)
                .with_context(|| format!("unable to open {}", archive.display()))?;
            let reader_options = ReaderOptions {
                max_files,
                max_memory,
                max_output_bytes,
                ..ReaderOptions::default()
            };
            let mut reader = open_archive(file, &reader_options)?;
            let target_dir = match directory {
                Some(path) => path,
                None => std::env::current_dir().context("unable to determine current directory")?,
            };
            let options = ExtractOptions {
                overwrite,
                preserve_mode: !no_preserve_mode,
                preserve_time: !no_preserve_time,
                strip_components: strip_components.unwrap_or(0),
                dry_run,
                max_output_bytes,
                max_files,
                max_memory,
            };
            let report = extract_all(&mut reader, &target_dir, &options)?;
            let output = json!({
                "files": report.file_count,
                "bytes": report.total_bytes,
                "skipped": report.skipped,
                "dry_run": dry_run,
            });
            emit(json_output, quiet, output, || {
                format!(
                    "{} {} files, {} bytes",
                    if dry_run { "Validated" } else { "Extracted" },
                    report.file_count,
                    report.total_bytes
                )
            })?;
        }
        Commands::Verify { archive } => {
            let file = std::fs::File::open(&archive)
                .with_context(|| format!("unable to open {}", archive.display()))?;
            let mut reader = open_archive(file, &ReaderOptions::default())?;
            let report = reader.verify(VerifyLevel::Full)?;
            let output = json!({
                "valid": report.valid,
                "issues": report.issues,
            });
            if !report.valid {
                return Err(anyhow!("archive verification failed"));
            }
            emit(json_output, quiet, output, || {
                "Verification: PASSED".to_string()
            })?;
        }
        Commands::Info { archive } => {
            let file = std::fs::File::open(&archive)
                .with_context(|| format!("unable to open {}", archive.display()))?;
            let reader = open_archive(file, &ReaderOptions::default())?;
            let info = reader.info();
            let output = json!({
                "format_version": info.format_version,
                "files": info.file_count,
                "chunks": info.chunk_count,
                "archive_size": info.archive_size,
                "chunk_size_min": info.chunk_size_min,
                "chunk_size_avg": info.chunk_size_avg,
                "chunk_size_max": info.chunk_size_max,
            });
            emit(json_output, quiet, output, || {
                format!(
                    "Format version: {}\nFiles: {}\nChunks: {}\nArchive size: {} bytes\nChunk size min: {}\nChunk size avg: {}\nChunk size max: {}",
                    info.format_version,
                    info.file_count,
                    info.chunk_count,
                    info.archive_size,
                    info.chunk_size_min,
                    info.chunk_size_avg,
                    info.chunk_size_max
                )
            })?;
        }
        Commands::TestVector => {
            run_builtin_codec_checks()?;
            let output = json!({
                "passed": true,
                "scope": "built-in codec checks",
                "conformance_vectors": false,
            });
            emit(json_output, quiet, output, || {
                "Built-in codec checks: PASSED (conformance archive vectors are not bundled)"
                    .to_string()
            })?;
        }
        Commands::Version => {
            let output = json!({
                "application_version": env!("CARGO_PKG_VERSION"),
                "format_version": 1,
            });
            emit(json_output, quiet, output, || {
                format!("kolark {}\nFormat version: 1", env!("CARGO_PKG_VERSION"))
            })?;
        }
    }

    Ok(())
}

fn archive_input_path(path: &Path, current_dir: &Path) -> Result<PathBuf> {
    let relative = if path.is_absolute() {
        path.strip_prefix(current_dir).map_err(|_| {
            anyhow!(
                "input {} is outside the current directory and has no safe archive-relative path",
                path.display()
            )
        })?
    } else {
        path
    };

    let mut normalized = PathBuf::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(anyhow!(
                    "input {} must resolve to a path inside the current directory",
                    path.display()
                ));
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err(anyhow!("input path must name a file"));
    }
    Ok(normalized)
}

fn emit<F>(json_output: bool, quiet: bool, value: Value, text: F) -> Result<()>
where
    F: FnOnce() -> String,
{
    if json_output {
        print_json(&value)
    } else if !quiet {
        println!("{}", text());
        Ok(())
    } else {
        Ok(())
    }
}

fn print_json(value: &Value) -> Result<()> {
    println!("{}", serde_json::to_string(value)?);
    Ok(())
}

fn error_exit_code(error: &anyhow::Error) -> u8 {
    if let Some(error) = error.downcast_ref::<FormatError>() {
        return match error {
            FormatError::UnsupportedVersion(_)
            | FormatError::UnsupportedFlags(_)
            | FormatError::UnsupportedFeature(_) => 4,
            FormatError::ResourceLimitExceeded(_) => 6,
            FormatError::HashMismatch { .. } => 7,
            FormatError::DestinationExists(_) | FormatError::PathCollision(_) => 8,
            FormatError::Io(_) => 5,
            _ => 3,
        };
    }
    if error.downcast_ref::<std::io::Error>().is_some() {
        5
    } else {
        1
    }
}

fn run_builtin_codec_checks() -> Result<()> {
    const EMPTY_BLAKE3: [u8; 32] = [
        0xaf, 0x13, 0x49, 0xb9, 0xf5, 0xf9, 0xa1, 0xa6, 0xa0, 0x40, 0x4d, 0xea, 0x36, 0xdc, 0xc9,
        0x49, 0x9b, 0xcb, 0x25, 0xc9, 0xad, 0xc1, 0x12, 0xb7, 0xcc, 0x9a, 0x93, 0xca, 0xe4, 0x1f,
        0x32, 0x62,
    ];
    if blake3_hash(b"") != EMPTY_BLAKE3 {
        return Err(anyhow!("BLAKE3 empty-input known-answer check failed"));
    }

    for sample in [
        b"".as_slice(),
        b"KolArk codec self-check".as_slice(),
        &b"repeated archive content ".repeat(64),
    ] {
        let compressed = lz_compress(sample);
        let decompressed = lz_decompress(&compressed, sample.len())
            .map_err(|error| anyhow!("KolArk-LZ self-check failed: {error}"))?;
        if decompressed != sample {
            return Err(anyhow!("KolArk-LZ round-trip self-check failed"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_paths_are_normalized_without_escape() {
        let root = Path::new("/workspace");
        assert_eq!(
            archive_input_path(Path::new("./nested/file"), root).unwrap(),
            PathBuf::from("nested/file")
        );
        assert_eq!(
            archive_input_path(Path::new("/workspace/nested/file"), root).unwrap(),
            PathBuf::from("nested/file")
        );
        assert!(archive_input_path(Path::new("../outside"), root).is_err());
        assert!(archive_input_path(Path::new("/outside/file"), root).is_err());
    }

    #[test]
    fn error_categories_follow_cli_contract() {
        assert_eq!(
            error_exit_code(&anyhow!(FormatError::UnsupportedVersion(2))),
            4
        );
        assert_eq!(
            error_exit_code(&anyhow!(FormatError::ResourceLimitExceeded(
                "limit".to_string()
            ))),
            6
        );
        assert_eq!(
            error_exit_code(&anyhow!(FormatError::DestinationExists(
                "archive.kola".to_string()
            ))),
            8
        );
    }

    #[test]
    fn built_in_codec_checks_pass() {
        run_builtin_codec_checks().unwrap();
    }
}
