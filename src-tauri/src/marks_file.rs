//! Reading and writing the per-repo marks file.
//!
//! ```text
//! ~/.config/dreamd/marks/<basename>-<16hex>.json    0600   (dir 0700)
//! ```
//!
//! This is the one place session state is allowed to outlive the process, and
//! it is a deliberate amendment to tenet 2 rather than an oversight — marks
//! that vanish on quit make an agent loop that spans sessions impossible. The
//! user's markdown is still never written (tenet 1); everything here lands
//! under `~/.config/dreamd/`.
//!
//! **The hash is FNV-1a, and must stay FNV-1a.** `DefaultHasher`'s output is
//! explicitly undefined across Rust releases, so a toolchain bump would rename
//! every user's marks file and silently orphan their work — a data-loss bug
//! with no error message and no stack trace. `the_path_for_two_roots_differs_and_is_stable`
//! pins the digest against a hardcoded hex string precisely so that swap
//! fails loudly. The same constants already anchor `annotations::mint_id`.
//!
//! **Everything a loaded file must survive lives in [`admit`].** Not in
//! [`load`], which only does I/O and hands the parsed document over. That split
//! is the whole shape of this module: a rule that needs a real directory on
//! disk to exercise is a rule nobody writes a test for, and the rules here are
//! the ones that decide whether a hand-edited or copied JSON file can point
//! dreamd at `/etc/passwd`. If you add a load-time rule, add it to `admit`.
//!
//! What is *not* here, on purpose: the debounced save thread, the socket-based
//! multi-instance lock, the startup wiring in `main.rs`, and the
//! `dreamd marks prune` subcommand. This module is pure logic plus two
//! filesystem entry points; the scheduling of those entry points is the
//! caller's business.

use crate::annotations::{Highlight, HighlightState, Id, Store};
use crate::config;
use crate::guard;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The format version this build writes. Read but never enforced — see
/// [`MarksDoc`].
pub const VERSION: u32 = 1;

/// Refuse to even read a marks file larger than this. A few thousand marks is
/// a few hundred KiB; 4 MiB means something has gone wrong, and parsing it
/// would block startup on a file we are about to distrust anyway.
pub const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// Per-field ceiling for `quote`, `prefix` and `suffix`. A highlight is a
/// sentence or a paragraph; 8 KiB is generous by two orders of magnitude, and
/// the point is to bound what a hand-edited file can push through `locate` on
/// every reanchor.
pub const MAX_FIELD_BYTES: usize = 8 * 1024;

/// Default `marks.max_per_repo`. Kept here rather than in `config` because
/// nothing reads the config key yet; when the GC lands it should pass its own
/// value to [`enforce_cap`].
pub const MAX_PER_REPO: usize = 2000;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// The on-disk document.
///
/// `#[serde(default)]` is container-wide and there is **no**
/// `deny_unknown_fields`, on both this and [`Highlight`]. A marks file written
/// by a later dreamd must load in an earlier one, keeping what it understands,
/// rather than being rejected wholesale — that is what lets step 5 add a field
/// without stranding anyone who downgrades. `version` is recorded for a future
/// migration to branch on; nothing refuses a document for carrying a number it
/// does not recognise, because refusing is the failure mode this is designed to
/// avoid.
///
/// `root` is what makes a hash collision or a copied file *detectable*: `admit`
/// refuses a document whose root disagrees with the one being opened rather
/// than adopting another repo's marks.
///
/// There is no `next_id`. Ids stopped coming from a counter in step 1, which is
/// that change's quiet payoff: nothing about identity has to survive in this
/// file for an id written down last session to still mean the same mark.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MarksDoc {
    pub version: u32,
    /// The canonicalised repo root these marks belong to.
    pub root: String,
    /// Unix seconds.
    pub saved_at: u64,
    pub highlights: Vec<Highlight>,
    pub stack: Vec<Id>,
}

// ---- paths ---------------------------------------------------------------

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// FNV-1a over the root's bytes, as 16 lowercase hex digits.
///
/// See the module doc: this is pinned by test, and swapping it for a
/// `std::hash` hasher is a silent data-loss bug.
pub fn root_hash(canonical_root: &Path) -> String {
    format!(
        "{:016x}",
        fnv1a(canonical_root.to_string_lossy().as_bytes())
    )
}

