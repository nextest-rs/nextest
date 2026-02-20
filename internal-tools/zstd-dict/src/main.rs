// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Train and analyze zstd dictionaries for nextest record archives.
//!
//! This tool does the following.
//! 1. Scans the nextest recordings directory for existing store.zip archives.
//! 2. Extracts stdout and stderr samples from them.
//! 3. Trains separate zstd dictionaries for stdout and stderr.
//! 4. Analyzes compression improvement across archives.

use camino::{Utf8Path, Utf8PathBuf};
use clap::{Parser, Subcommand};
use color_eyre::eyre::{Context, Result, bail};
use eazip::{
    Archive, ArchiveWriter, CompressionMethod, read::File as EazipFile, write::FileOptions,
};
use etcetera::BaseStrategy;
use nextest_runner::record::{NEXTEST_STATE_DIR_ENV, OutputDict};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufRead, BufReader, BufWriter, Read, Seek, Write},
};

#[derive(Parser)]
#[command(name = "zstd-dict")]
#[command(about = "Train and analyze zstd dictionaries for nextest record archives")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Train dictionaries from existing archives.
    Train {
        /// Output directory for dictionaries.
        #[arg(short, long, default_value = "/tmp/nextest-dicts")]
        output_dir: Utf8PathBuf,

        /// Maximum number of samples per category.
        #[arg(short, long, default_value = "10000")]
        max_samples: usize,

        /// Dictionary size in bytes.
        #[arg(short, long, default_value = "65536")]
        dict_size: usize,
    },

    /// Recompress an archive using trained dictionaries.
    Recompress {
        /// Path to store.zip to recompress.
        archive: Utf8PathBuf,

        /// Directory containing trained dictionaries (uses embedded if not specified).
        #[arg(short, long)]
        dict_dir: Option<Utf8PathBuf>,

        /// Output path for recompressed archive.
        #[arg(short, long)]
        output: Option<Utf8PathBuf>,
    },

    /// Analyze compression across all archives.
    Analyze {
        /// Directory containing trained dictionaries.
        #[arg(short, long, conflicts_with = "no_dict")]
        dict_dir: Option<Utf8PathBuf>,

        /// Maximum number of archives to analyze.
        #[arg(short, long, default_value = "50")]
        max_archives: usize,

        /// Compress with plain zstd (no dictionary) instead of dict-compressed.
        /// Useful for comparing dict vs no-dict compression.
        #[arg(long, conflicts_with = "dict_dir")]
        no_dict: bool,
    },

    /// Test different dictionary sizes to find optimal size.
    SizeSweep {
        /// Maximum number of samples to use for training.
        #[arg(short, long, default_value = "10000")]
        max_samples: usize,
    },

    /// Sweep compression levels to find optimal level.
    LevelSweep {
        /// Maximum number of samples per category.
        #[arg(short, long, default_value = "10000")]
        max_samples: usize,

        /// Directory containing trained dictionaries.
        #[arg(short, long, conflicts_with = "no_dict")]
        dict_dir: Option<Utf8PathBuf>,

        /// Test with plain zstd only (no dictionary).
        #[arg(long, conflicts_with = "dict_dir")]
        no_dict: bool,
    },

    /// Analyze compression per-project.
    PerProject {
        /// Directory containing trained dictionaries.
        #[arg(short, long, conflicts_with = "no_dict")]
        dict_dir: Option<Utf8PathBuf>,

        /// Compress with plain zstd (no dictionary) instead of dict-compressed.
        #[arg(long, conflicts_with = "dict_dir")]
        no_dict: bool,
    },

    /// Dump per-entry compression sizes for CDF plotting.
    ///
    /// Outputs to stdout in space-separated format suitable for gnuplot:
    ///
    ///   category uncompressed dict_compressed plain_compressed
    ///
    /// where `category` is a label for the entry (for example, stdout, stderr,
    /// or combined), and the remaining columns are sizes in bytes. One line per
    /// test output entry. Not pre-sorted; the accompanying gnuplot scripts
    /// handle sorting and CDF computation.
    DumpCdf {
        /// Directory containing trained dictionaries.
        #[arg(short, long)]
        dict_dir: Option<Utf8PathBuf>,

        /// Maximum number of archives to process.
        #[arg(short, long, default_value = "50")]
        max_archives: usize,
    },
}

fn main() -> Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();

    match cli.command {
        Command::Train {
            output_dir,
            max_samples,
            dict_size,
        } => train_dictionaries(&output_dir, max_samples, dict_size),
        Command::Recompress {
            archive,
            dict_dir,
            output,
        } => recompress_archive(&archive, dict_dir.as_deref(), output.as_deref()),
        Command::Analyze {
            dict_dir,
            max_archives,
            no_dict,
        } => {
            let source = DictSource::from_cli(dict_dir, no_dict);
            analyze_compression(&source, max_archives)
        }
        Command::SizeSweep { max_samples } => size_sweep(max_samples),
        Command::LevelSweep {
            max_samples,
            dict_dir,
            no_dict,
        } => {
            let source = DictSource::from_cli(dict_dir, no_dict);
            level_sweep(max_samples, &source)
        }
        Command::PerProject { dict_dir, no_dict } => {
            let source = DictSource::from_cli(dict_dir, no_dict);
            analyze_per_project(&source)
        }
        Command::DumpCdf {
            dict_dir,
            max_archives,
        } => dump_cdf(dict_dir.as_deref(), max_archives),
    }
}

// ---
// State directory and archive discovery
// ---

/// Find the nextest recordings directory.
fn find_state_dir() -> Result<Utf8PathBuf> {
    if let Ok(base_dir) = std::env::var(NEXTEST_STATE_DIR_ENV) {
        return Ok(Utf8PathBuf::from(base_dir));
    }

    let base = etcetera::base_strategy::choose_base_strategy()
        .wrap_err("failed to determine base directories")?;
    let nextest_dir = if let Some(state_dir) = base.state_dir() {
        state_dir.join("nextest")
    } else {
        base.cache_dir().join("nextest")
    };

    let nextest_state =
        Utf8PathBuf::try_from(nextest_dir).wrap_err("state path is not valid UTF-8")?;
    Ok(nextest_state)
}

