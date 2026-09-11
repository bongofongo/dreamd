//! The share flow: hand one or more of the repo's markdown files to the
//! system's own sharing surface, without the reader leaving the window.
//!
//! Everything in this file is pure and compiles on both platforms. The native
//! picker (`picker`) and the print-to-file path (`pdf`) are macOS-only and sit
//! beside it, so the part that decides *what may be shared* is testable
//! wherever `cargo test` runs — the same reason [`crate::guard`] exists at all.
//!
//! Two tenets meet here:
//!
//! * Tenet 1 — nothing is written into the repo. A PDF is an artifact rather
//!   than session state, so it lands in the system temp directory exactly as
//!   [`crate::send`]'s query files do, under a name this module can recognise
//!   later and sweep. Sharing a `.md` writes nothing at all: the picker is
//!   handed the file already on disk.
//! * Tenet 4 — a path reaching the picker is checked the way `delete_file`
//!   checks one. Canonicalised first, then `inside_root`, then `is_markdown`.
//!   The frontend chooses from a tree dreamd itself walked, so this is a
//!   backstop rather than the only check; it is here because the frontend is
//!   the layer a document's own content can reach.

use crate::guard;
use crate::is_markdown;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(target_os = "macos")]
pub mod pdf;
#[cfg(target_os = "macos")]
pub mod picker;

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Every export this process has minted and not handed away.
///
/// A PDF is a means rather than a document the reader asked to keep: it exists
/// so the share sheet has something to offer, and once the window is gone
/// there is nothing left that could want it. So the session cleans up after
/// itself on the way out ([`cleanup_session`]) and the day-stamped sweep below
/// becomes the backstop for a process that was killed rather than quit.
///
/// A saved export is [`forget`]ten instead: it has been moved somewhere the
/// reader chose and is theirs, not ours.
static EXPORTS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Every export this module writes starts with this. The sweep will only ever
/// consider deleting a name carrying it.
const EXPORT_PREFIX: &str = "dreamd-share-";

/// What leaves the app.
///
/// Deliberately closed, and deliberately not a string the frontend passes
/// through: a format is a branch in the native code below, so a third value is
/// a decision someone makes here rather than one a payload can assert.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// The source file itself, shared where it already lies.
    Md,
    /// The open document, printed through the `@media print` sheet.
    Pdf,
}

/// How a finished share is reported back to the frontend for its toast.
#[derive(Debug, Clone, Serialize)]
pub struct ShareResult {
    pub format: Format,
    /// How many files went to the picker.
    pub count: usize,
    /// Human-readable detail — the file name, or "3 files".
    pub detail: String,
}

/// Validate the frontend's chosen paths into something the picker may open.
///
/// Order matters: canonicalise *first*, so a symlink pointing out of the repo
/// is resolved before it is judged rather than after. `delete_file` does the
/// same, and for the same reason.
///
/// An empty list is an error rather than an empty share, because a picker with
/// no items shows an empty menu the reader cannot act on and would read as the
/// feature being broken.
pub fn resolve(root: &Path, files: &[String]) -> Result<Vec<PathBuf>, String> {
    if files.is_empty() {
        return Err("nothing selected to share".into());
    }
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut out = Vec::with_capacity(files.len());
    for f in files {
        let target = Path::new(f)
            .canonicalize()
            .map_err(|e| format!("cannot resolve {f}: {e}"))?;
        if !guard::inside_root(&root, &target) {
            return Err("refusing to share a file outside the repo root".into());
        }
        if !is_markdown(&target) {
            return Err(format!(
                "refusing to share a non-markdown file: {}",
                target.display()
            ));
        }
        // A repeated path would put the same attachment on a mail draft twice.
        if !out.contains(&target) {
            out.push(target);
        }
    }
    Ok(out)
}

/// The toast line for a finished share.
pub fn describe(files: &[PathBuf]) -> String {
    match files {
        [one] => one
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| one.display().to_string()),
        many => format!("{} files", many.len()),
    }
}

/// Whole days since the Unix epoch — the same coarse stamp [`crate::send`]
/// uses, and coarse for the same reason: the only question an export's name
/// has to answer is "was this written before today".
fn epoch_day(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0)
}

fn export_name(day: u64, pid: u32, n: u64, stem: &str) -> String {
    // The document's own name is carried through so the recipient sees
    // `notes.pdf` in the mail draft rather than a serial number. It is a file
    // *stem* taken from a path dreamd walked, never arbitrary text.
    format!("{EXPORT_PREFIX}{day}-{pid}-{n}-{stem}.pdf")
}

/// The day stamp out of a name [`export_name`] produced, or `None` for anything
/// else in the temp directory.
///
/// The stem may itself contain `-`, so this parses the three leading fields and
/// stops caring — where `send`'s parser can demand an exact field count, this
/// one must not.
fn export_day(name: &str) -> Option<u64> {
    let stem = name.strip_prefix(EXPORT_PREFIX)?.strip_suffix(".pdf")?;
    let mut fields = stem.split('-');
    let day = fields.next()?.parse().ok()?;
    // pid and serial must both be present and numeric; without that check a
    // user's own `dreamd-share-notes.pdf` would parse its way into the sweep.
    let _pid: u32 = fields.next()?.parse().ok()?;
    let _n: u64 = fields.next()?.parse().ok()?;
    Some(day)
}

