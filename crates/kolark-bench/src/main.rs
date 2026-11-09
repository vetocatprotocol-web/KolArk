use anyhow::{anyhow, bail, Context, Result};
use kolark_core::{
    create_archive, open_archive, CreateOptions, InputEntry, ReaderOptions, VerifyLevel,
};
use kolark_extract::{extract_all, ExtractOptions};
use serde::{Deserialize, Serialize};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use tempfile::TempDir;

const WORKLOADS: [&str; 4] = [
    "small-files",
    "large-unique",
    "large-repetitive",
    "duplicate-files",
];
const SMALL_FILE_COUNT: usize = 256;
const SMALL_FILE_SIZE: usize = 4096;
const DUPLICATE_FILE_COUNT: usize = 4;

#[derive(Debug, Serialize)]
struct BenchmarkSuite {
    schema_version: u32,
    iterations: usize,
    large_file_size_bytes: usize,
    environment: Environment,
    workloads: Vec<WorkloadResult>,
}

#[derive(Debug, Serialize)]
struct Environment {
    operating_system: &'static str,
    architecture: &'static str,
    logical_cpus: Option<usize>,
    peak_memory_metric: &'static str,
}

#[derive(Debug, Deserialize, Serialize)]
struct WorkloadResult {
    name: String,
    input_files: usize,
    input_bytes: u64,
    archive_bytes: u64,
    archive_to_input_ratio: f64,
    deduplication_ratio: f64,
    chunk_count: u32,
    create_samples_ms: Vec<f64>,
    create_median_ms: f64,
    create_mib_per_second: f64,
    open_samples_ms: Vec<f64>,
    open_median_ms: f64,
    list_samples_ms: Vec<f64>,
    list_median_ms: f64,
    verify_samples_ms: Vec<f64>,
    verify_median_ms: f64,
    verify_mib_per_second: f64,
    extract_samples_ms: Vec<f64>,
    extract_median_ms: f64,
    extract_mib_per_second: f64,
    process_peak_memory_mib: Option<f64>,
}

struct Dataset {
    name: String,
    entries: Vec<InputEntry>,
    files: usize,
    input_bytes: u64,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("kolark-bench: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|argument| argument == "--worker") {
        return run_worker(&args[1..]);
    }

    let (iterations, size_mib) = parse_options(&args)?;
    let executable = std::env::current_exe().context("unable to locate benchmark executable")?;
    let size_bytes = size_mib
        .checked_mul(1024 * 1024)
        .ok_or_else(|| anyhow!("--size-mib is too large"))?;