/// Find all store.zip files in the recordings directory.
fn find_archives() -> Result<Vec<Utf8PathBuf>> {
    let state_dir = find_state_dir()?;
    let projects_dir = state_dir.join("projects");

    if !projects_dir.exists() {
        bail!("no nextest recordings found at {}", projects_dir);
    }

    let mut archives = Vec::new();
    let mut skipped_tmp = 0;
    for entry in walkdir::WalkDir::new(&projects_dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.file_name() == Some(std::ffi::OsStr::new("store.zip"))
            && let Ok(utf8_path) = Utf8PathBuf::try_from(path.to_path_buf())
        {
            // Skip temporary fixture directories (nextest's own test fixtures).
            if utf8_path.as_str().contains("_stmp") {
                skipped_tmp += 1;
                continue;
            }
            archives.push(utf8_path);
        }
    }

    eprintln!(
        "Found {} archives ({} tmp fixtures skipped)",
        archives.len(),
        skipped_tmp
    );
    Ok(archives)
}

// ---
// Sample categories
// ---

/// Sample categories for dictionary training.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SampleCategory {
    Stdout,
    Stderr,
    Meta,
}

impl SampleCategory {
    fn from_filename(name: &str) -> Option<Self> {
        if name.starts_with("out/") {
            if name.ends_with("-stdout") || name.ends_with("-combined") {
                Some(Self::Stdout)
            } else if name.ends_with("-stderr") {
                Some(Self::Stderr)
            } else {
                None
            }
        } else if name.starts_with("meta/") {
            Some(Self::Meta)
        } else {
            None
        }
    }

    fn dict_filename(&self) -> &'static str {
        match self {
            Self::Stdout => "stdout.dict",
            Self::Stderr => "stderr.dict",
            Self::Meta => "meta.dict",
        }
    }

    /// Convert to OutputDict for use with nextest-runner's dictionary system.
    fn to_output_dict(self) -> OutputDict {
        match self {
            Self::Stdout => OutputDict::Stdout,
            Self::Stderr => OutputDict::Stderr,
            Self::Meta => OutputDict::None,
        }
    }

    /// Get the embedded dictionary bytes for this category (if available).
    fn embedded_dict(&self) -> Option<&'static [u8]> {
        self.to_output_dict().dict_bytes()
    }
}

// ---
// Dictionary source
// ---

/// Where to load recompression dictionaries from.
enum DictSource {
    /// Do not use dictionaries; compress with plain zstd.
    NoDictionary,

    /// Use the dictionaries embedded in the nextest-runner binary.
    Embedded,

    /// Use dictionaries from the given directory on disk.
    Directory(Utf8PathBuf),
}

impl DictSource {
    /// Construct from the CLI flags. `--no-dict` and `--dict-dir` are mutually
    /// exclusive (enforced by clap).
    fn from_cli(dict_dir: Option<Utf8PathBuf>, no_dict: bool) -> Self {
        if no_dict {
            Self::NoDictionary
        } else if let Some(dir) = dict_dir {
            Self::Directory(dir)
        } else {
            Self::Embedded
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::NoDictionary => "plain zstd-3",
            Self::Embedded => "embedded dict",
            Self::Directory(_) => "custom dict",
        }
    }

    /// Load a dictionary for the given category. Returns `None` for
    /// `NoDictionary` or if the category has no dictionary.
    fn load(&self, category: SampleCategory) -> Result<Option<Vec<u8>>> {
        match self {
            Self::NoDictionary => Ok(None),
            Self::Embedded => Ok(category.embedded_dict().map(|d| d.to_vec())),
            Self::Directory(dir) => {
                let dict_path = dir.join(category.dict_filename());
                if dict_path.exists() {
                    Ok(Some(fs::read(&dict_path)?))
                } else {
                    // Fall back to embedded if not on disk.
                    Ok(category.embedded_dict().map(|d| d.to_vec()))
                }
            }
        }
    }
}

// ---
// Dictionary training
// ---

/// Train dictionaries from archived samples.
fn train_dictionaries(output_dir: &Utf8Path, max_samples: usize, dict_size: usize) -> Result<()> {
    fs::create_dir_all(output_dir)?;

    let archives = find_archives()?;
    let mut samples: HashMap<SampleCategory, Vec<Vec<u8>>> = HashMap::new();

    eprintln!("Collecting samples from {} archives...", archives.len());

    for archive_path in &archives {
        let file = File::open(archive_path)
            .wrap_err_with(|| format!("failed to open {}", archive_path))?;
        let reader = BufReader::new(file);

        let mut archive = match Archive::new(reader) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("Warning: failed to read {}: {}", archive_path, e);
                continue;
            }
        };

        let dicts = ArchiveDicts::load(&mut archive);

        let entry_count = archive.entries().len();
        for i in 0..entry_count {
            let mut file = archive.get_by_index(i).expect("index is in bounds");

            let name = file.metadata().name().to_string();
            let Some(category) = SampleCategory::from_filename(&name) else {
                continue;
            };

            let samples_for_category = samples.entry(category).or_default();
            if samples_for_category.len() >= max_samples {
                continue;
            }

            let data = match read_out_entry_raw(&mut file, category, &dicts) {
                Ok(d) if !d.is_empty() => d,
                _ => continue,
            };

            // Skip very large samples (>1MB) as they skew the dictionary.
            if data.len() <= 1_000_000 {
                samples_for_category.push(data);
            }
        }

        // Early exit if we have enough samples.
        let all_full = samples.values().all(|s| s.len() >= max_samples);
        if all_full {
            break;
        }
    }

    // Train and save dictionaries.
    for (category, category_samples) in &samples {
        eprintln!(
            "Training {:?} dictionary from {} samples...",
            category,
            category_samples.len()
        );

        if category_samples.is_empty() {
            eprintln!("  Skipping (no samples)");
            continue;
        }

        // Compute total size for statistics.
        let total_size: usize = category_samples.iter().map(|s| s.len()).sum();
        eprintln!(
            "  Total sample size: {} bytes ({} avg)",
            total_size,
            total_size / category_samples.len()
        );

        // Train the dictionary.
        let dict_data = zstd::dict::from_samples(category_samples, dict_size)
            .wrap_err_with(|| format!("failed to train {:?} dictionary", category))?;

        let dict_path = output_dir.join(category.dict_filename());
        fs::write(&dict_path, &dict_data)?;
        eprintln!("  Saved {} ({} bytes)", dict_path, dict_data.len());
    }

    eprintln!("\nDictionaries saved to {}", output_dir);
    Ok(())
}

