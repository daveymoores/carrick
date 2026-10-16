//! Per-position compiler rows (carrick#2405, move 2 of the fact layer).
//!
//! Before the scan's analysis loop starts any sidecar, a fresh record-only
//! sidecar walks each service's source files and writes, for every call,
//! member access, comparison and expression identifier, what the compiler
//! knows there: the symbol, the declaration's file and line, the declaring
//! package and the type text. The walk is the sidecar's
//! (`src/sidecar/src/position-rows.ts`); the walks run in a lane of their own
//! (`crate::engine::walk_lane`). Every walk process exits before the first
//! analysis sidecar spawns, so the two programs never share a heap
//! (carrick#2413).
//!
//! The walk runs only with [`POSITION_ROWS_ENV`] set to `1`. With a local
//! store directory ([`CACHE_DIR_ENV`]) it writes
//! `<store>/positions/<repo>__<service>.jsonl.gz` beside the blobs. Both blob
//! listings read top-level `*.json` only, so the subdirectory is never read
//! as a blob. Without one (the Action, the cloud runner) it writes under a
//! per-run directory in the OS temp dir, never inside the scanned tree, and
//! the file is removed when the run ends ([`Destination`]). A walk that dies or
//! misses its deadline loses that service's rows and says so in the run log,
//! and nothing else: it is never a scan-health entry, a finding or a stop.
//!
//! The rows are read before each analysis pass's discovery (the loop, and
//! the owed-work retry), into a [`PositionIndex`] the
//! request summaries carry. Nothing reads a row yet (move 3 of the fact
//! layer reads them slice by slice); a lookup that finds none is a hole with
//! a reason ([`HoleReason`]), never a fallback.
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

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::debug;

use crate::cloud_storage::{CACHE_DIR_ENV, blob_stem};
use crate::request_summary::HoleReason;
use crate::services::type_sidecar::{PositionRowsCounts, SidecarRecipe};

/// Set to `1` to walk each service and read its rows.
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
    pub fn decl_file(&self) -> Option<&str> {
        self.4.as_deref()
    }
    pub fn decl_line(&self) -> Option<u32> {
        self.5
    }
    pub fn decl_package(&self) -> Option<&str> {
        self.6.as_deref()
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

/// Read a rows file from gzip JSON lines.
pub fn read_rows_file(path: &Path) -> Result<RowsFile, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    read_rows(BufReader::new(flate2::read::GzDecoder::new(file)))
}

/// [`read_rows_file`] over decompressed lines.
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
pub fn content_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Where one service's rows file goes this run.
///
/// Beside the blobs when there is a local store, and kept there. Otherwise
/// under a directory of this run's own in the OS temp dir: the file and, once
/// empty, the directory are removed when this is dropped, which is when the
/// run ends (carrick#2413).
#[derive(Debug)]
pub struct Destination {
    path: PathBuf,
    /// The per-run temp directory the file sits in, when there is no store.
    temporary: Option<PathBuf>,
}

impl Destination {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether the file is removed when this is dropped.
    pub fn is_temporary(&self) -> bool {
        self.temporary.is_some()
    }
}

impl Drop for Destination {
    fn drop(&mut self) {
        if let Some(dir) = &self.temporary {
            let _ = std::fs::remove_file(&self.path);
            let _ = std::fs::remove_file(partial_path(&self.path));
            // Another service of this run may still be writing beside it.
            let _ = std::fs::remove_dir(dir);
        }
    }
}

/// Where a service's rows go this run, or `None` when the flag is unset.
pub fn destination(repo_name: &str, service_name: Option<&str>) -> Option<Destination> {
    destination_from(
        std::env::var(POSITION_ROWS_ENV).ok().as_deref(),
        std::env::var_os(CACHE_DIR_ENV)
            .map(PathBuf::from)
            .as_deref(),
        &std::env::temp_dir(),
        repo_name,
        service_name,
    )
}