    let mut workloads = Vec::with_capacity(WORKLOADS.len());
    for workload in WORKLOADS {
        let output = Command::new(&executable)
            .arg("--worker")
            .arg(workload)
            .arg(iterations.to_string())
            .arg(size_bytes.to_string())
            .output()
            .with_context(|| format!("failed to launch workload {workload}"))?;
        if !output.status.success() {
            bail!(
                "workload {workload} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        workloads.push(
            serde_json::from_slice::<WorkloadResult>(&output.stdout)
                .with_context(|| format!("invalid result from workload {workload}"))?,
        );
    }

    let suite = BenchmarkSuite {
        schema_version: 1,
        iterations,
        large_file_size_bytes: size_bytes,
        environment: Environment {
            operating_system: std::env::consts::OS,
            architecture: std::env::consts::ARCH,
            logical_cpus: std::thread::available_parallelism().ok().map(usize::from),
            peak_memory_metric: if cfg!(target_os = "linux") {
                "Linux VmHWM process high-water mark"
            } else {
                "unavailable on this platform"
            },
        },
        workloads,
    };
    println!("{}", serde_json::to_string_pretty(&suite)?);
    Ok(())
}

fn parse_options(args: &[String]) -> Result<(usize, usize)> {
    let mut iterations = 3usize;
    let mut size_mib = 4usize;
    let mut index = 0;
    while index < args.len() {
        let option = args[index].as_str();
        if option == "--help" || option == "-h" {
            print_help();
            std::process::exit(0);
        }
        let value = args
            .get(index + 1)
            .ok_or_else(|| anyhow!("missing value for {option}"))?;
        match option {
            "--iterations" => {
                iterations = value
                    .parse()
                    .context("--iterations must be a positive integer")?
            }
            "--size-mib" => {
                size_mib = value
                    .parse()
                    .context("--size-mib must be a positive integer")?
            }
            _ => bail!("unknown option: {option}"),
        }
        index += 2;
    }
    if iterations == 0 || size_mib == 0 {
        bail!("--iterations and --size-mib must be greater than zero");
    }
    Ok((iterations, size_mib))
}

fn print_help() {
    println!(
        "KolArk performance benchmark\n\n\
         Usage: cargo run --release -p kolark-bench -- [OPTIONS]\n\n\
         Options:\n\
           --iterations N   Measured iterations per operation (default: 3)\n\
           --size-mib N     Size of large-file workloads (default: 4 MiB)\n\
           -h, --help        Show this help\n\n\
         Runs isolated workers for small files, unique large data, repetitive large data,\n\
         and duplicate files. Writes a JSON report to stdout."
    );
}

fn run_worker(args: &[String]) -> Result<()> {
    if args.len() != 3 {
        bail!("internal worker expects workload, iterations, and size bytes");
    }
    let name = args[0].as_str();
    if !WORKLOADS.contains(&name) {
        bail!("unknown benchmark workload: {name}");
    }
    let iterations: usize = args[1].parse().context("invalid worker iteration count")?;
    let size_bytes: usize = args[2].parse().context("invalid worker size")?;
    if iterations == 0 || size_bytes == 0 {
        bail!("worker iteration count and size must be positive");
    }

    let workspace = tempfile::tempdir().context("unable to create benchmark workspace")?;
    std::env::set_current_dir(workspace.path()).context("unable to enter benchmark workspace")?;
    let dataset = create_dataset(name, size_bytes)?;
    let result = measure_workload(dataset, iterations, workspace)?;
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}

fn create_dataset(name: &str, large_size: usize) -> Result<Dataset> {
    let root = Path::new("inputs");
    std::fs::create_dir(root)?;
    let mut entries = Vec::new();
    let mut input_bytes = 0u64;

    match name {
        "small-files" => {
            for index in 0..SMALL_FILE_COUNT {
                let path = root.join(format!("small-{index:04}.bin"));
                let data = pseudo_random_bytes(SMALL_FILE_SIZE, index as u64 + 1);
                std::fs::write(&path, &data)?;
                input_bytes = input_bytes
                    .checked_add(data.len() as u64)
                    .ok_or_else(|| anyhow!("input size overflow"))?;
                entries.push(input_entry(path));
            }
        }
        "large-unique" => {
            let path = root.join("large-unique.bin");
            let data = pseudo_random_bytes(large_size, 0x4b4f_4c41_524b);
            std::fs::write(&path, &data)?;
            input_bytes = data.len() as u64;
            entries.push(input_entry(path));
        }
        "large-repetitive" => {
            let path = root.join("large-repetitive.bin");
            let data = repetitive_bytes(large_size);
            std::fs::write(&path, &data)?;
            input_bytes = data.len() as u64;
            entries.push(input_entry(path));
        }
        "duplicate-files" => {
            let data = pseudo_random_bytes(large_size, 0x4445_4455_504c_4943);
            for index in 0..DUPLICATE_FILE_COUNT {
                let path = root.join(format!("duplicate-{index}.bin"));
                std::fs::write(&path, &data)?;
                input_bytes = input_bytes
                    .checked_add(data.len() as u64)
                    .ok_or_else(|| anyhow!("input size overflow"))?;
                entries.push(input_entry(path));
            }
        }
        _ => bail!("unknown benchmark workload: {name}"),
    }

    Ok(Dataset {
        name: name.to_string(),
        files: entries.len(),
        entries,
        input_bytes,
    })
}

fn input_entry(path: PathBuf) -> InputEntry {
    InputEntry {
        path,
        mtime: Some(0),
        mode: Some(0o100644),
    }
}

fn pseudo_random_bytes(length: usize, mut state: u64) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(length);
    while bytes.len() < length {
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = state;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^= value >> 31;
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.truncate(length);
    bytes
}

fn repetitive_bytes(length: usize) -> Vec<u8> {
    const PATTERN: &[u8] = b"KolArk v1 benchmark: repeatable archive payload.\n";
    PATTERN.iter().copied().cycle().take(length).collect()
}

fn measure_workload(
    dataset: Dataset,
    iterations: usize,
    workspace: TempDir,
) -> Result<WorkloadResult> {
    let mut create_samples = Vec::with_capacity(iterations);
    let mut final_archive = Vec::new();
    let mut final_summary = None;
    for _ in 0..iterations {
        let start = Instant::now();
        let mut archive = Cursor::new(Vec::new());
        let summary = create_archive(&mut archive, &dataset.entries, &CreateOptions::default())?;
        create_samples.push(start.elapsed());
        final_archive = archive.into_inner();
        final_summary = Some(summary);
    }
    let summary = final_summary.ok_or_else(|| anyhow!("create benchmark did not run"))?;

    let mut open_samples = Vec::with_capacity(iterations);
    let mut list_samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = Instant::now();
        let reader = open_archive(
            Cursor::new(final_archive.as_slice()),
            &ReaderOptions::default(),
        )?;
        open_samples.push(start.elapsed());

        let start = Instant::now();
        let listed_bytes = reader
            .entries()
            .iter()
            .fold(0u64, |sum, entry| sum.wrapping_add(entry.size));
        std::hint::black_box((reader.info(), listed_bytes));
        list_samples.push(start.elapsed());
    }