// ---
// Dictionary loading
// ---

/// Load a dictionary, trying disk first then falling back to embedded.
fn load_dictionary(
    dict_dir: Option<&Utf8Path>,
    category: SampleCategory,
) -> Result<Option<Vec<u8>>> {
    // Try loading from disk first if a directory is provided.
    if let Some(dir) = dict_dir {
        let dict_path = dir.join(category.dict_filename());
        if dict_path.exists() {
            let data = fs::read(&dict_path)?;
            return Ok(Some(data));
        }
    }

    // Fall back to embedded dictionary.
    Ok(category.embedded_dict().map(|d| d.to_vec()))
}

// ---
// Archive recompression
// ---

/// Recompress a single archive using dictionaries.
fn recompress_archive(
    archive_path: &Utf8Path,
    dict_dir: Option<&Utf8Path>,
    output_path: Option<&Utf8Path>,
) -> Result<()> {
    // Load dictionaries (from disk or embedded).
    let stdout_dict = load_dictionary(dict_dir, SampleCategory::Stdout)?;
    let stderr_dict = load_dictionary(dict_dir, SampleCategory::Stderr)?;
    let meta_dict = load_dictionary(dict_dir, SampleCategory::Meta)?;

    if stdout_dict.is_none() && stderr_dict.is_none() && meta_dict.is_none() {
        bail!("no dictionaries found");
    }

    // Open source archive.
    let file = File::open(archive_path)?;
    let reader = BufReader::new(file);
    let mut source = Archive::new(reader)?;

    let original_size = fs::metadata(archive_path)?.len();

    // Create output archive.
    let output = output_path.map(|p| p.to_owned()).unwrap_or_else(|| {
        let stem = archive_path.file_stem().unwrap_or("archive");
        archive_path.with_file_name(format!("{}-recompressed.zip", stem))
    });

    let output_file = File::create(&output)?;
    let mut dest = ArchiveWriter::new(output_file);

    let mut stats = CompressionStats::default();

    let source_dicts = ArchiveDicts::load(&mut source);

    // Process each entry.
    let entry_count = source.entries().len();
    for i in 0..entry_count {
        let mut file = source.get_by_index(i).expect("index is in bounds");
        let name = file.metadata().name().to_string();
        let category = SampleCategory::from_filename(&name);

        // Read the raw (decompressed) data, handling dict-compressed entries.
        let data = if let Some(cat) = category {
            read_out_entry_raw(&mut file, cat, &source_dicts)?
        } else {
            let mut buf = Vec::new();
            file.read()?.read_to_end(&mut buf)?;
            buf
        };

        let original_compressed = file.metadata().compressed_size;
        stats.original_uncompressed += data.len() as u64;
        stats.original_compressed += original_compressed;

        // Choose compression strategy.
        let (new_compressed_size, method_name) = if let Some(cat) = category {
            let dict = match cat {
                SampleCategory::Stdout => stdout_dict.as_deref(),
                SampleCategory::Stderr => stderr_dict.as_deref(),
                SampleCategory::Meta => meta_dict.as_deref(),
            };

            if let Some(dict_data) = dict {
                // Compress with dictionary.
                let compressed = compress_with_dict(&data, dict_data, 3)?;
                let compressed_size = compressed.len() as u64;

                // Write as stored (already compressed).
                let mut options = FileOptions::default();
                options.compression_method = CompressionMethod::STORE;
                dest.add_file(&name, &compressed[..], &options)?;

                (compressed_size, format!("{:?}+dict", cat))
            } else {
                // No dictionary, use regular zstd.
                write_zstd(&mut dest, &name, &data)?
            }
        } else {
            // Unknown category, use regular zstd.
            write_zstd(&mut dest, &name, &data)?
        };

        stats.new_compressed += new_compressed_size;

        // Print per-file stats for interesting cases.
        if data.len() > 100 && data.len() < 10000 {
            let orig_ratio = data.len() as f64 / original_compressed as f64;
            let new_ratio = data.len() as f64 / new_compressed_size as f64;
            if (new_ratio - orig_ratio).abs() > 0.1 {
                eprintln!(
                    "  {}: {} -> {} (was {}) [{}]",
                    name,
                    data.len(),
                    new_compressed_size,
                    original_compressed,
                    method_name
                );
            }
        }
    }

    dest.finish()?;

    let new_size = fs::metadata(&output)?.len();

    eprintln!("\nCompression results for {}:", archive_path);
    eprintln!("  Original size:     {:>10} bytes", original_size);
    eprintln!("  Recompressed size: {:>10} bytes", new_size);
    eprintln!(
        "  Improvement:       {:>10.1}%",
        (1.0 - new_size as f64 / original_size as f64) * 100.0
    );
    eprintln!("\nDetailed stats:");
    eprintln!(
        "  Uncompressed:      {:>10} bytes",
        stats.original_uncompressed
    );
    eprintln!(
        "  Original zstd:     {:>10} bytes",
        stats.original_compressed
    );
    eprintln!("  Dict-compressed:   {:>10} bytes", stats.new_compressed);
    eprintln!("  Output: {}", output);

    Ok(())
}

