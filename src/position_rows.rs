//! Per-position compiler rows (carrick#2405, move 2 of the fact layer).
//!
//! After a service's analysis drops its sidecar, a fresh record-only sidecar
//! walks the service's source files and writes, for every call, member
//! access, comparison and expression identifier, what the compiler knows
//! there: the symbol, the declaration's file and line, the declaring package
//! and the type text. The walk is the sidecar's (`src/sidecar/src/position-rows.ts`).
//!
//! Record-only: nothing in the scan reads the rows yet. The walk runs only
//! with [`POSITION_ROWS_ENV`] set to `1` and a local store directory
//! ([`CACHE_DIR_ENV`]), and writes `<store>/positions/<repo>__<service>.jsonl.gz`
//! beside the blobs. Both blob listings read top-level `*.json` only, so the
//! subdirectory is never read as a blob. A walk that dies or misses its
//! deadline loses that service's rows and says so in the run log, and
//! nothing else: it is never a scan-health entry, a finding or a stop.
//!
//! The file format is pinned here by serde types both sides read
//! (`tests/fixtures/position-rows/api.expected.jsonl`). seams-engineer owns it
//! from move 3; a change to it is a seam change.
//!
//! The file is gzip JSON lines. Line 1 is the [`Header`]. Then one
//! [`FileRows`] line per source file, in request order. A row's key is
//! `(start, end, kind)`: TypeScript `getStart()`/`getEnd()` of the node, in
//! UTF-16 code units from the start of the file, which is what
//! [`crate::utils::sidecar_position`] gives for an SWC span.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::debug;

use crate::cloud_storage::{CACHE_DIR_ENV, blob_stem};
use crate::services::type_sidecar::{PositionRowsCounts, SidecarRecipe};

/// Set to `1`, with a local store directory, to write the rows.
pub const POSITION_ROWS_ENV: &str = "CARRICK_POSITION_ROWS";

/// The store subdirectory the rows files go in.
pub const POSITIONS_DIR: &str = "positions";

/// The format version the header carries.
pub const FORMAT_VERSION: u32 = 1;

/// The type is `any`, declared or inferred. Never set with [`FLAG_ERROR_TYPE`].
pub const FLAG_ANY: u8 = 1;
/// The type is the compiler's error type: something did not resolve.
pub const FLAG_ERROR_TYPE: u8 = 2;
/// The checker threw at this node; the row holds nothing else.
pub const FLAG_CHECKER_THREW: u8 = 4;
/// The declaration is outside the checkout, not the default library and in
/// no installed package, so `decl_file` is `null`.
pub const FLAG_DECLARATION_UNNAMED: u8 = 8;

/// Line 1 of a rows file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Header {
    pub v: u32,
    /// The compiler version the rows were read with.
    pub typescript: String,
    pub service: String,
    /// How many [`FileRows`] lines follow.
    pub files: usize,
}

/// One source file's rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileRows {
    /// Relative to the scanned repo's root, with `/` separators.
    pub f: String,
    /// sha256 hex of the file's bytes on disk, BOM included. Absent only for
    /// a file that could not be read. A reader whose bytes hash otherwise
    /// holds stale rows, and that is a hole, never a guess.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub h: Option<String>,
    /// Why the file has no rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip: Option<Skip>,
    pub r: Vec<Row>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Skip {
    Unreadable,
    NotInProgram,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A call, `new` or tagged template: the callee's symbol, the result's type.
    Call,
    /// A property or element access: the name's or argument's symbol.
    Member,
    /// `=== !== == != < > <= >= instanceof in`: the left operand.
    Compare,
    /// An identifier in expression position.
    Ident,
}

/// `[kind, start, end, symbol, decl_file, decl_line, decl_package, type_text, flags]`.
///
/// `decl_file` is a repo-relative path (`node_modules/...` included),
/// `lib:<file>` for the compiler's default library, `pkg:<name>/<path>` for
/// an installed package outside the repo root, or `null` with
/// [`FLAG_DECLARATION_UNNAMED`]. `decl_line` is 1-based.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row(
    pub Kind,
    pub u32,
    pub u32,
    pub Option<String>,
    pub Option<String>,
    pub Option<u32>,
    pub Option<String>,
    pub Option<String>,
    pub u8,
);

