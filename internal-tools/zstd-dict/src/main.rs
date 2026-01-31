// Copyright (c) The nextest Contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Train and analyze zstd dictionaries for nextest record archives.
//!
//! This tool:
//! 1. Scans ~/.cache/nextest for existing store.zip archives
//! 2. Extracts stdout/stderr samples from them
//! 3. Trains separate zstd dictionaries for stdout and stderr
//! 4. Analyzes compression improvement across archives

use camino::{Utf8Path, Utf8PathBuf};
use clap::{Parser, Subcommand};
use color_eyre::eyre::{Context, Result, bail};
use etcetera::BaseStrategy;
use nextest_runner::record::OutputDict;
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufReader, Read, Write},
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
        /// Directory containing trained dictionaries (uses embedded if not specified).
        #[arg(short, long)]
        dict_dir: Option<Utf8PathBuf>,

        /// Maximum number of archives to analyze.
        #[arg(short, long, default_value = "50")]
        max_archives: usize,
    },

    /// Test different dictionary sizes to find optimal size.
    SizeSweep {
        /// Maximum number of samples to use for training.
        #[arg(short, long, default_value = "10000")]
        max_samples: usize,
    },

    /// Analyze compression per-project.
    PerProject {
        /// Directory containing trained dictionaries (uses embedded if not specified).
        #[arg(short, long)]
        dict_dir: Option<Utf8PathBuf>,
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
        } => analyze_compression(dict_dir.as_deref(), max_archives),
        Command::SizeSweep { max_samples } => size_sweep(max_samples),
        Command::PerProject { dict_dir } => analyze_per_project(dict_dir.as_deref()),
    }
}

// ---
// Cache directory and archive discovery
// ---

/// Find the nextest cache directory.
fn find_cache_dir() -> Result<Utf8PathBuf> {
    let base = etcetera::base_strategy::choose_native_strategy()
        .wrap_err("failed to determine base directories")?;
    let cache_dir = base.cache_dir();
    let nextest_cache = Utf8PathBuf::try_from(cache_dir.join("nextest"))
        .wrap_err("cache path is not valid UTF-8")?;
    Ok(nextest_cache)
}