/// Compress data using a zstd dictionary.
fn compress_with_dict(data: &[u8], dict: &[u8], level: i32) -> Result<Vec<u8>> {
    let dict = zstd::dict::EncoderDictionary::copy(dict, level);
    let mut encoder = zstd::stream::Encoder::with_prepared_dictionary(Vec::new(), &dict)?;
    encoder.write_all(data)?;
    Ok(encoder.finish()?)
}

/// Decompress data that was compressed with a zstd dictionary.
///
/// This is a local copy of the logic in nextest-runner's reader, without the
/// size limit (this tool only processes trusted local data).
fn decompress_with_dict(compressed: &[u8], dict: &[u8]) -> Result<Vec<u8>> {
    let dict = zstd::dict::DecoderDictionary::copy(dict);
    let mut decoder = zstd::stream::Decoder::with_prepared_dictionary(compressed, &dict)?;
    let mut decompressed = Vec::new();
    decoder.read_to_end(&mut decompressed)?;
    Ok(decompressed)
}

// ---
// Archive dictionary loading and decompression
// ---

/// Dictionaries embedded in a store.zip archive.
///
/// Newer archives store `out/` entries as zstd+dict compressed data with
/// `CompressionMethod::STORE`. The dictionaries needed to decompress these
/// entries are at `meta/stdout.dict` and `meta/stderr.dict` within the archive.
/// Older archives used `CompressionMethod::ZSTD` (no dictionary), and don't
/// contain these dictionary entries.
struct ArchiveDicts {
    stdout: Option<Vec<u8>>,
    stderr: Option<Vec<u8>>,
}

impl ArchiveDicts {
    /// Load dictionaries from the archive. Returns `None` values for any
    /// dictionary entries that are missing (e.g. old-format archives).
    fn load<R: BufRead + Seek>(archive: &mut Archive<R>) -> Self {
        let stdout = Self::read_dict_entry(archive, "meta/stdout.dict");
        let stderr = Self::read_dict_entry(archive, "meta/stderr.dict");
        Self { stdout, stderr }
    }

    fn read_dict_entry<R: BufRead + Seek>(archive: &mut Archive<R>, path: &str) -> Option<Vec<u8>> {
        let mut entry = archive.get_by_name(path)?;
        let mut data = Vec::new();
        match entry.read().and_then(|mut r| r.read_to_end(&mut data)) {
            Ok(_) => Some(data),
            Err(e) => {
                eprintln!("Warning: failed to read dictionary {path}: {e}");
                None
            }
        }
    }

    /// Get the dictionary bytes for the given output category.
    fn dict_for_category(&self, category: SampleCategory) -> Option<&[u8]> {
        match category {
            SampleCategory::Stdout => self.stdout.as_deref(),
            SampleCategory::Stderr => self.stderr.as_deref(),
            SampleCategory::Meta => None,
        }
    }
}

/// Read the raw (decompressed) data for an `out/` zip entry.
///
/// Handles both archive formats transparently:
/// - New format: entry is `CompressionMethod::STORE` containing zstd+dict data.
///   We decompress using the archive's embedded dictionary.
/// - Old format: entry is `CompressionMethod::ZSTD` and eazip decompresses it
///   for us.
fn read_out_entry_raw<R: BufRead + Seek>(
    file: &mut EazipFile<'_, R>,
    category: SampleCategory,
    dicts: &ArchiveDicts,
) -> Result<Vec<u8>> {
    // Extract compression method before taking the mutable read() borrow.
    let compression = file.metadata().compression_method;

    let mut data = Vec::new();
    file.read()?.read_to_end(&mut data)?;

    // If the entry is stored (not compressed by the zip layer) and we have a
    // dictionary for this category, the data is zstd+dict compressed and needs
    // to be decompressed.
    if compression == CompressionMethod::STORE
        && let Some(dict) = dicts.dict_for_category(category)
    {
        return decompress_with_dict(&data, dict);
    }

    // Otherwise eazip already decompressed the entry (old format with
    // CompressionMethod::ZSTD), or this category has no dictionary.
    Ok(data)
}

/// Write a file with standard zstd compression.
fn write_zstd(dest: &mut ArchiveWriter<File>, name: &str, data: &[u8]) -> Result<(u64, String)> {
    let mut options = FileOptions::default();
    options.compression_method = CompressionMethod::ZSTD;
    options.level = Some(3);
    dest.add_file(name, data, &options)?;

    // Estimate compressed size. eazip doesn't expose per-entry compressed
    // sizes, so we compress independently to get the approximate size.
    let compressed = zstd::encode_all(data, 3)?;
    Ok((compressed.len() as u64, "zstd".to_string()))
}

#[derive(Default)]
struct CompressionStats {
    original_uncompressed: u64,
    original_compressed: u64,
    new_compressed: u64,
}

// ---
// Compression analysis
// ---