/// [`destination`] over the settings it reads: the flag, the local store and
/// the OS temp dir.
pub fn destination_from(
    flag: Option<&str>,
    store: Option<&Path>,
    temp: &Path,
    repo_name: &str,
    service_name: Option<&str>,
) -> Option<Destination> {
    if flag != Some("1") {
        return None;
    }
    let name = format!("{}.jsonl.gz", blob_stem(repo_name, service_name));
    Some(match store.filter(|dir| !dir.as_os_str().is_empty()) {
        Some(store) => Destination {
            path: store.join(POSITIONS_DIR).join(name),
            temporary: None,
        },
        None => {
            let dir = temp.join(format!("carrick-positions-{}", std::process::id()));
            Destination {
                path: dir.join(name),
                temporary: Some(dir),
            }
        }
    })
}

/// A service's rows, read once before discovery for the request summaries
/// to look up by span (carrick#2413).
///
/// Every file is bound to the bytes discovery read ([`Self::bind`]) before a
/// lookup in it can succeed: a file whose bytes hash otherwise than its rows'
/// `h` holds stale rows. A lookup that finds no usable row says why, with a
/// [`HoleReason`]; it never guesses.
#[derive(Debug, Default)]
pub struct PositionIndex {
    /// The scanned repo's canonical root: every row path is relative to it.
    scan_root: PathBuf,
    /// By repo-relative path with `/` separators, as [`FileRows::f`].
    files: HashMap<String, IndexedFile>,
}

#[derive(Debug)]
struct IndexedFile {
    hash: Option<String>,
    skip: Option<Skip>,
    rows: HashMap<(u32, u32, Kind), Row>,
    bound: Bound,
}

/// What discovery's read of a file said about its rows.
#[derive(Debug)]
enum Bound {
    /// Discovery has not read the file.
    Unread,
    /// The bytes discovery read hash to `h`. The text is kept only when its
    /// SWC byte offsets and the rows' UTF-16 offsets differ: anything but
    /// plain ASCII.
    Fresh(Option<Arc<str>>),
    /// The bytes discovery read hash otherwise.
    Stale,
}

impl PositionIndex {
    /// An index with no rows, for a service whose walk wrote none: every
    /// lookup is [`HoleReason::RowsMissing`].
    pub fn missing(scan_root: &Path) -> Self {
        Self {
            scan_root: scan_root.to_path_buf(),
            files: HashMap::new(),
        }
    }

    /// The index over a rows file read for the repo rooted at `scan_root`.
    pub fn of(scan_root: &Path, rows: RowsFile) -> Self {
        let files = rows
            .files
            .into_iter()
            .map(|file| {
                let rows = file
                    .r
                    .into_iter()
                    .map(|row| ((row.start(), row.end(), row.kind()), row))
                    .collect();
                (
                    file.f,
                    IndexedFile {
                        hash: file.h,
                        skip: file.skip,
                        rows,
                        bound: Bound::Unread,
                    },
                )
            })
            .collect();
        Self {
            scan_root: scan_root.to_path_buf(),
            files,
        }
    }