    let mut verify_samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = Instant::now();
        let mut reader = open_archive(
            Cursor::new(final_archive.as_slice()),
            &ReaderOptions::default(),
        )?;
        let report = reader.verify(VerifyLevel::Full)?;
        if !report.valid {
            bail!("full verification reported invalid archive");
        }
        verify_samples.push(start.elapsed());
    }

    let extraction_roots = (0..iterations)
        .map(|index| {
            let path = workspace.path().join(format!("extract-{index}"));
            std::fs::create_dir(&path)?;
            Ok(path)
        })
        .collect::<Result<Vec<_>>>()?;
    let mut extract_samples = Vec::with_capacity(iterations);
    for destination in &extraction_roots {
        let mut reader = open_archive(
            Cursor::new(final_archive.as_slice()),
            &ReaderOptions::default(),
        )?;
        let start = Instant::now();
        let report = extract_all(&mut reader, destination, &ExtractOptions::default())?;
        extract_samples.push(start.elapsed());
        if report.file_count as usize != dataset.files || report.total_bytes != dataset.input_bytes
        {
            bail!("extraction did not reproduce the benchmark input sizes");
        }
    }

    let process_peak_memory_mib = process_peak_memory_bytes().map(|bytes| bytes as f64 / 1048576.0);
    let archive_to_input_ratio = if dataset.input_bytes == 0 {
        0.0
    } else {
        summary.archive_size as f64 / dataset.input_bytes as f64
    };
    let create_median = median(&create_samples);
    let verify_median = median(&verify_samples);
    let extract_median = median(&extract_samples);

    Ok(WorkloadResult {
        name: dataset.name,
        input_files: dataset.files,
        input_bytes: dataset.input_bytes,
        archive_bytes: summary.archive_size,
        archive_to_input_ratio,
        deduplication_ratio: summary.dedup_ratio,
        chunk_count: summary.chunk_count,
        create_samples_ms: milliseconds(&create_samples),
        create_median_ms: create_median,
        create_mib_per_second: throughput_mib_per_second(dataset.input_bytes, create_median),
        open_samples_ms: milliseconds(&open_samples),
        open_median_ms: median(&open_samples),
        list_samples_ms: milliseconds(&list_samples),
        list_median_ms: median(&list_samples),
        verify_samples_ms: milliseconds(&verify_samples),
        verify_median_ms: verify_median,
        verify_mib_per_second: throughput_mib_per_second(dataset.input_bytes, verify_median),
        extract_samples_ms: milliseconds(&extract_samples),
        extract_median_ms: extract_median,
        extract_mib_per_second: throughput_mib_per_second(dataset.input_bytes, extract_median),
        process_peak_memory_mib,
    })
}

fn median(samples: &[Duration]) -> f64 {
    let mut values: Vec<f64> = samples
        .iter()
        .map(|sample| sample.as_secs_f64() * 1000.0)
        .collect();
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    }
}

fn milliseconds(samples: &[Duration]) -> Vec<f64> {
    samples
        .iter()
        .map(|sample| sample.as_secs_f64() * 1000.0)
        .collect()
}

fn throughput_mib_per_second(bytes: u64, elapsed_ms: f64) -> f64 {
    if elapsed_ms == 0.0 {
        0.0
    } else {
        bytes as f64 / 1048576.0 / (elapsed_ms / 1000.0)
    }
}

#[cfg(target_os = "linux")]
fn process_peak_memory_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let high_water_kib = status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()?;
    high_water_kib.checked_mul(1024)
}

#[cfg(not(target_os = "linux"))]
fn process_peak_memory_bytes() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_workloads_have_expected_sizes_and_distinct_content() {
        assert_ne!(pseudo_random_bytes(1024, 1), pseudo_random_bytes(1024, 2));
        assert_eq!(repetitive_bytes(100), repetitive_bytes(100));
        assert_eq!(pseudo_random_bytes(17, 4).len(), 17);
    }

    #[test]
    fn median_handles_even_and_odd_samples() {
        assert_eq!(
            median(&[Duration::from_millis(1), Duration::from_millis(3)]),
            2.0
        );
        assert_eq!(
            median(&[
                Duration::from_millis(5),
                Duration::from_millis(1),
                Duration::from_millis(3)
            ]),
            3.0
        );
    }
}