/// Analyze compression improvement across all archives.
fn analyze_compression(dict_source: &DictSource, max_archives: usize) -> Result<()> {
    let stdout_dict = dict_source.load(SampleCategory::Stdout)?;
    let stderr_dict = dict_source.load(SampleCategory::Stderr)?;
    let meta_dict = dict_source.load(SampleCategory::Meta)?;

    if !matches!(dict_source, DictSource::NoDictionary)
        && stdout_dict.is_none()
        && stderr_dict.is_none()
        && meta_dict.is_none()
    {
        bail!("no dictionaries found");
    }

    let label = dict_source.label();

    let archives = find_archives()?;
    let archives: Vec<_> = archives.into_iter().take(max_archives).collect();

    eprintln!(
        "Analyzing {} archives (mode: {})...\n",
        archives.len(),
        label
    );

    let mut total_original: u64 = 0;
    let mut total_recompressed: u64 = 0;
    let mut total_uncompressed: u64 = 0;

    // Per-category stats: (uncompressed, in_archive, recompressed).
    let mut category_stats: HashMap<SampleCategory, (u64, u64, u64)> = HashMap::new();

    for archive_path in &archives {
        let file = match File::open(archive_path) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let reader = BufReader::new(file);
        let mut archive = match Archive::new(reader) {
            Ok(a) => a,
            Err(_) => continue,
        };

        let archive_dicts = ArchiveDicts::load(&mut archive);

        let entry_count = archive.entries().len();
        for i in 0..entry_count {
            let mut file = archive.get_by_index(i).expect("index is in bounds");

            let name = file.metadata().name().to_string();
            let Some(category) = SampleCategory::from_filename(&name) else {
                continue;
            };

            let original_compressed = file.metadata().compressed_size;

            let data = match read_out_entry_raw(&mut file, category, &archive_dicts) {
                Ok(d) => d,
                Err(_) => continue,
            };

            let uncompressed = data.len() as u64;

            // Recompress according to the chosen mode.
            let dict = match category {
                SampleCategory::Stdout => stdout_dict.as_deref(),
                SampleCategory::Stderr => stderr_dict.as_deref(),
                SampleCategory::Meta => meta_dict.as_deref(),
            };

            let recompressed = if let Some(dict_data) = dict {
                compress_with_dict(&data, dict_data, 3)
                    .map(|c| c.len() as u64)
                    .unwrap_or(original_compressed)
            } else {
                // No dictionary for this category: use plain zstd.
                zstd::encode_all(data.as_slice(), 3)
                    .map(|c| c.len() as u64)
                    .unwrap_or(original_compressed)
            };

            total_original += original_compressed;
            total_recompressed += recompressed;
            total_uncompressed += uncompressed;

            let (cat_uncomp, cat_orig, cat_recomp) = category_stats.entry(category).or_default();
            *cat_uncomp += uncompressed;
            *cat_orig += original_compressed;
            *cat_recomp += recompressed;
        }
    }

    // Print results.
    eprintln!("=== Overall Results ({}) ===", label);
    eprintln!("Total uncompressed:     {:>12} bytes", total_uncompressed);
    eprintln!("Total in archive:       {:>12} bytes", total_original);
    eprintln!(
        "Total recompressed:     {:>12} bytes ({})",
        total_recompressed, label
    );
    eprintln!(
        "Improvement:            {:>12.1}%",
        (1.0 - total_recompressed as f64 / total_original as f64) * 100.0
    );
    eprintln!(
        "Archive ratio:          {:>12.2}x",
        total_uncompressed as f64 / total_original as f64
    );
    eprintln!(
        "Recompressed ratio:     {:>12.2}x",
        total_uncompressed as f64 / total_recompressed as f64
    );

    eprintln!("\n=== Per-Category Results ({}) ===", label);
    for (category, (uncomp, orig, recomp)) in &category_stats {
        eprintln!("\n{:?}:", category);
        eprintln!("  Uncompressed:   {:>12} bytes", uncomp);
        eprintln!("  In archive:     {:>12} bytes", orig);
        eprintln!("  Recompressed:   {:>12} bytes ({})", recomp, label);
        eprintln!(
            "  Improvement:    {:>12.1}%",
            (1.0 - *recomp as f64 / *orig as f64) * 100.0
        );
        eprintln!("  Archive ratio:  {:>12.2}x", *uncomp as f64 / *orig as f64);
        eprintln!(
            "  Recomp ratio:   {:>12.2}x",
            *uncomp as f64 / *recomp as f64
        );
    }

    Ok(())
}

// ---
// Size sweep
// ---

/// Test different dictionary sizes to find optimal size.
fn size_sweep(max_samples: usize) -> Result<()> {
    let archives = find_archives()?;

    // Collect samples by category.
    let mut stdout_samples: Vec<Vec<u8>> = Vec::new();
    let mut stderr_samples: Vec<Vec<u8>> = Vec::new();

    eprintln!("Collecting samples from {} archives...", archives.len());

    for archive_path in &archives {
        let file = match File::open(archive_path) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let reader = BufReader::new(file);

        let mut archive = match Archive::new(reader) {
            Ok(a) => a,
            Err(_) => continue,
        };

        let dicts = ArchiveDicts::load(&mut archive);

        let entry_count = archive.entries().len();
        for i in 0..entry_count {
            let mut file = archive.get_by_index(i).expect("index is in bounds");

            let name = file.metadata().name().to_string();
            let Some(category) = SampleCategory::from_filename(&name) else {
                continue;
            };

            // Only collect stdout/stderr, skip meta (it doesn't benefit from dict).
            let samples = match category {
                SampleCategory::Stdout => &mut stdout_samples,
                SampleCategory::Stderr => &mut stderr_samples,
                SampleCategory::Meta => continue,
            };

            if samples.len() >= max_samples {
                continue;
            }

            let data = match read_out_entry_raw(&mut file, category, &dicts) {
                Ok(d) if !d.is_empty() && d.len() <= 100_000 => d,
                _ => continue,
            };

            samples.push(data);
        }

        if stdout_samples.len() >= max_samples && stderr_samples.len() >= max_samples {
            break;
        }
    }

    eprintln!(
        "Collected {} stdout samples, {} stderr samples\n",
        stdout_samples.len(),
        stderr_samples.len()
    );

    let dict_sizes = [512, 1024, 2048, 4096, 8192, 16384, 32768, 65536];

    for (name, samples) in [("Stdout", &stdout_samples), ("Stderr", &stderr_samples)] {
        if samples.is_empty() {
            continue;
        }

        eprintln!("=== {} ===", name);

        // Use a subset for testing compression.
        let test_samples: Vec<_> = samples.iter().take(2000).collect();
        let total_uncompressed: usize = test_samples.iter().map(|s| s.len()).sum();

        // Baseline: no dictionary.
        let baseline_compressed: usize = test_samples
            .iter()
            .map(|s| zstd::encode_all(s.as_slice(), 3).unwrap().len())
            .sum();

        eprintln!("Uncompressed:     {:>8} bytes", total_uncompressed);
        eprintln!(
            "No dict (zstd-3): {:>8} bytes ({:.2}x)",
            baseline_compressed,
            total_uncompressed as f64 / baseline_compressed as f64
        );
        eprintln!();

        for &size in &dict_sizes {
            let dict_data = match zstd::dict::from_samples(samples, size) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("Dict {:>6} bytes: failed to train: {}", size, e);
                    continue;
                }
            };
            let dict = zstd::dict::EncoderDictionary::copy(&dict_data, 3);

            let compressed: usize = test_samples
                .iter()
                .map(|s| {
                    let mut enc =
                        zstd::stream::Encoder::with_prepared_dictionary(Vec::new(), &dict).unwrap();
                    enc.write_all(s).unwrap();
                    enc.finish().unwrap().len()
                })
                .sum();

            eprintln!(
                "Dict {:>6} bytes: {:>8} bytes ({:.2}x) - {:.1}% better than no-dict",
                size,
                compressed,
                total_uncompressed as f64 / compressed as f64,
                (1.0 - compressed as f64 / baseline_compressed as f64) * 100.0
            );
        }

        eprintln!();
    }

    Ok(())
}