    /// How many files the rows name.
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// How many of the files the rows name are in each state: `fresh`,
    /// `stale`, `unread` (discovery did not read it), `not_in_program` and
    /// `unreadable` (the walk's skips). For the run log and the census.
    pub fn file_states(&self) -> std::collections::BTreeMap<&'static str, usize> {
        let mut states = std::collections::BTreeMap::new();
        for file in self.files.values() {
            let state = match (&file.skip, &file.bound) {
                (Some(Skip::NotInProgram), _) => "not_in_program",
                (Some(Skip::Unreadable), _) => "unreadable",
                (None, Bound::Fresh(_)) => "fresh",
                (None, Bound::Stale) => "stale",
                (None, Bound::Unread) => "unread",
            };
            *states.entry(state).or_default() += 1;
        }
        states
    }

    /// `file` (a path under the scan root) as the rows name it.
    fn relative(&self, file: &Path) -> Option<String> {
        let relative = file.strip_prefix(&self.scan_root).ok()?;
        let parts: Vec<_> = relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect();
        Some(parts.join("/"))
    }

    /// Record the text discovery parsed `file` from, as read from disk (BOM
    /// included): the rows' `h` is checked against it, and lookups in the
    /// file convert spans over it.
    pub fn bind(&mut self, file: &Path, text: &str) {
        let Some(relative) = self.relative(file) else {
            return;
        };
        let Some(indexed) = self.files.get_mut(&relative) else {
            return;
        };
        indexed.bound = if indexed.hash.as_deref() == Some(content_hash(text.as_bytes()).as_str()) {
            Bound::Fresh((!text.is_ascii()).then(|| Arc::from(text)))
        } else {
            Bound::Stale
        };
    }

    /// Why `file` holds no usable rows, or its rows.
    fn file(&self, file: &Path) -> Result<(&IndexedFile, Option<&str>), HoleReason> {
        let indexed = self
            .relative(file)
            .and_then(|relative| self.files.get(&relative))
            .ok_or(HoleReason::RowsMissing)?;
        if indexed.skip.is_some() {
            return Err(HoleReason::NotInProgram);
        }
        match &indexed.bound {
            Bound::Fresh(text) => Ok((indexed, text.as_deref())),
            Bound::Stale => Err(HoleReason::RowsStale),
            Bound::Unread => Err(HoleReason::RowsMissing),
        }
    }

    /// The row of `kind` at the SWC span `lo..hi` of `file`, counted from
    /// [`crate::swc_scanner::SWC_SPAN_BASE`] as in a file parsed on its own
    /// (what [`crate::request_summary::FileIr`] spans convert to).
    pub fn lookup(&self, file: &Path, lo: u32, hi: u32, kind: Kind) -> Result<&Row, HoleReason> {
        let (indexed, text) = self.file(file)?;
        let (start, end) = match text {
            Some(text) => (
                crate::utils::sidecar_position(text, lo),
                crate::utils::sidecar_position(text, hi),
            ),
            None => (
                lo.saturating_sub(crate::swc_scanner::SWC_SPAN_BASE),
                hi.saturating_sub(crate::swc_scanner::SWC_SPAN_BASE),
            ),
        };
        let row = indexed
            .rows
            .get(&(start, end, kind))
            .ok_or(HoleReason::RowsMissing)?;
        if row.flags() & FLAG_CHECKER_THREW != 0 {
            return Err(HoleReason::CheckerThrew);
        }
        Ok(row)
    }

    /// The row at [`Self::lookup`]'s position whose symbol names a
    /// declaration: one with no symbol, or with the error type, is a hole
    /// that says whether the name was an import (`import_local`); a member
    /// or call row typed `any` with no symbol is an untyped receiver.
    pub fn declaration(
        &self,
        file: &Path,
        lo: u32,
        hi: u32,
        kind: Kind,
        import_local: bool,
    ) -> Result<&Row, HoleReason> {
        let row = self.lookup(file, lo, hi, kind)?;
        let unresolved = if import_local {
            HoleReason::UnresolvedImport
        } else {
            HoleReason::UnresolvedName
        };
        if row.flags() & FLAG_ERROR_TYPE != 0 {
            return Err(unresolved);
        }
        if row.symbol().is_none() {
            if matches!(kind, Kind::Member | Kind::Call) && row.flags() & FLAG_ANY != 0 {
                return Err(HoleReason::UntypedReceiver);
            }
            return Err(unresolved);
        }
        Ok(row)
    }
}