/// The last component of the root, reduced to characters that are unambiguous
/// in a file name on every platform dreamd runs on.
///
/// Cosmetic only — the hash is what identifies the repo. It exists so that
/// `ls ~/.config/dreamd/marks` is readable, which `cli.rs` states as policy.
fn basename(canonical_root: &Path) -> String {
    let raw = canonical_root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let cleaned: String = raw
        .chars()
        .take(32)
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    // A root of `/` has no file name, and a name of all-dots would make a
    // file that is awkward to remove.
    if cleaned.trim_matches(['.', '-']).is_empty() {
        "repo".to_string()
    } else {
        cleaned
    }
}

/// The marks file name for an **already canonical** root.
///
/// Pure on purpose: the `canonicalize` call is [`path_for`]'s job, so the hash
/// and the name can be pinned by a unit test that touches no directory and no
/// `config_dir()`.
pub fn file_name_for(canonical_root: &Path) -> String {
    format!(
        "{}-{}.json",
        basename(canonical_root),
        root_hash(canonical_root)
    )
}

/// `~/.config/dreamd/marks`.
pub fn dir() -> PathBuf {
    config::config_dir().join("marks")
}

/// Canonicalise, falling back to the path as given.
///
/// The fallback matters: `load` is called for roots that may have been renamed
/// or unmounted since, and a hard failure there would lose marks rather than
/// report a missing directory.
fn canonical(root: &Path) -> PathBuf {
    root.canonicalize().unwrap_or_else(|_| root.to_path_buf())
}

pub fn path_for(root: &Path) -> PathBuf {
    dir().join(file_name_for(&canonical(root)))
}

// ---- admission -----------------------------------------------------------