// ---
// Level sweep
// ---

/// Sweep compression levels to find the optimal level for each category.
///
/// Unlike `size_sweep` (which varies dictionary size at a fixed level), this
/// varies the compression level at a fixed dictionary. Includes meta, which
/// does not use a dictionary but still benefits from level tuning.
fn level_sweep(max_samples: usize, dict_source: &DictSource) -> Result<()> {
    let archives = find_archives()?;

    let all_categories = [
        SampleCategory::Stdout,
        SampleCategory::Stderr,
        SampleCategory::Meta,
    ];

    // Collect samples by category (including meta).
    let mut samples_by_category: HashMap<SampleCategory, Vec<Vec<u8>>> = HashMap::new();

    eprintln!("Collecting samples from {} archives...", archives.len());

    for archive_path in &archives {
        let file = match File::open(archive_path) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let reader = BufReader::new(file);

        let mut archive = match Archive::new(reader) {
            Ok(a) => a,
            Err(_) => continue,
        };

        let dicts = ArchiveDicts::load(&mut archive);

        let entry_count = archive.entries().len();
        for i in 0..entry_count {
            let mut file = archive.get_by_index(i).expect("index is in bounds");

            let name = file.metadata().name().to_string();
            let Some(category) = SampleCategory::from_filename(&name) else {
                continue;
            };

            let samples = samples_by_category.entry(category).or_default();
            if samples.len() >= max_samples {
                continue;
            }

            let data = match read_out_entry_raw(&mut file, category, &dicts) {
                Ok(d) if !d.is_empty() && d.len() <= 100_000 => d,
                _ => continue,
            };

            samples.push(data);
        }

        let all_full = all_categories.iter().all(|cat| {
            samples_by_category
                .get(cat)
                .is_some_and(|s| s.len() >= max_samples)
        });
        if all_full {
            break;
        }
    }

    for &category in &all_categories {
        let count = samples_by_category.get(&category).map_or(0, Vec::len);
        eprintln!("  {:?}: {} samples", category, count);
    }
    eprintln!();

    let levels = [1, 3, 5, 7, 9];

    for &category in &all_categories {
        let Some(samples) = samples_by_category.get(&category) else {
            continue;
        };
        if samples.is_empty() {
            continue;
        }

        // Use a subset for testing compression.
        let test_samples: Vec<_> = samples.iter().take(2000).collect();
        let total_uncompressed: usize = test_samples.iter().map(|s| s.len()).sum();

        // Load dictionary for this category.
        let dict_data = dict_source.load(category)?;
        let has_dict = dict_data.is_some();

        if has_dict {
            eprintln!("=== {:?} ({}) ===", category, dict_source.label());
        } else {
            eprintln!("=== {:?} (no dictionary) ===", category);
        }
        eprintln!(
            "Uncompressed: {:>8} bytes ({} samples)\n",
            total_uncompressed,
            test_samples.len()
        );

        if has_dict {
            eprintln!(
                "{:>5}  {:>10} {:>7}  {:>10} {:>7} {:>7}",
                "Level", "No-dict", "Ratio", "Dict", "Ratio", "Improv"
            );
            eprintln!("{}", "-".repeat(55));
        } else {
            eprintln!("{:>5}  {:>10} {:>7}", "Level", "Compressed", "Ratio");
            eprintln!("{}", "-".repeat(25));
        }

        for &level in &levels {
            // No-dict compression at this level.
            let no_dict_compressed: usize = test_samples
                .iter()
                .map(|s| zstd::encode_all(s.as_slice(), level).unwrap().len())
                .sum();
            let no_dict_ratio = total_uncompressed as f64 / no_dict_compressed as f64;

            if let Some(ref dict_bytes) = dict_data {
                // Dict compression at this level.
                let dict = zstd::dict::EncoderDictionary::copy(dict_bytes, level);
                let dict_compressed: usize = test_samples
                    .iter()
                    .map(|s| {
                        let mut enc =
                            zstd::stream::Encoder::with_prepared_dictionary(Vec::new(), &dict)
                                .unwrap();
                        enc.write_all(s).unwrap();
                        enc.finish().unwrap().len()
                    })
                    .sum();
                let dict_ratio = total_uncompressed as f64 / dict_compressed as f64;
                let improvement =
                    (1.0 - dict_compressed as f64 / no_dict_compressed as f64) * 100.0;

                eprintln!(
                    "{:>5}  {:>10} {:>6.2}x  {:>10} {:>6.2}x {:>6.1}%",
                    level,
                    no_dict_compressed,
                    no_dict_ratio,
                    dict_compressed,
                    dict_ratio,
                    improvement
                );
            } else {
                eprintln!(
                    "{:>5}  {:>10} {:>6.2}x",
                    level, no_dict_compressed, no_dict_ratio
                );
            }
        }

        eprintln!();
    }

    Ok(())
}

