//! `gen-ts`: generates the desktop UI's view of the canonical telemetry model (D-023).
//!
//! Outputs, relative to the repository root:
//! - `apps/desktop/src/generated/channels.ts`: every `ChannelDef` of `telemetry_model::registry`, the `ch` id
//!   constants, lookup maps, `TELEMETRY_SCHEMA_VERSION` and the enum value tables, all read from the compiled
//!   Rust model ([`channels_ts`]);
//! - `apps/desktop/src/generated/golden/`: TLV blocks and UiBatch envelopes with `manifest.json` describing
//!   the expected decode result of each ([`golden`]).
//!
//! Output is deterministic: stable order, LF line endings, UTF-8 without BOM. The
//! `committed_files_are_up_to_date` test regenerates everything in memory and fails when the committed files
//! differ, so the TypeScript decoders can never be tested against vectors the Rust codec no longer produces.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub mod channels_ts;
pub mod golden;
pub mod uibatch;
pub mod variants;

/// Path of the generated channel table, relative to the repository root.
pub const CHANNELS_TS: &str = "apps/desktop/src/generated/channels.ts";

/// One generated file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputFile {
    /// `/`-separated path relative to the repository root.
    pub path: String,
    pub contents: Vec<u8>,
}

/// Every generated file, in a stable order.
pub fn generate() -> Vec<OutputFile> {
    let mut files = vec![OutputFile {
        path: CHANNELS_TS.to_owned(),
        contents: channels_ts::channels_ts().into_bytes(),
    }];
    files.extend(golden::golden_files());
    for file in files.iter().filter(|f| !f.path.ends_with(".bin")) {
        let text = std::str::from_utf8(&file.contents)
            .unwrap_or_else(|_| panic!("{} must be UTF-8", file.path));
        assert!(
            !text.contains('\r'),
            "{} must use LF line endings",
            file.path
        );
        assert!(
            !text.starts_with('\u{feff}'),
            "{} must not start with a BOM",
            file.path
        );
    }
    files
}

/// The repository root (this crate lives in `tools/gen-ts`).
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn native(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(root.to_path_buf(), |p, part| p.join(part))
}

/// Files below the golden directory on disk, as `/`-separated paths relative to the repository root.
fn golden_files_on_disk(root: &Path) -> io::Result<Vec<String>> {
    let mut out = Vec::new();
    let mut pending = vec![golden::GOLDEN_DIR.to_owned()];
    while let Some(dir) = pending.pop() {
        let entries = match fs::read_dir(native(root, &dir)) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = format!("{dir}/{name}");
            if entry.file_type()?.is_dir() {
                pending.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Differences between `files` and the repository: missing or outdated generated files, and files in the
/// golden directory that the generator no longer produces. Empty when everything is up to date.
pub fn stale(root: &Path, files: &[OutputFile]) -> io::Result<Vec<String>> {
    let mut problems = Vec::new();
    for file in files {
        match fs::read(native(root, &file.path)) {
            Ok(bytes) if bytes == file.contents => {}
            Ok(_) => problems.push(format!("outdated {}", file.path)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                problems.push(format!("missing {}", file.path));
            }
            Err(e) => return Err(e),
        }
    }
    for path in golden_files_on_disk(root)? {
        if !files.iter().any(|f| f.path == path) {
            problems.push(format!("unexpected {path}"));
        }
    }
    Ok(problems)
}

/// Writes every file that changed and deletes golden files the generator no longer produces. Returns the
/// number of files written or deleted.
pub fn write(root: &Path, files: &[OutputFile]) -> io::Result<usize> {
    let mut changed = 0;
    for file in files {
        let path = native(root, &file.path);
        if fs::read(&path).is_ok_and(|bytes| bytes == file.contents) {
            continue;
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, &file.contents)?;
        changed += 1;
    }
    for path in golden_files_on_disk(root)? {
        if !files.iter().any(|f| f.path == path) {
            fs::remove_file(native(root, &path))?;
            changed += 1;
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_is_deterministic() {
        assert_eq!(generate(), generate());
    }

    #[test]
    fn golden_set_stays_small() {
        let golden_bytes: usize = generate()
            .iter()
            .filter(|f| f.path.starts_with(golden::GOLDEN_DIR))
            .map(|f| f.contents.len())
            .sum();
        assert!(
            golden_bytes < 300 * 1024,
            "golden set is {golden_bytes} bytes; keep it under 300 KiB"
        );
    }

    #[test]
    fn committed_files_are_up_to_date() {
        let problems = stale(&repo_root(), &generate()).expect("read generated files");
        assert!(
            problems.is_empty(),
            "generated files are stale ({}); run cargo run -p gen-ts",
            problems.join(", ")
        );
    }
}