impl Row {
    pub fn kind(&self) -> Kind {
        self.0
    }
    pub fn start(&self) -> u32 {
        self.1
    }
    pub fn end(&self) -> u32 {
        self.2
    }
    pub fn symbol(&self) -> Option<&str> {
        self.3.as_deref()
    }
    pub fn flags(&self) -> u8 {
        self.8
    }
}

/// A rows file, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowsFile {
    pub header: Header,
    pub files: Vec<FileRows>,
}

/// Read a rows file from gzip JSON lines. Nothing in a scan calls this yet:
/// it is the format's reader, for the tests that pin it and for move 3.
#[allow(dead_code)]
pub fn read_rows_file(path: &Path) -> Result<RowsFile, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    read_rows(BufReader::new(flate2::read::GzDecoder::new(file)))
}

/// [`read_rows_file`] over decompressed lines.
#[allow(dead_code)]
pub fn read_rows(reader: impl BufRead) -> Result<RowsFile, String> {
    let mut lines = reader.lines();
    let header: Header = match lines.next() {
        Some(line) => serde_json::from_str(&line.map_err(|e| e.to_string())?)
            .map_err(|e| format!("header: {e}"))?,
        None => return Err("an empty rows file".to_string()),
    };
    let mut files = Vec::with_capacity(header.files);
    for (index, line) in lines.enumerate() {
        let line = line.map_err(|e| e.to_string())?;
        if line.is_empty() {
            continue;
        }
        files.push(serde_json::from_str(&line).map_err(|e| format!("line {}: {e}", index + 2))?);
    }
    if files.len() != header.files {
        return Err(format!(
            "the header says {} file(s) and {} follow",
            header.files,
            files.len()
        ));
    }
    Ok(RowsFile { header, files })
}

/// The digest a rows file's `h` holds for `bytes`: sha256, lowercase hex.
#[allow(dead_code)]
pub fn content_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Where a service's rows go this run, or `None` when the walk is off: the
/// flag unset, or no local store (the Action and the cloud runner).
pub fn destination(repo_name: &str, service_name: Option<&str>) -> Option<PathBuf> {
    destination_from(
        std::env::var(POSITION_ROWS_ENV).ok().as_deref(),
        std::env::var_os(CACHE_DIR_ENV)
            .map(PathBuf::from)
            .as_deref(),
        repo_name,
        service_name,
    )
}

/// [`destination`] over the two settings it reads.
pub fn destination_from(
    flag: Option<&str>,
    store: Option<&Path>,
    repo_name: &str,
    service_name: Option<&str>,
) -> Option<PathBuf> {
    if flag != Some("1") {
        return None;
    }
    let store = store.filter(|dir| !dir.as_os_str().is_empty())?;
    Some(
        store
            .join(POSITIONS_DIR)
            .join(format!("{}.jsonl.gz", blob_stem(repo_name, service_name))),
    )
}

/// The temporary name the sidecar writes a rows file under before it renames
/// it into place.
pub fn partial_path(out: &Path) -> PathBuf {
    let mut name = out.as_os_str().to_owned();
    name.push(".partial");
    PathBuf::from(name)
}

/// One service's walk: where it ran and what it cost.
#[derive(Debug, Clone)]
pub struct ServiceWalk<'a> {
    /// The service's root and tsconfig, as its analysis sidecar was scoped.
    pub service_root: &'a Path,
    pub tsconfig: Option<&'a str>,
    /// The scanned repo's root: every path in the rows is relative to it.
    pub scan_root: &'a Path,
    /// The service's source files, as discovery walks them.
    pub files: &'a [PathBuf],
    pub out: &'a Path,
    pub service: &'a str,
}