// ---
// Per-project analysis
// ---

/// Analyze compression per-project to see which benefit most.
fn analyze_per_project(dict_source: &DictSource) -> Result<()> {
    let state_dir = find_state_dir()?;
    let projects_dir = state_dir.join("projects");

    let stdout_dict = dict_source.load(SampleCategory::Stdout)?;
    let stderr_dict = dict_source.load(SampleCategory::Stderr)?;

    if !matches!(dict_source, DictSource::NoDictionary)
        && stdout_dict.is_none()
        && stderr_dict.is_none()
    {
        bail!("no stdout or stderr dictionary found");
    }

    let label = dict_source.label();

    let mut results: Vec<(String, u64, u64, u64, usize)> = Vec::new();

    for entry in fs::read_dir(&projects_dir)? {
        let entry = entry?;
        let project_path = Utf8PathBuf::try_from(entry.path()).ok();
        let Some(project_path) = project_path else {
            continue;
        };

        let project_name = project_path.file_name().unwrap_or("unknown");

        // Skip tmp fixtures.
        if project_name.contains("_stmp") {
            continue;
        }

        // Extract short name from encoded path.
        let short_name = project_name
            .rsplit("_s")
            .next()
            .unwrap_or(project_name)
            .to_string();

        let archives: Vec<_> = walkdir::WalkDir::new(&project_path)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().file_name() == Some(std::ffi::OsStr::new("store.zip")))
            .filter_map(|e| Utf8PathBuf::try_from(e.path().to_path_buf()).ok())
            .collect();

        let mut total_uncompressed: u64 = 0;
        let mut total_original: u64 = 0;
        let mut total_recompressed: u64 = 0;
        let mut test_count: usize = 0;

        for archive_path in archives {
            let file = match File::open(&archive_path) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let reader = BufReader::new(file);
            let mut archive = match Archive::new(reader) {
                Ok(a) => a,
                Err(_) => continue,
            };

            let archive_dicts = ArchiveDicts::load(&mut archive);

            let entry_count = archive.entries().len();
            for i in 0..entry_count {
                let mut file = archive.get_by_index(i).expect("index is in bounds");

                let name = file.metadata().name().to_string();

                // Only analyze stdout/stderr (skip meta).
                if !name.starts_with("out/") {
                    continue;
                }

                let category = SampleCategory::from_filename(&name);

                let original_compressed = file.metadata().compressed_size;

                let data = if let Some(cat) = category {
                    match read_out_entry_raw(&mut file, cat, &archive_dicts) {
                        Ok(d) => d,
                        Err(_) => continue,
                    }
                } else {
                    let mut buf = Vec::new();
                    if file
                        .read()
                        .ok()
                        .and_then(|mut r| r.read_to_end(&mut buf).ok())
                        .is_none()
                    {
                        continue;
                    }
                    buf
                };

                let uncompressed = data.len() as u64;

                if name.ends_with("-stdout") || name.ends_with("-combined") {
                    test_count += 1;
                }

                // Recompress using the same logic as analyze_compression.
                let dict = match category {
                    Some(SampleCategory::Stdout) => stdout_dict.as_deref(),
                    Some(SampleCategory::Stderr) => stderr_dict.as_deref(),
                    _ => None,
                };

                let recompressed = if let Some(dict_data) = dict {
                    compress_with_dict(&data, dict_data, 3)
                        .map(|c| c.len() as u64)
                        .unwrap_or(original_compressed)
                } else {
                    // No dictionary for this category: use plain zstd.
                    zstd::encode_all(data.as_slice(), 3)
                        .map(|c| c.len() as u64)
                        .unwrap_or(original_compressed)
                };

                total_uncompressed += uncompressed;
                total_original += original_compressed;
                total_recompressed += recompressed;
            }
        }

        if test_count > 0 {
            results.push((
                short_name,
                total_uncompressed,
                total_original,
                total_recompressed,
                test_count,
            ));
        }
    }

    // Sort by test count descending.
    results.sort_by(|a, b| b.4.cmp(&a.4));

    eprintln!(
        "{:<15} {:>6} {:>10} {:>10} {:>10} {:>7}",
        "Project", "Tests", "Uncomp", "InArchive", label, "Improv"
    );
    eprintln!("{}", "-".repeat(70));

    for (name, uncomp, orig, recomp, count) in &results {
        let improvement = if *orig > 0 {
            (1.0 - *recomp as f64 / *orig as f64) * 100.0
        } else {
            0.0
        };
        eprintln!(
            "{:<15} {:>6} {:>10} {:>10} {:>10} {:>6.1}%",
            name, count, uncomp, orig, recomp, improvement
        );
    }

    // Summary for large repos (300+ tests).
    let large_repos: Vec<_> = results.iter().filter(|r| r.4 >= 300).collect();
    if !large_repos.is_empty() {
        let total_orig: u64 = large_repos.iter().map(|r| r.2).sum();
        let total_recomp: u64 = large_repos.iter().map(|r| r.3).sum();
        let total_tests: usize = large_repos.iter().map(|r| r.4).sum();
        eprintln!("\n=== Large repos (300+ tests, {}) ===", label);
        eprintln!("Total tests: {}", total_tests);
        eprintln!("In archive: {} bytes", total_orig);
        eprintln!("Recompressed: {} bytes ({})", total_recomp, label);
        eprintln!(
            "Improvement: {:.1}%",
            (1.0 - total_recomp as f64 / total_orig as f64) * 100.0
        );
    }

    Ok(())
}

// ---
// CDF data dump
// ---

/// A single test output entry's sizes for CDF output.
struct CdfEntry {
    category: SampleCategory,
    uncompressed: u64,
    dict_compressed: u64,
    plain_compressed: u64,
}