/// Find all store.zip files in the cache.
fn find_archives() -> Result<Vec<Utf8PathBuf>> {
    let cache_dir = find_cache_dir()?;
    let projects_dir = cache_dir.join("projects");

    if !projects_dir.exists() {
        bail!("no nextest cache found at {}", projects_dir);
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

        let mut archive = match zip::ZipArchive::new(reader) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("Warning: failed to read {}: {}", archive_path, e);
                continue;
            }
        };

        for i in 0..archive.len() {
            let mut entry = match archive.by_index(i) {
                Ok(e) => e,
                Err(_) => continue,
            };

            let name = entry.name().to_string();
            let Some(category) = SampleCategory::from_filename(&name) else {
                continue;
            };

            let samples_for_category = samples.entry(category).or_default();
            if samples_for_category.len() >= max_samples {
                continue;
            }

            let mut data = Vec::new();
            if entry.read_to_end(&mut data).is_ok() && !data.is_empty() {
                // Skip very large samples (>1MB) as they skew the dictionary.
                if data.len() <= 1_000_000 {
                    samples_for_category.push(data);
                }
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
    let mut source = zip::ZipArchive::new(reader)?;

    let original_size = fs::metadata(archive_path)?.len();

    // Create output archive.
    let output = output_path.map(|p| p.to_owned()).unwrap_or_else(|| {
        let stem = archive_path.file_stem().unwrap_or("archive");
        archive_path.with_file_name(format!("{}-recompressed.zip", stem))
    });

    let output_file = File::create(&output)?;
    let mut dest = zip::ZipWriter::new(output_file);

    let mut stats = CompressionStats::default();

    // Process each entry.
    for i in 0..source.len() {
        let mut entry = source.by_index(i)?;
        let name = entry.name().to_string();
        let category = SampleCategory::from_filename(&name);

        // Read the decompressed data.
        let mut data = Vec::new();
        entry.read_to_end(&mut data)?;

        let original_compressed = entry.compressed_size();
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
                let options = zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored);
                dest.start_file(&name, options)?;
                dest.write_all(&compressed)?;

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

/// Write a file with standard zstd compression.
fn write_zstd(dest: &mut zip::ZipWriter<File>, name: &str, data: &[u8]) -> Result<(u64, String)> {
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Zstd)
        .compression_level(Some(3));
    dest.start_file(name, options)?;
    dest.write_all(data)?;

    // Estimate compressed size (we don't have exact size until finish).
    // This is approximate but good enough for comparison.
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
fn analyze_compression(dict_dir: Option<&Utf8Path>, max_archives: usize) -> Result<()> {
    // Load dictionaries (from disk or embedded).
    let stdout_dict = load_dictionary(dict_dir, SampleCategory::Stdout)?;
    let stderr_dict = load_dictionary(dict_dir, SampleCategory::Stderr)?;
    let meta_dict = load_dictionary(dict_dir, SampleCategory::Meta)?;

    if stdout_dict.is_none() && stderr_dict.is_none() && meta_dict.is_none() {
        bail!("no dictionaries found");
    }

    let archives = find_archives()?;
    let archives: Vec<_> = archives.into_iter().take(max_archives).collect();

    eprintln!("Analyzing {} archives...\n", archives.len());

    let mut total_original: u64 = 0;
    let mut total_with_dict: u64 = 0;
    let mut total_uncompressed: u64 = 0;

    // Per-category stats.
    let mut category_stats: HashMap<SampleCategory, (u64, u64, u64)> = HashMap::new();

    for archive_path in &archives {
        let file = match File::open(archive_path) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let reader = BufReader::new(file);
        let mut archive = match zip::ZipArchive::new(reader) {
            Ok(a) => a,
            Err(_) => continue,
        };

        for i in 0..archive.len() {
            let mut entry = match archive.by_index(i) {
                Ok(e) => e,
                Err(_) => continue,
            };

            let name = entry.name().to_string();
            let Some(category) = SampleCategory::from_filename(&name) else {
                continue;
            };

            let mut data = Vec::new();
            if entry.read_to_end(&mut data).is_err() {
                continue;
            }

            let original_compressed = entry.compressed_size();
            let uncompressed = data.len() as u64;

            // Compress with dictionary.
            let dict = match category {
                SampleCategory::Stdout => stdout_dict.as_deref(),
                SampleCategory::Stderr => stderr_dict.as_deref(),
                SampleCategory::Meta => meta_dict.as_deref(),
            };

            let dict_compressed = if let Some(dict_data) = dict {
                compress_with_dict(&data, dict_data, 3)
                    .map(|c| c.len() as u64)
                    .unwrap_or(original_compressed)
            } else {
                original_compressed
            };

            total_original += original_compressed;
            total_with_dict += dict_compressed;
            total_uncompressed += uncompressed;

            let (cat_uncomp, cat_orig, cat_dict) = category_stats.entry(category).or_default();
            *cat_uncomp += uncompressed;
            *cat_orig += original_compressed;
            *cat_dict += dict_compressed;
        }
    }

    // Print results.
    eprintln!("=== Overall Results ===");
    eprintln!("Total uncompressed:     {:>12} bytes", total_uncompressed);
    eprintln!("Total original zstd:    {:>12} bytes", total_original);
    eprintln!("Total with dictionary:  {:>12} bytes", total_with_dict);
    eprintln!(
        "Improvement:            {:>12.1}%",
        (1.0 - total_with_dict as f64 / total_original as f64) * 100.0
    );
    eprintln!(
        "Original ratio:         {:>12.2}x",
        total_uncompressed as f64 / total_original as f64
    );
    eprintln!(
        "Dictionary ratio:       {:>12.2}x",
        total_uncompressed as f64 / total_with_dict as f64
    );

    eprintln!("\n=== Per-Category Results ===");
    for (category, (uncomp, orig, dict)) in &category_stats {
        eprintln!("\n{:?}:", category);
        eprintln!("  Uncompressed:   {:>12} bytes", uncomp);
        eprintln!("  Original zstd:  {:>12} bytes", orig);
        eprintln!("  With dict:      {:>12} bytes", dict);
        eprintln!(
            "  Improvement:    {:>12.1}%",
            (1.0 - *dict as f64 / *orig as f64) * 100.0
        );
        eprintln!("  Original ratio: {:>12.2}x", *uncomp as f64 / *orig as f64);
        eprintln!("  Dict ratio:     {:>12.2}x", *uncomp as f64 / *dict as f64);
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

        let mut archive = match zip::ZipArchive::new(reader) {
            Ok(a) => a,
            Err(_) => continue,
        };

        for i in 0..archive.len() {
            let mut entry = match archive.by_index(i) {
                Ok(e) => e,
                Err(_) => continue,
            };

            let name = entry.name().to_string();
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

            let mut data = Vec::new();
            if entry.read_to_end(&mut data).is_ok() && !data.is_empty() && data.len() <= 100_000 {
                samples.push(data);
            }
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
// Per-project analysis
// ---

/// Analyze compression per-project to see which benefit most.
fn analyze_per_project(dict_dir: Option<&Utf8Path>) -> Result<()> {
    let cache_dir = find_cache_dir()?;
    let projects_dir = cache_dir.join("projects");

    // Load dictionaries (from disk or embedded).
    let stdout_dict = load_dictionary(dict_dir, SampleCategory::Stdout)?;
    let stderr_dict = load_dictionary(dict_dir, SampleCategory::Stderr)?;

    if stdout_dict.is_none() {
        bail!("no stdout dictionary found");
    }

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
        let mut total_dict: u64 = 0;
        let mut test_count: usize = 0;

        for archive_path in archives {
            let file = match File::open(&archive_path) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let reader = BufReader::new(file);
            let mut archive = match zip::ZipArchive::new(reader) {
                Ok(a) => a,
                Err(_) => continue,
            };

            for i in 0..archive.len() {
                let mut entry = match archive.by_index(i) {
                    Ok(e) => e,
                    Err(_) => continue,
                };

                let name = entry.name().to_string();

                // Only analyze stdout/stderr (skip meta).
                if !name.starts_with("out/") {
                    continue;
                }

                let mut data = Vec::new();
                if entry.read_to_end(&mut data).is_err() {
                    continue;
                }

                let original_compressed = entry.compressed_size();
                let uncompressed = data.len() as u64;

                let dict = if name.ends_with("-stdout") {
                    test_count += 1;
                    stdout_dict.as_deref()
                } else if name.ends_with("-stderr") {
                    stderr_dict.as_deref()
                } else {
                    None
                };

                let dict_compressed = if let Some(dict_data) = dict {
                    compress_with_dict(&data, dict_data, 3)
                        .map(|c| c.len() as u64)
                        .unwrap_or(original_compressed)
                } else {
                    original_compressed
                };

                total_uncompressed += uncompressed;
                total_original += original_compressed;
                total_dict += dict_compressed;
            }
        }

        if test_count > 0 {
            results.push((
                short_name,
                total_uncompressed,
                total_original,
                total_dict,
                test_count,
            ));
        }
    }

    // Sort by test count descending.
    results.sort_by(|a, b| b.4.cmp(&a.4));

    eprintln!(
        "{:<15} {:>6} {:>10} {:>10} {:>10} {:>7}",
        "Project", "Tests", "Uncomp", "Original", "WithDict", "Improv"
    );
    eprintln!("{}", "-".repeat(70));

    for (name, uncomp, orig, dict, count) in &results {
        let improvement = if *orig > 0 {
            (1.0 - *dict as f64 / *orig as f64) * 100.0
        } else {
            0.0
        };
        eprintln!(
            "{:<15} {:>6} {:>10} {:>10} {:>10} {:>6.1}%",
            name, count, uncomp, orig, dict, improvement
        );
    }

    // Summary for large repos (300+ tests).
    let large_repos: Vec<_> = results.iter().filter(|r| r.4 >= 300).collect();
    if !large_repos.is_empty() {
        let total_orig: u64 = large_repos.iter().map(|r| r.2).sum();
        let total_dict: u64 = large_repos.iter().map(|r| r.3).sum();
        let total_tests: usize = large_repos.iter().map(|r| r.4).sum();
        eprintln!("\n=== Large repos (300+ tests) ===");
        eprintln!("Total tests: {}", total_tests);
        eprintln!("Original: {} bytes", total_orig);
        eprintln!("With dict: {} bytes", total_dict);
        eprintln!(
            "Improvement: {:.1}%",
            (1.0 - total_dict as f64 / total_orig as f64) * 100.0
        );
    }

    Ok(())
}