fn truncate_at(text: &mut String, max: usize) {
    if text.len() <= max {
        return;
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
}

/// Is this a path a document in `root` is allowed to name?
///
/// [`guard::inside_root`] compares whole components and is the containment
/// decision, but it documents that resolving `..` is the caller's job — and
/// here the caller cannot resolve it, because a marks file routinely names
/// files that no longer exist and `canonicalize` would fail on exactly those.
/// So `..` is refused outright instead: `/repo/../etc/passwd` *does* start with
/// `/repo` component-wise, and would otherwise walk straight out.
fn path_is_admissible(root: &Path, file_path: &str) -> bool {
    if file_path.is_empty() {
        return false;
    }
    let path = Path::new(file_path);
    if !path.is_absolute() {
        return false;
    }
    if path.components().any(|c| c == Component::ParentDir) {
        return false;
    }
    guard::inside_root(root, path)
}

/// Trim `highlights` down to `max`, oldest first, and **never** an annotated
/// mark.
///
/// Stale-and-unannotated goes first because a stale mark is one whose quote
/// stopped resolving — usually mid-`git checkout`, and usually about to come
/// back — but an unannotated one carries no question anyone asked. An
/// annotated mark is a question in flight and is never dropped, so a store of
/// nothing but annotated marks stays over the cap by design; the alternative is
/// deleting the user's work to satisfy a number.
///
/// Insertion order is the age order: the vector is appended to, and a loaded
/// file preserves the order it was saved in.
pub fn enforce_cap(highlights: &mut Vec<Highlight>, max: usize) {
    if highlights.len() <= max {
        return;
    }
    for stale_only in [true, false] {
        let mut excess = highlights.len().saturating_sub(max);
        if excess == 0 {
            break;
        }
        highlights.retain(|h| {
            if excess == 0 {
                return true;
            }
            let droppable =
                h.annotation.is_none() && (!stale_only || h.state == HighlightState::Stale);
            if droppable {
                excess -= 1;
                false
            } else {
                true
            }
        });
    }
    if highlights.len() > max {
        eprintln!(
            "dreamd: {} marks exceeds marks.max_per_repo ({max}), but the rest are annotated",
            highlights.len()
        );
    }
}

/// Every load-time rule, applied to an already-parsed document.
///
/// `root` must already be canonical — [`load`] does that. Nothing in here
/// touches the filesystem, which is the point: each rule below is a security
/// or integrity decision, and each one is reachable from a unit test with a
/// hand-built `MarksDoc`.
///
/// In order:
/// 1. `doc.root` must equal `root`. A mismatch refuses the **whole** document
///    rather than filtering it, because it means the file belongs to a
///    different repo — a hash collision, a copied `~/.config`, or a rename.
/// 2. Every `file_path` must be absolute, free of `..`, and inside `root`
///    ([`guard::inside_root`]).
/// 3. `quote`, `prefix` and `suffix` are truncated to [`MAX_FIELD_BYTES`].
/// 4. A mark with a blank quote is dropped: it can never re-anchor, and the
///    whitespace-stripped tier of `markdown::locate` would match it anywhere.
/// 5. The per-repo cap applies ([`enforce_cap`]).
/// 6. Stack entries with no surviving highlight are dropped, and the stack is
///    de-duplicated — `set_annotation` guarantees uniqueness in memory, and a
///    hand-edited file must not be able to queue the same evidence twice.
pub fn admit(root: &Path, doc: MarksDoc) -> Store {
    if Path::new(&doc.root) != root {
        eprintln!(
            "dreamd: ignoring marks for {:?}; this session's root is {}",
            doc.root,
            root.display()
        );
        return Store::default();
    }

    let mut highlights: Vec<Highlight> = doc
        .highlights
        .into_iter()
        .filter_map(|mut h| {
            if !path_is_admissible(root, &h.file_path) {
                eprintln!(
                    "dreamd: dropping a mark for {} — outside {}",
                    h.file_path,
                    root.display()
                );
                return None;
            }
            truncate_at(&mut h.quote, MAX_FIELD_BYTES);
            truncate_at(&mut h.prefix, MAX_FIELD_BYTES);
            truncate_at(&mut h.suffix, MAX_FIELD_BYTES);
            if h.quote.trim().is_empty() {
                return None;
            }
            // Coming off disk is what "previous session" *means*. `prior` is
            // `#[serde(skip)]`, so it is false on arrival no matter what the
            // file said, and this is the only place it is ever set.
            h.prior = true;
            Some(h)
        })
        .collect();

    enforce_cap(&mut highlights, MAX_PER_REPO);

    let mut seen = std::collections::HashSet::new();
    let stack: Vec<Id> = doc
        .stack
        .into_iter()
        .filter(|id| highlights.iter().any(|h| &h.id == id) && seen.insert(id.clone()))
        .collect();

    Store::from_parts(highlights, stack)
}

// ---- I/O -----------------------------------------------------------------

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Project a store into the document that would be written for `root`.
///
/// Split out from [`save`] so a round trip can be exercised without a
/// directory: build the doc, serialise it, parse it, hand it to [`admit`].
///
/// `prior` is cleared on the way out. It answers "was this read from a file",
/// which is a fact about *this* session and means nothing to the next one —
/// [`admit`] decides it again on load. The copy is free: `to_vec` was cloning
/// already.
pub fn doc_from(canonical_root: &Path, store: &Store) -> MarksDoc {
    let (highlights, stack) = store.parts();
    MarksDoc {
        version: VERSION,
        root: canonical_root.to_string_lossy().into_owned(),
        saved_at: now_secs(),
        highlights: highlights
            .iter()
            .map(|h| Highlight {
                prior: false,
                ..h.clone()
            })
            .collect(),
        stack: stack.to_vec(),
    }
}

/// Read the marks for `root`. **Never fails and never panics** — every failure
/// path warns on stderr and yields an empty store, the convention `config.rs`
/// establishes throughout. Losing a session's marks is bad; refusing to start
/// because of a corrupt cache file is worse.
pub fn load(root: &Path) -> Store {
    let root = canonical(root);
    let path = dir().join(file_name_for(&root));

    let meta = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Store::default(),
        Err(e) => {
            eprintln!("dreamd: cannot stat {}: {e}", path.display());
            return Store::default();
        }
    };
    // Checked before reading, not after: the point is not to pull 4 MiB of
    // someone's mistake into memory on the startup path.
    if meta.len() > MAX_FILE_BYTES {
        eprintln!(
            "dreamd: ignoring {} — {} bytes exceeds the {MAX_FILE_BYTES} byte cap",
            path.display(),
            meta.len()
        );
        return Store::default();
    }

    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("dreamd: cannot read {}: {e}", path.display());
            return Store::default();
        }
    };
    match serde_json::from_str::<MarksDoc>(&text) {
        Ok(doc) => admit(&root, doc),
        Err(e) => {
            eprintln!("dreamd: ignoring unparseable {}: {e}", path.display());
            Store::default()
        }
    }
}