/// Delete exports left in `dir` by earlier days. Returns how many went.
///
/// Today's are kept for the reason `send`'s are: the reader may still have a
/// mail draft open holding the attachment, and a second dreamd is writing its
/// own. Best-effort throughout.
fn sweep_stale_exports(dir: &Path, today: u64) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut swept = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(day) = name.to_str().and_then(export_day) else {
            continue;
        };
        // `<`, not `!=`: a clock that has gone backwards should leave a file
        // alone rather than delete one a live process is pointing at.
        if day < today && std::fs::remove_file(entry.path()).is_ok() {
            swept += 1;
        }
    }
    swept
}

/// Reduce a frontend-supplied name to something safe to put in a filename.
///
/// `file_stem` already strips directories, so `../../etc/passwd` arrives as
/// `passwd` — but a name is easier to reason about when the rule is written
/// down rather than inherited from `Path`'s behaviour, and a separator that
/// survived would put the export somewhere nobody swept. Anything left empty
/// falls back to `document`.
pub fn safe_stem(name: &str) -> String {
    let cleaned: String = Path::new(name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .filter(|c| !matches!(c, '/' | '\\' | ':' | '\0'))
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').to_string();
    if trimmed.is_empty() {
        "document".into()
    } else {
        trimmed
    }
}

/// Mint the path a PDF export should be written to, sweeping earlier days' on
/// the session's first call.
///
/// `source` names the document being exported and supplies the stem; anything
/// unusable falls back to `document`.
pub fn export_path(source: &Path) -> PathBuf {
    let dir = std::env::temp_dir();
    let day = epoch_day(SystemTime::now());
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    if n == 0 {
        sweep_stale_exports(&dir, day);
    }
    let stem = source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "document".into());
    let path = dir.join(export_name(day, std::process::id(), n, &stem));
    EXPORTS.lock().unwrap().push(path.clone());
    path
}

/// Stop tracking `path` — it has been saved somewhere the reader chose, so it
/// is no longer this session's to delete.
pub fn forget(path: &Path) {
    EXPORTS.lock().unwrap().retain(|p| p != path);
}