/// Read the rows `destination` holds into an index for the repo rooted at
/// `scan_root`, or an empty one, with the reason logged, when there are none
/// to read. The file is read once: a temporary one is removed when
/// `destination` is dropped.
pub fn load_index(destination: &Destination, scan_root: &Path, service: &str) -> PositionIndex {
    match read_rows_file(destination.path()) {
        Ok(rows) => {
            let index = PositionIndex::of(scan_root, rows);
            debug!(
                "Position rows for service {service} read: {} file(s)",
                index.file_count()
            );
            index
        }
        Err(error) => {
            debug!("Position rows for service {service} were not read: {error}");
            PositionIndex::missing(scan_root)
        }
    }
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

    fn path_of(destination: Option<Destination>) -> Option<(PathBuf, bool)> {
        destination.map(|d| (d.path().to_path_buf(), d.is_temporary()))
    }

    #[test]
    fn the_walk_is_off_without_the_flag() {
        let (store, temp) = (Path::new("/store"), Path::new("/tmp"));
        assert_eq!(
            path_of(destination_from(
                None,
                Some(store),
                temp,
                "repo",
                Some("api")
            )),
            None
        );
        assert_eq!(
            path_of(destination_from(
                Some("0"),
                Some(store),
                temp,
                "repo",
                Some("api")
            )),
            None
        );
    }

    #[test]
    fn the_rows_file_sits_beside_the_blobs_under_the_blob_stem() {
        let temp = Path::new("/tmp");
        assert_eq!(
            path_of(destination_from(
                Some("1"),
                Some(Path::new("/store")),
                temp,
                "repo",
                Some("apps/api")
            )),
            Some((
                PathBuf::from("/store/positions/repo__apps_api.jsonl.gz"),
                false
            ))
        );
        assert_eq!(
            path_of(destination_from(
                Some("1"),
                Some(Path::new("/store")),
                temp,
                "repo",
                None
            )),
            Some((PathBuf::from("/store/positions/repo.jsonl.gz"), false))
        );
    }

    /// carrick#2413: with no store the flag still walks, into a directory
    /// of this run's own under the temp dir, which goes when the service
    /// ends.
    #[test]
    fn without_a_store_the_rows_go_to_a_run_directory_that_is_removed() {
        let temp = tempfile::tempdir().unwrap();
        for store in [None, Some(Path::new(""))] {
            let destination =
                destination_from(Some("1"), store, temp.path(), "repo", Some("apps/api"))
                    .expect("the flag walks without a store");
            assert!(destination.is_temporary());
            let dir = destination.path().parent().unwrap().to_path_buf();
            assert_eq!(dir.parent(), Some(temp.path()));
            assert_eq!(
                destination.path().file_name().unwrap(),
                "repo__apps_api.jsonl.gz"
            );
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(destination.path(), b"rows").unwrap();
            drop(destination);
            assert!(!dir.exists(), "the run directory outlived the service");
        }
    }

    fn row(kind: Kind, start: u32, end: u32, symbol: Option<&str>, flags: u8) -> Row {
        Row(
            kind,
            start,
            end,
            symbol.map(str::to_string),
            symbol.map(|_| "src/a.ts".to_string()),
            symbol.map(|_| 1),
            None,
            Some("string".to_string()),
            flags,
        )
    }

    fn file_rows(f: &str, text: Option<&str>, skip: Option<Skip>, r: Vec<Row>) -> FileRows {
        FileRows {
            f: f.to_string(),
            h: text.map(|text| content_hash(text.as_bytes())),
            skip,
            r,
        }
    }

    /// Every case of carrick#2413 §3 reads its reason, and a usable row
    /// reads as itself.
    #[test]
    fn every_missing_stale_or_skipped_row_is_a_hole_with_its_reason() {
        let root = Path::new("/repo");
        let text = "const a = b;\n";
        let rows = RowsFile {
            header: Header {
                v: FORMAT_VERSION,
                typescript: "6.0.2".to_string(),
                service: "api".to_string(),
                files: 4,
            },
            files: vec![
                file_rows(
                    "src/a.ts",
                    Some(text),
                    None,
                    vec![
                        row(Kind::Ident, 10, 11, Some("b"), 0),
                        row(Kind::Ident, 6, 7, None, FLAG_ERROR_TYPE),
                        row(Kind::Ident, 0, 1, None, FLAG_CHECKER_THREW),
                        row(Kind::Member, 0, 3, None, FLAG_ANY),
                        row(Kind::Call, 0, 4, Some("f"), FLAG_ERROR_TYPE),
                    ],
                ),
                file_rows("src/stale.ts", Some("old text"), None, vec![]),
                file_rows(
                    "src/legacy.mjs",
                    Some(text),
                    Some(Skip::NotInProgram),
                    vec![],
                ),
                file_rows("src/unread.ts", Some(text), None, vec![]),
            ],
        };
        let mut index = PositionIndex::of(root, rows);
        let a = root.join("src/a.ts");
        index.bind(&a, text);
        index.bind(&root.join("src/stale.ts"), "new text");
        index.bind(&root.join("src/legacy.mjs"), text);
        let base = crate::swc_scanner::SWC_SPAN_BASE;
        let at = |lo: u32, hi: u32| (lo + base, hi + base);

        let (lo, hi) = at(10, 11);
        assert_eq!(
            index
                .declaration(&a, lo, hi, Kind::Ident, false)
                .map(Row::symbol),
            Ok(Some("b"))
        );
        // No row of the kind at the position.
        assert_eq!(
            index.lookup(&a, lo, hi, Kind::Member).err(),
            Some(HoleReason::RowsMissing)
        );
        // A file the rows do not name, and one discovery never read.
        assert_eq!(
            index
                .lookup(&root.join("src/b.ts"), lo, hi, Kind::Ident)
                .err(),
            Some(HoleReason::RowsMissing)
        );
        assert_eq!(
            index
                .lookup(&root.join("src/unread.ts"), lo, hi, Kind::Ident)
                .err(),
            Some(HoleReason::RowsMissing)
        );
        assert_eq!(
            index
                .lookup(&root.join("src/stale.ts"), lo, hi, Kind::Ident)
                .err(),
            Some(HoleReason::RowsStale)
        );
        assert_eq!(
            index
                .lookup(&root.join("src/legacy.mjs"), lo, hi, Kind::Ident)
                .err(),
            Some(HoleReason::NotInProgram)
        );
        let (lo, hi) = at(0, 1);
        assert_eq!(
            index.lookup(&a, lo, hi, Kind::Ident).err(),
            Some(HoleReason::CheckerThrew)
        );
        let (lo, hi) = at(6, 7);
        assert_eq!(
            index.declaration(&a, lo, hi, Kind::Ident, false).err(),
            Some(HoleReason::UnresolvedName)
        );
        assert_eq!(
            index.declaration(&a, lo, hi, Kind::Ident, true).err(),
            Some(HoleReason::UnresolvedImport)
        );
        let (lo, hi) = at(0, 3);
        assert_eq!(
            index.declaration(&a, lo, hi, Kind::Member, false).err(),
            Some(HoleReason::UntypedReceiver)
        );
        // The error type wins over a symbol.
        let (lo, hi) = at(0, 4);
        assert_eq!(
            index.declaration(&a, lo, hi, Kind::Call, false).err(),
            Some(HoleReason::UnresolvedName)
        );
        // No rows file at all.
        assert_eq!(
            PositionIndex::missing(root)
                .lookup(&a, lo, hi, Kind::Call)
                .err(),
            Some(HoleReason::RowsMissing)
        );
    }

    /// A span in a file with multi-byte text, CRLF line ends or a BOM joins
    /// its row: the conversion is `sidecar_position` over the bound text.
    #[test]
    fn spans_join_in_multi_byte_crlf_and_bom_files() {
        let root = Path::new("/repo");
        let base = crate::swc_scanner::SWC_SPAN_BASE;
        for text in [
            "const s = \"é€😀\";\r\nconst t = s;\r\n",
            "\u{feff}const s = \"é\";\nconst t = s;\n",
            "const s = 1;\r\nconst t = s;\r\n",
        ] {
            // The SWC span of the last `s`: bytes after any BOM.
            let body = text.strip_prefix('\u{feff}').unwrap_or(text);
            let byte = body.rfind(" s;").unwrap() as u32 + 1;
            let unit = body[..byte as usize].encode_utf16().count() as u32;
            let rows = RowsFile {
                header: Header {
                    v: FORMAT_VERSION,
                    typescript: "6.0.2".to_string(),
                    service: "api".to_string(),
                    files: 1,
                },
                files: vec![file_rows(
                    "a.ts",
                    Some(text),
                    None,
                    vec![row(Kind::Ident, unit, unit + 1, Some("s"), 0)],
                )],
            };
            let mut index = PositionIndex::of(root, rows);
            let file = root.join("a.ts");
            index.bind(&file, text);
            assert_eq!(
                index
                    .lookup(&file, byte + base, byte + 1 + base, Kind::Ident)
                    .map(Row::symbol),
                Ok(Some("s")),
                "{text:?}"
            );
        }
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