/// Write the marks for `root`.
///
/// Mirrors `config::write_global`'s `create_dir_all` → serialise → `.tmp`
/// sibling → `rename` shape, so a crash mid-write leaves the previous file
/// intact rather than a truncated one. The addition here is the mode: `0700` on
/// the directory and `0600` on the temp file **before** any bytes reach it, set
/// via `OpenOptionsExt` rather than a `set_permissions` afterwards, because the
/// window between the two is exactly when another user could open it. Marks
/// carry quoted document text and the questions the user asked about it.
///
/// (`write_global` sets no mode today. That is arguably a bug there too, but
/// fixing it belongs in its own commit rather than entangled with this one.)
pub fn save(root: &Path, store: &Store) -> io::Result<()> {
    let root = canonical(root);
    let dir = dir();
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }

    let path = dir.join(file_name_for(&root));
    let body = serde_json::to_string(&doc_from(&root, store))
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    let tmp = path.with_extension("json.tmp");
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    {
        use std::io::Write;
        let mut file = opts.open(&tmp)?;
        file.write_all(body.as_bytes())?;
    }
    std::fs::rename(&tmp, &path)
}

#[cfg(test)]
mod tests {
    //! Nothing here touches `config_dir()` or a real directory: `admit` holds
    //! every load-time rule, and the path tests run against `file_name_for`,
    //! which takes an already-canonical root. The filesystem side — file mode,
    //! directory mode, a corrupt file, an orphaned `.tmp` — is
    //! `examples/marks_check.rs`'s job, because it can sandbox
    //! `XDG_CONFIG_HOME` for the whole process without racing tests running in
    //! parallel threads.
    use super::*;

    const ROOT: &str = "/w/notes";

    fn root() -> PathBuf {
        PathBuf::from(ROOT)
    }

    fn mark(id: &str, file_path: &str, quote: &str) -> Highlight {
        Highlight {
            id: id.to_string(),
            file_path: file_path.to_string(),
            line_start: 1,
            line_end: 1,
            quote: quote.to_string(),
            ..Highlight::default()
        }
    }