/// Delete every export this session still owns. Called on the way out.
///
/// Best-effort, and deliberately not fussy about failures: a file that will
/// not delete is one something else is holding, and the day-stamped sweep will
/// find it tomorrow. Returns how many went, which is what the test asserts on.
pub fn cleanup_session() -> usize {
    let mut held = EXPORTS.lock().unwrap();
    let mut gone = 0;
    for p in held.iter() {
        if std::fs::remove_file(p).is_ok() {
            gone += 1;
        }
    }
    held.clear();
    gone
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch repo with a couple of files in it. Returned canonicalised,
    /// because macOS's temp dir is a symlink and every path here is compared
    /// after resolution.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dreamd-share-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("docs")).expect("scratch dir");
        std::fs::write(dir.join("a.md"), "# a\n").expect("fixture");
        std::fs::write(dir.join("docs/b.md"), "# b\n").expect("fixture");
        std::fs::write(dir.join("notes.txt"), "not markdown\n").expect("fixture");
        dir.canonicalize().unwrap_or(dir)
    }

    fn s(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn resolves_markdown_inside_the_root() {
        let root = scratch("ok");
        let files = vec![s(&root.join("a.md")), s(&root.join("docs/b.md"))];
        let out = resolve(&root, &files).expect("both are markdown inside the root");
        assert_eq!(out.len(), 2);
        assert!(out[0].ends_with("a.md"));
        assert!(out[1].ends_with("docs/b.md"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn refuses_an_empty_selection() {
        let root = scratch("empty");
        let err = resolve(&root, &[]).unwrap_err();
        assert!(err.contains("nothing selected"), "got {err:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn refuses_a_file_outside_the_root() {
        let root = scratch("outside");
        // A sibling sharing the root's textual prefix — the case `inside_root`
        // exists for, asserted here too so the guard cannot be swapped for a
        // `str::starts_with` without this going red.
        let sibling = root.with_file_name(format!(
            "{}-private",
            root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(&sibling).expect("sibling");
        let secret = sibling.join("secret.md");
        std::fs::write(&secret, "# secret\n").expect("fixture");

        let err = resolve(&root, &[s(&secret)]).unwrap_err();
        assert!(err.contains("outside the repo root"), "got {err:?}");

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&sibling);
    }

    #[test]
    fn refuses_a_symlink_pointing_out_of_the_root() {
        // Canonicalising before judging is the whole point of the ordering in
        // `resolve`; a link resolved *after* the check would pass.
        let root = scratch("symlink");
        let outside =
            std::env::temp_dir().join(format!("dreamd-share-target-{}", std::process::id()));
        std::fs::write(&outside, "# elsewhere\n").expect("fixture");
        let link = root.join("link.md");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &link).expect("symlink");

        #[cfg(unix)]
        {
            let err = resolve(&root, &[s(&link)]).unwrap_err();
            assert!(err.contains("outside the repo root"), "got {err:?}");
        }

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_file(&outside);
    }

    #[test]
    fn refuses_a_non_markdown_file() {
        let root = scratch("ext");
        let err = resolve(&root, &[s(&root.join("notes.txt"))]).unwrap_err();
        assert!(err.contains("non-markdown"), "got {err:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_repeated_path_is_shared_once() {
        // Two checkboxes cannot produce this, but the open document being
        // pre-selected *and* ticked in the tree can.
        let root = scratch("dupe");
        let one = s(&root.join("a.md"));
        let out = resolve(&root, &[one.clone(), one]).expect("valid");
        assert_eq!(out.len(), 1, "the same file went twice");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn describe_names_one_file_and_counts_many() {
        assert_eq!(describe(&[PathBuf::from("/repo/notes.md")]), "notes.md");
        assert_eq!(
            describe(&[PathBuf::from("/a.md"), PathBuf::from("/b.md")]),
            "2 files"
        );
    }

    #[test]
    fn the_written_name_is_one_the_sweeper_can_read() {
        // The halves are only useful together: a naming change the parser
        // stopped recognising would disable the sweep silently.
        assert_eq!(
            export_day(&export_name(20_660, 4242, 7, "notes")),
            Some(20_660)
        );
        // A stem carrying the separator must still parse — this is where the
        // scheme differs from `send`'s fixed field count.
        assert_eq!(
            export_day(&export_name(20_660, 4242, 7, "my-long-name")),
            Some(20_660)
        );
        assert_eq!(
            export_day("dreamd-share-notes.pdf"),
            None,
            "a user's own file"
        );
        assert_eq!(export_day("holiday.pdf"), None);
        assert_eq!(
            export_day(&export_name(20_660, 4242, 7, "x").replace(".pdf", ".md")),
            None
        );
    }

    #[test]
    fn stale_exports_are_swept_but_todays_are_kept() {
        let dir = std::env::temp_dir().join(format!("dreamd-share-sweep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");

        let today = 20_660;
        let put = |name: String| {
            let p = dir.join(name);
            std::fs::write(&p, "pdf").expect("fixture");
            p
        };

        let yesterday = put(export_name(today - 1, 99, 0, "notes"));
        let last_year = put(export_name(today - 400, 99, 3, "old"));
        let kept = [
            put(export_name(today, 99, 1, "notes")),
            put(export_name(today, 1234, 7, "other")),
            put(export_name(today + 1, 99, 0, "future")),
            put("dreamd-share-notes.pdf".into()),
            put("holiday.pdf".into()),
        ];

        assert_eq!(sweep_stale_exports(&dir, today), 2);
        assert!(!yesterday.exists(), "yesterday's export survived");
        assert!(!last_year.exists(), "last year's export survived");
        for k in &kept {
            assert!(k.exists(), "swept {} and should not have", k.display());
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_export_path_carries_the_documents_name() {
        let p = export_path(Path::new("/repo/docs/design notes.md"));
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with(EXPORT_PREFIX), "{name}");
        assert!(name.ends_with("-design notes.pdf"), "{name}");
        assert_eq!(p.parent(), Some(std::env::temp_dir().as_path()));
    }

    #[test]
    fn the_session_deletes_the_exports_it_still_owns() {
        // `export_path` records; `forget` hands one over to the reader. What is
        // left is what a quit should take with it.
        let kept = export_path(Path::new("saved.md"));
        let dropped = export_path(Path::new("shared.md"));
        std::fs::write(&kept, "pdf").expect("fixture");
        std::fs::write(&dropped, "pdf").expect("fixture");

        forget(&kept);
        assert!(cleanup_session() >= 1, "the tracked export survived");
        assert!(!dropped.exists(), "a tracked export was not cleaned up");
        assert!(kept.exists(), "a forgotten export was deleted anyway");

        // And the registry is empty afterwards, so a second quit is a no-op.
        assert_eq!(cleanup_session(), 0);
        let _ = std::fs::remove_file(&kept);
    }

    #[test]
    fn a_supplied_name_cannot_escape_the_temp_directory() {
        // The frontend names the PDF after the selection, so this is the one
        // string in the export path that did not come from the walker.
        assert_eq!(safe_stem("../../etc/passwd"), "passwd");
        assert_eq!(safe_stem("a/b/c.md"), "c");
        assert_eq!(safe_stem("notes.md"), "notes");
        assert_eq!(safe_stem("   "), "document");
        assert_eq!(safe_stem(""), "document");
        assert_eq!(safe_stem("..."), "document");
        assert!(!safe_stem("we:ird/name").contains(['/', ':']));
    }

    #[test]
    fn an_export_path_falls_back_when_there_is_no_stem() {
        let name = export_path(Path::new("/"))
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(name.ends_with("-document.pdf"), "{name}");
    }
}