/// Walk one service in a fresh record-only sidecar from `recipe` and write
/// its rows to `walk.out`. The process is started here and gone when this
/// returns, so it never overlaps the service's analysis sidecar.
///
/// Any rows file from an earlier run is removed first, so a walk that fails
/// leaves none: lost rows are lost, never stale. Every failure is the
/// caller's to log; nothing is recorded for the run.
pub fn record_service_rows(
    recipe: &SidecarRecipe,
    walk: &ServiceWalk<'_>,
) -> Result<PositionRowsCounts, String> {
    let partial = partial_path(walk.out);
    for stale in [walk.out, partial.as_path()] {
        match std::fs::remove_file(stale) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{} could not be removed: {e}", stale.display())),
        }
    }
    let started = Instant::now();
    let answer = recipe
        .spawn_scoped_record_only(walk.service_root, walk.tsconfig)
        .and_then(|sidecar| {
            let counts = sidecar.position_rows(walk.files, walk.scan_root, walk.out, walk.service);
            let peak = sidecar.peak_resident_bytes();
            drop(sidecar);
            counts.map(|counts| (counts, peak))
        });
    match answer {
        Ok((counts, peak)) => {
            debug!(
                "Position rows for service {}: {} row(s) over {} file(s), {} byte(s) \
                 ({} resolved, {} any, {} error type); load {} ms, walk {} ms, wall {:.1}s, \
                 peak {}",
                walk.service,
                counts.rows,
                counts.files,
                counts.bytes,
                counts.resolved,
                counts.any,
                counts.error_type,
                counts.load_ms,
                counts.walk_ms,
                started.elapsed().as_secs_f64(),
                peak.map(|bytes| format!("{} MB", bytes / (1024 * 1024)))
                    .unwrap_or_else(|| "unknown".to_string()),
            );
            Ok(counts)
        }
        Err(error) => {
            // The sidecar renames into place only once the file is whole; a
            // walk that died may have left its temporary file.
            let _ = std::fs::remove_file(&partial);
            let _ = std::fs::remove_file(walk.out);
            Err(error.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_walk_is_off_without_the_flag_or_a_store() {
        let store = Path::new("/store");
        assert_eq!(
            destination_from(None, Some(store), "repo", Some("api")),
            None
        );
        assert_eq!(
            destination_from(Some("0"), Some(store), "repo", Some("api")),
            None
        );
        assert_eq!(destination_from(Some("1"), None, "repo", Some("api")), None);
        assert_eq!(
            destination_from(Some("1"), Some(Path::new("")), "repo", None),
            None
        );
    }

    #[test]
    fn the_rows_file_sits_beside_the_blobs_under_the_blob_stem() {
        assert_eq!(
            destination_from(
                Some("1"),
                Some(Path::new("/store")),
                "repo",
                Some("apps/api")
            ),
            Some(PathBuf::from("/store/positions/repo__apps_api.jsonl.gz"))
        );
        assert_eq!(
            destination_from(Some("1"), Some(Path::new("/store")), "repo", None),
            Some(PathBuf::from("/store/positions/repo.jsonl.gz"))
        );
    }

    #[test]
    fn a_line_with_a_key_the_format_does_not_name_is_refused() {
        let text = "{\"v\":1,\"typescript\":\"6.0.2\",\"service\":\"api\",\"files\":1}\n\
                    {\"f\":\"a.ts\",\"h\":\"00\",\"r\":[],\"extra\":1}\n";
        assert!(read_rows(text.as_bytes()).is_err());
    }

    #[test]
    fn a_row_is_a_nine_element_array() {
        let text = "{\"v\":1,\"typescript\":\"6.0.2\",\"service\":\"api\",\"files\":1}\n\
                    {\"f\":\"a.ts\",\"h\":\"00\",\"r\":[[\"call\",0,3,\"f\",\"a.ts\",1,null,\"void\",0]]}\n";
        let file = read_rows(text.as_bytes()).expect("a valid file");
        let row = &file.files[0].r[0];
        assert_eq!(row.kind(), Kind::Call);
        assert_eq!((row.start(), row.end()), (0, 3));
        assert_eq!(row.symbol(), Some("f"));
        let short = "{\"v\":1,\"typescript\":\"6.0.2\",\"service\":\"api\",\"files\":1}\n\
                     {\"f\":\"a.ts\",\"r\":[[\"call\",0,3]]}\n";
        assert!(read_rows(short.as_bytes()).is_err());
    }
}