/// Dump per-entry compression sizes for CDF plotting.
///
/// For each test output entry (stdout, stderr, combined) across archives,
/// computes the uncompressed size, dict-compressed size, and plain
/// zstd-compressed size. Outputs to stdout in a format suitable for gnuplot's
/// `smooth cnormal`.
///
/// Output format (space-separated):
///
///   category uncompressed dict_compressed plain_compressed
///
/// where category is `stdout` or `stderr`, and sizes are in bytes.
fn dump_cdf(dict_dir: Option<&Utf8Path>, max_archives: usize) -> Result<()> {
    // Always compare dict vs plain: load dictionaries from disk or embedded.
    let source = match dict_dir {
        Some(dir) => DictSource::Directory(dir.to_owned()),
        None => DictSource::Embedded,
    };

    let stdout_dict = source.load(SampleCategory::Stdout)?;
    let stderr_dict = source.load(SampleCategory::Stderr)?;

    let archives = find_archives()?;
    let archives: Vec<_> = archives.into_iter().take(max_archives).collect();

    eprintln!(
        "Processing {} archives ({})...",
        archives.len(),
        source.label()
    );

    let mut entries: Vec<CdfEntry> = Vec::new();

    for archive_path in &archives {
        let file = match File::open(archive_path) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let reader = BufReader::new(file);
        let mut archive = match Archive::new(reader) {
            Ok(a) => a,
            Err(_) => continue,
        };

        let archive_dicts = ArchiveDicts::load(&mut archive);

        let entry_count = archive.entries().len();
        for i in 0..entry_count {
            let mut file = archive.get_by_index(i).expect("index is in bounds");

            let name = file.metadata().name().to_string();

            // Only test output entries.
            if !name.starts_with("out/") {
                continue;
            }

            let Some(category) = SampleCategory::from_filename(&name) else {
                continue;
            };

            let data = match read_out_entry_raw(&mut file, category, &archive_dicts) {
                Ok(d) if !d.is_empty() => d,
                _ => continue,
            };

            let uncompressed = data.len() as u64;

            // Compress with dictionary.
            let dict = match category {
                SampleCategory::Stdout => stdout_dict.as_deref(),
                SampleCategory::Stderr => stderr_dict.as_deref(),
                SampleCategory::Meta => None,
            };

            let dict_compressed = if let Some(dict_data) = dict {
                compress_with_dict(&data, dict_data, 3)
                    .map(|c| c.len() as u64)
                    .unwrap_or(uncompressed)
            } else {
                zstd::encode_all(data.as_slice(), 3)
                    .map(|c| c.len() as u64)
                    .unwrap_or(uncompressed)
            };

            // Compress without dictionary (plain zstd level 3).
            let plain_compressed = zstd::encode_all(data.as_slice(), 3)
                .map(|c| c.len() as u64)
                .unwrap_or(uncompressed);

            entries.push(CdfEntry {
                category,
                uncompressed,
                dict_compressed,
                plain_compressed,
            });
        }
    }

    let n = entries.len();
    eprintln!("Collected {} entries", n);

    if n == 0 {
        bail!("no test output entries found");
    }

    // Print summary statistics to stderr, overall and per-category.
    print_cdf_summary("All", &entries);

    let stdout_entries: Vec<_> = entries
        .iter()
        .filter(|e| matches!(e.category, SampleCategory::Stdout))
        .collect();
    let stderr_entries: Vec<_> = entries
        .iter()
        .filter(|e| matches!(e.category, SampleCategory::Stderr))
        .collect();
    print_cdf_summary("Stdout", &stdout_entries);
    print_cdf_summary("Stderr", &stderr_entries);

    // Output raw sizes to stdout for gnuplot.
    let stdout = std::io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    writeln!(
        out,
        "# category uncompressed dict_compressed plain_compressed"
    )?;
    for entry in &entries {
        let cat_label = match entry.category {
            SampleCategory::Stdout => "stdout",
            SampleCategory::Stderr => "stderr",
            SampleCategory::Meta => "meta",
        };
        writeln!(
            out,
            "{} {} {} {}",
            cat_label, entry.uncompressed, entry.dict_compressed, entry.plain_compressed,
        )?;
    }

    Ok(())
}

/// Print percentile summary for a set of CDF entries.
fn print_cdf_summary(label: &str, entries: &[impl std::borrow::Borrow<CdfEntry>]) {
    let n = entries.len();
    if n == 0 {
        eprintln!("{}: (no entries)", label);
        return;
    }

    let mut uncompressed: Vec<u64> = entries.iter().map(|e| e.borrow().uncompressed).collect();
    let mut dict: Vec<u64> = entries.iter().map(|e| e.borrow().dict_compressed).collect();
    let mut plain: Vec<u64> = entries
        .iter()
        .map(|e| e.borrow().plain_compressed)
        .collect();
    uncompressed.sort_unstable();
    dict.sort_unstable();
    plain.sort_unstable();

    eprintln!("{} ({} entries):", label, n);
    eprintln!(
        "  Uncompressed: p50 {} B, p75 {} B, p95 {} B, p99 {} B, max {} B",
        uncompressed[n / 2],
        uncompressed[n * 75 / 100],
        uncompressed[n * 95 / 100],
        uncompressed[n * 99 / 100],
        uncompressed[n - 1],
    );
    eprintln!(
        "  Dict zstd-3:  p50 {} B, p75 {} B, p95 {} B, p99 {} B, max {} B",
        dict[n / 2],
        dict[n * 75 / 100],
        dict[n * 95 / 100],
        dict[n * 99 / 100],
        dict[n - 1],
    );
    eprintln!(
        "  Plain zstd-3: p50 {} B, p75 {} B, p95 {} B, p99 {} B, max {} B",
        plain[n / 2],
        plain[n * 75 / 100],
        plain[n * 95 / 100],
        plain[n * 99 / 100],
        plain[n - 1],
    );
}