    fn doc(highlights: Vec<Highlight>, stack: &[&str]) -> MarksDoc {
        MarksDoc {
            version: VERSION,
            root: ROOT.to_string(),
            saved_at: 1_753_617_600,
            highlights,
            stack: stack.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    // ---- the root claim ---------------------------------------------------

    #[test]
    fn a_file_whose_root_disagrees_is_refused_wholesale() {
        // Not filtered — refused. A root mismatch means the file belongs to
        // another repo (a hash collision, a copied ~/.config, a rename), and
        // adopting half of it would be worse than adopting none: the marks
        // would quote text from documents this session cannot see.
        let mut d = doc(vec![mark("h1", "/w/notes/a.md", "alpha")], &["h1"]);
        d.root = "/w/other".to_string();
        let store = admit(&root(), d);
        assert!(store.parts().0.is_empty());
        assert!(store.stack_pairs().is_empty());

        // A document with no root claim at all is refused for the same reason,
        // rather than being taken as "belongs to whoever is asking".
        let mut d = doc(vec![mark("h1", "/w/notes/a.md", "alpha")], &[]);
        d.root = String::new();
        assert!(admit(&root(), d).parts().0.is_empty());
    }

    #[test]
    fn a_matching_root_is_admitted() {
        let store = admit(
            &root(),
            doc(vec![mark("h1", "/w/notes/a.md", "alpha")], &[]),
        );
        assert_eq!(store.parts().0.len(), 1);
    }

    // ---- containment (security-shaped) ------------------------------------

    #[test]
    fn a_highlight_outside_the_repo_root_is_dropped_on_load() {
        // Break-the-guard applies: delete the `guard::inside_root` call in
        // `path_is_admissible` and this test must go red.
        //
        // `/w/notes-private` is the case `Path::starts_with` exists for — a
        // textual prefix check would admit it, because it never reaches a
        // separator.
        let store = admit(
            &root(),
            doc(
                vec![
                    mark("h1", "/w/notes/a.md", "kept"),
                    mark("h2", "/etc/passwd", "root:x:0:0"),
                    mark("h3", "/w/notes-private/secret.md", "sibling prefix"),
                    mark("h4", "/w", "the parent"),
                    mark("h5", "relative/a.md", "not absolute"),
                    mark("h6", "", "no path at all"),
                ],
                &["h1", "h2", "h3", "h4", "h5", "h6"],
            ),
        );
        let (highlights, stack) = store.parts();
        assert_eq!(
            highlights.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(),
            vec!["h1"],
            "only the mark inside the root survives"
        );
        // ...and the stack cannot keep pointing at what the filter removed.
        assert_eq!(stack, ["h1"]);
    }

    #[test]
    fn a_highlight_that_escapes_by_dot_dot_is_dropped_on_load() {
        // `inside_root` alone is not enough here, and that is not a bug in it:
        // `/w/notes/../../etc/passwd` genuinely *does* start with `/w/notes`
        // component-wise. `inside_root` documents that resolving `..` is the
        // caller's job, and this module cannot canonicalize — a marks file
        // routinely names files that were deleted since, and canonicalize
        // fails on exactly those. So `..` is refused outright.
        let store = admit(
            &root(),
            doc(
                vec![
                    mark("h1", "/w/notes/../../etc/passwd", "escaped"),
                    mark("h2", "/w/notes/sub/../a.md", "harmless but still refused"),
                ],
                &["h1", "h2"],
            ),
        );
        assert!(
            store.parts().0.is_empty(),
            "no path containing `..` is admitted, escaping or not"
        );
    }

    // ---- field hygiene ----------------------------------------------------

    #[test]
    fn an_overlong_quote_is_truncated_rather_than_rejected() {
        // Truncated, because the mark is still the user's: a shortened quote
        // may go Stale on the next reanchor, which is visible and recoverable,
        // whereas dropping it is silent.
        let mut h = mark("h1", "/w/notes/a.md", "");
        h.quote = "q".repeat(MAX_FIELD_BYTES + 500);
        h.prefix = "p".repeat(MAX_FIELD_BYTES + 1);
        h.suffix = "s".repeat(10);
        let store = admit(&root(), doc(vec![h], &[]));
        let kept = &store.parts().0[0];
        assert_eq!(kept.quote.len(), MAX_FIELD_BYTES);
        assert_eq!(kept.prefix.len(), MAX_FIELD_BYTES);
        assert_eq!(kept.suffix.len(), 10, "a short field is left alone");
    }

    #[test]
    fn truncation_lands_on_a_char_boundary() {
        // Multi-byte text is the common case in prose, and `String::truncate`
        // panics on a boundary miss — in `admit`, on the startup path.
        let mut h = mark("h1", "/w/notes/a.md", "");
        // 3 bytes each, so the cap falls mid-character. It has to be 3: this
        // test used `é`, which is *2* bytes, and `MAX_FIELD_BYTES` is even — so
        // the cap always landed on a boundary and a naive `String::truncate`
        // passed it. Verified the other way round: with a 3-byte character and
        // the boundary walk removed, this panics.
        h.quote = "あ".repeat(MAX_FIELD_BYTES);
        let store = admit(&root(), doc(vec![h], &[]));
        let quote = &store.parts().0[0].quote;
        assert!(quote.len() <= MAX_FIELD_BYTES);
        assert!(quote.len() > MAX_FIELD_BYTES - 4, "lost more than one char");
    }

    #[test]
    fn a_mark_with_a_blank_quote_is_dropped() {
        // It can never re-anchor: `locate`'s whitespace-stripped tier would
        // match an empty quote at the first byte of any file, so keeping it
        // means a highlight that silently wanders.
        let store = admit(
            &root(),
            doc(
                vec![
                    mark("h1", "/w/notes/a.md", ""),
                    mark("h2", "/w/notes/a.md", "   \n\t "),
                    mark("h3", "/w/notes/a.md", "real"),
                ],
                &["h1", "h2", "h3"],
            ),
        );
        assert_eq!(
            store
                .parts()
                .0
                .iter()
                .map(|h| h.id.as_str())
                .collect::<Vec<_>>(),
            vec!["h3"]
        );
    }

    // ---- prior, and pending -----------------------------------------------

    #[test]
    fn everything_that_comes_off_disk_is_prior() {
        // The entire implementation of "highlighted in a previous session".
        // There is no clock and no session id — being admitted *is* the
        // evidence, because the only thing `admit` ever reads is a file another
        // session wrote.
        let store = admit(
            &root(),
            doc(
                vec![
                    mark("h1", "/w/notes/a.md", "alpha"),
                    mark("h2", "/w/notes/b.md", "beta"),
                ],
                &["h1"],
            ),
        );
        assert!(
            store.parts().0.iter().all(|h| h.prior),
            "{:?}",
            store.parts().0
        );

        // ...and a mark minted in this session is not, so the flag actually
        // discriminates rather than being true everywhere.
        let mut fresh = Store::default();
        let id = fresh.add_highlight(
            "/w/notes/a.md".to_string(),
            1,
            1,
            "alpha".to_string(),
            String::new(),
            String::new(),
        );
        assert!(!fresh.get(&id).expect("present").prior);
    }

    #[test]
    fn prior_never_reaches_the_file() {
        // Asserted against the serialized *text*, not a round trip: a round
        // trip goes back through `admit`, which sets `prior` again and would
        // hide the bug completely.
        //
        // The store is one that came off disk, so every mark in it *is* prior —
        // the case that actually exercises `doc_from`'s strip. A store of marks
        // made this session would pass no matter what, because the flag would be
        // false anyway.
        let mut store = admit(
            &root(),
            doc(vec![mark("h1", "/w/notes/a.md", "alpha")], &[]),
        );
        assert!(
            store.get("h1").expect("admitted").prior,
            "premise: it must still be prior when the document is built, so it \
             must not be annotated here — annotating clears the flag (D13) and \
             this test would then pass whatever `doc_from` does"
        );
        // ...and one made this session, sent, so the persisted half of the pair
        // is in the same document.
        let fresh = store.add_highlight(
            "/w/notes/a.md".to_string(),
            1,
            1,
            "beta".to_string(),
            String::new(),
            String::new(),
        );
        store.set_annotation(&fresh, "and?".into());
        store.mark_sent(std::slice::from_ref(&fresh));

        let json = serde_json::to_string(&doc_from(&root(), &store)).expect("serialize");
        assert!(
            !json.contains("prior"),
            "a flag about this session's reading was written to disk: {json}"
        );
        // The other half of the pair *is* persisted — pending has to survive a
        // restart — so this also pins that the two fields were not confused.
        assert!(json.contains("sent_at"), "{json}");
    }

    #[test]
    fn prior_still_reaches_the_frontend() {
        // The other side of the same coin, and the reason `prior` is not a plain
        // `#[serde(skip)]`: one derive serves the marks file *and* the IPC reply
        // that `get_highlights` returns, so a skip that keeps the flag off disk
        // also keeps it away from the code that paints the fade. This test is
        // what fails if someone simplifies the attributes back.
        let store = admit(
            &root(),
            doc(vec![mark("h1", "/w/notes/a.md", "alpha")], &[]),
        );
        let h = store.get("h1").expect("admitted");
        let json = serde_json::to_string(&h).expect("serialize");
        assert!(json.contains(r#""prior":true"#), "{json}");

        // ...and false stays off the wire, so the frontend must read an absent
        // key as false.
        let mut fresh = Store::default();
        let id = fresh.add_highlight(
            "/w/notes/a.md".to_string(),
            1,
            1,
            "beta".to_string(),
            String::new(),
            String::new(),
        );
        let json = serde_json::to_string(&fresh.get(&id).expect("present")).expect("serialize");
        assert!(!json.contains("prior"), "{json}");
    }

    #[test]
    fn a_file_written_before_pending_existed_loads_without_it() {
        // Container-wide `#[serde(default)]` is what makes this work, and the
        // reverse direction — a file *with* `sent_at` loading in a build that
        // predates the field — is the same attribute plus the absence of
        // `deny_unknown_fields`, already pinned by
        // `a_file_from_a_future_version_loads_what_it_understands`.
        let json = r#"{
            "version": 1,
            "root": "/w/notes",
            "saved_at": 1753617600,
            "highlights": [
              {"id":"h1","file_path":"/w/notes/a.md","quote":"alpha",
               "annotation":"why?","state":"active"}
            ],
            "stack": ["h1"]
        }"#;
        let parsed: MarksDoc = serde_json::from_str(json).expect("an older file still parses");
        let store = admit(&root(), parsed);
        let h = store.get("h1").expect("admitted");
        assert_eq!(h.sent_at, None);
        assert!(h.prior, "it still came off disk");
        assert_eq!(store.stack_pairs().len(), 1);
    }

    #[test]
    fn a_pending_stamp_survives_a_round_trip() {
        let mut store = Store::default();
        let id = store.add_highlight(
            "/w/notes/a.md".to_string(),
            1,
            1,
            "alpha".to_string(),
            String::new(),
            String::new(),
        );
        store.set_annotation(&id, "why?".into());
        store.mark_sent(std::slice::from_ref(&id));
        let sent_at = store.get(&id).expect("present").sent_at;

        let json = serde_json::to_string(&doc_from(&root(), &store)).expect("serialize");
        let back = admit(&root(), serde_json::from_str(&json).expect("deserialize"));

        assert_eq!(back.get(&id).expect("present").sent_at, sent_at);
        assert!(
            back.stack_pairs().is_empty(),
            "a sent mark left the stack before the file was written"
        );
    }

    // ---- forward compatibility --------------------------------------------

    #[test]
    fn a_file_from_a_future_version_loads_what_it_understands() {
        // The test that stops step 5 breaking step 4's files. An unknown
        // top-level key and an unknown entry field must both be ignored, and a
        // version number from the future must not be a rejection.
        let json = r#"{
            "version": 7,
            "root": "/w/notes",
            "saved_at": 1753617600,
            "invented_by_a_later_version": {"a": 1},
            "highlights": [
              {"id":"h1","file_path":"/w/notes/a.md","quote":"alpha",
               "annotation":"why?","also_invented":42}
            ],
            "stack": ["h1"]
        }"#;
        let parsed: MarksDoc = serde_json::from_str(json).expect("unknown keys are ignored");
        assert_eq!(parsed.version, 7);
        let store = admit(&root(), parsed);
        assert_eq!(store.parts().0.len(), 1);
        assert_eq!(store.stack_pairs().len(), 1, "the stack survives too");

        // ...and the other direction: a document missing everything optional
        // still parses rather than erroring.
        let sparse: MarksDoc = serde_json::from_str(r#"{"root":"/w/notes"}"#).expect("defaults");
        assert_eq!(sparse.version, 0);
        assert!(admit(&root(), sparse).parts().0.is_empty());
    }

    // ---- round trip -------------------------------------------------------

    #[test]
    fn a_round_trip_preserves_stack_order() {
        // Stack order is the order the human asked their questions in, which is
        // the order the agent should answer them; reload must not reorder it.
        // Through real JSON rather than a struct copy, so a serde attribute
        // that drops a field fails here.
        let mut store = Store::default();
        let source = "alpha\nbeta\ngamma\n";
        let ids: Vec<Id> = ["alpha", "beta", "gamma"]
            .iter()
            .map(|q| {
                store.add_anchored(
                    "/w/notes/a.md".to_string(),
                    source,
                    (*q).to_string(),
                    String::new(),
                    String::new(),
                    crate::annotations::Origin::Human,
                )
            })
            .collect();
        // Queued third, then first — deliberately not insertion order.
        store.set_annotation(&ids[2], "why gamma?".into());
        store.set_annotation(&ids[0], "why alpha?".into());

        let json = serde_json::to_string(&doc_from(&root(), &store)).expect("serialize");
        let back = admit(&root(), serde_json::from_str(&json).expect("deserialize"));

        assert_eq!(
            back.stack_pairs()
                .iter()
                .map(|p| p.highlight.id.clone())
                .collect::<Vec<_>>(),
            vec![ids[2].clone(), ids[0].clone()]
        );
        assert_eq!(back.stack_pairs()[0].annotation, "why gamma?");
        assert_eq!(back.parts().0.len(), 3, "the unannotated mark survives too");
        assert_eq!(back.get(&ids[1]).expect("present").line_start, 2);
    }

    #[test]
    fn a_stack_entry_with_no_highlight_is_dropped_and_duplicates_collapse() {
        let mut d = doc(
            vec![
                mark("h1", "/w/notes/a.md", "alpha"),
                mark("h2", "/w/notes/a.md", "beta"),
            ],
            &["h2", "hffffffffffffffff", "h1", "h2"],
        );
        for h in &mut d.highlights {
            h.annotation = Some("why?".into());
        }
        let store = admit(&root(), d);
        assert_eq!(
            store
                .stack_pairs()
                .iter()
                .map(|p| p.highlight.id.clone())
                .collect::<Vec<_>>(),
            vec!["h2".to_string(), "h1".to_string()],
            "order kept, the phantom dropped, the repeat collapsed"
        );
    }

    // ---- the path hash ----------------------------------------------------

    #[test]
    fn the_path_for_two_roots_differs_and_is_stable() {
        // THE hardcoded-hex test. This is what catches someone swapping FNV-1a
        // for `DefaultHasher`: `std`'s hasher output is explicitly undefined
        // across releases, so a toolchain bump would rename every user's marks
        // file and orphan their work with no error anywhere. If this test goes
        // red, do not update the constants — find out what changed the hash.
        assert_eq!(
            file_name_for(Path::new("/w/notes")),
            "notes-fb1b4f606969c7f1.json"
        );
        assert_eq!(
            file_name_for(Path::new("/w/notes-private")),
            "notes-private-f8ef2532d1b2f04f.json"
        );
        assert_eq!(root_hash(Path::new("/w/other")), "63fbdf4f7ed6599e");

        // Sibling roots sharing a prefix must not share a file — the failure
        // this naming scheme exists to prevent.
        assert_ne!(
            file_name_for(Path::new("/w/notes")),
            file_name_for(Path::new("/w/notes-private"))
        );
        // ...and it is a function of the whole path, not the basename.
        assert_ne!(
            file_name_for(Path::new("/a/notes")),
            file_name_for(Path::new("/b/notes"))
        );
    }

    #[test]
    fn a_basename_is_reduced_to_safe_characters() {
        // Cosmetic — the hash identifies the repo — but a slash or a newline in
        // it would be a path-traversal seam of its own.
        assert!(file_name_for(Path::new("/w/my notes (2)")).starts_with("my-notes--2--"));
        assert!(file_name_for(Path::new("/")).starts_with("repo-"));
        assert!(file_name_for(Path::new("/w/..")).starts_with("repo-"));
        assert!(
            !file_name_for(Path::new("/w/a/../../etc")).contains('/'),
            "no separator can reach the name"
        );
    }

    // ---- the cap ----------------------------------------------------------

    #[test]
    fn the_cap_drops_stale_unannotated_marks_before_anything_else() {
        // Priority order: stale-and-unannotated, then active-and-unannotated,
        // and never an annotated mark — an annotated one is a question in
        // flight, and a stale one is usually mid-`git checkout` and about to
        // come back.
        let mut highlights = Vec::new();
        let mut push = |id: &str, state: HighlightState, annotated: bool| {
            let mut h = mark(id, "/w/notes/a.md", "q");
            h.state = state;
            h.annotation = annotated.then(|| "why?".to_string());
            highlights.push(h);
        };
        push("stale_old", HighlightState::Stale, false);
        push("annotated_stale", HighlightState::Stale, true);
        push("active_a", HighlightState::Active, false);
        push("stale_new", HighlightState::Stale, false);
        push("active_b", HighlightState::Active, false);
        push("annotated_active", HighlightState::Active, true);

        let mut four = highlights.clone();
        enforce_cap(&mut four, 4);
        assert_eq!(
            four.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(),
            vec![
                "annotated_stale",
                "active_a",
                "active_b",
                "annotated_active"
            ],
            "both stale unannotated marks went, oldest first, and nothing else"
        );

        // Squeeze harder and the active unannotated ones go next, oldest
        // first — still not the annotated pair.
        let mut two = highlights.clone();
        enforce_cap(&mut two, 2);
        assert_eq!(
            two.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(),
            vec!["annotated_stale", "annotated_active"]
        );

        // Below the number of annotated marks the cap yields rather than
        // deleting a question the user asked. Staying over the cap is the
        // designed outcome, not a bug.
        let mut one = highlights.clone();
        enforce_cap(&mut one, 1);
        assert_eq!(one.len(), 2, "annotated marks are never dropped");

        // And under the cap it is a no-op.
        let mut under = highlights.clone();
        enforce_cap(&mut under, 100);
        assert_eq!(under.len(), 6);
    }

    #[test]
    fn admit_applies_the_cap_and_then_prunes_the_stack() {
        // Order matters: a mark the cap removes must not leave a dangling
        // stack entry behind it.
        let mut highlights: Vec<Highlight> = (0..MAX_PER_REPO + 5)
            .map(|i| {
                let mut h = mark(&format!("h{i}"), "/w/notes/a.md", "q");
                h.state = HighlightState::Stale;
                h
            })
            .collect();
        highlights[0].annotation = Some("survives".into());
        let ids: Vec<&str> = vec!["h0", "h1"];
        let store = admit(&root(), doc(highlights, &ids));
        assert_eq!(store.parts().0.len(), MAX_PER_REPO);
        assert!(store.get("h0").is_some(), "the annotated mark is kept");
        assert!(store.get("h1").is_none(), "an oldest stale mark went");
        assert_eq!(store.parts().1, ["h0"], "the stack followed");
    }
}
