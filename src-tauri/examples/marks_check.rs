//! Correctness harness for the marks file: real directories, real modes, real
//! corrupt files. Exits non-zero on the first failure.
//!
//! It is an example rather than a `#[cfg(test)]` module for the reason
//! `CLAUDE.md` gives: `marks_file::dir()` reads `config_dir()`, which reads the
//! real `~/.config/dreamd`, and `cargo test` is not allowed anywhere near it.
//! Here `XDG_CONFIG_HOME` is repointed process-wide before anything runs — the
//! same trick `config_check.rs` and `mcp_check.rs` use, and the reason all
//! three are examples rather than tests.
//!
//! What is *not* here: every rule `marks_file`'s own unit tests already reach
//! through `admit` with a hand-built document — the root claim, containment,
//! truncation, the cap. Those need no directory and are tested where they
//! belong. This harness exists for what only a real file can show: the modes,
//! an unparseable file on the startup path, the size cap that is checked
//! against `metadata()` rather than content, the `.tmp` sibling a crash leaves
//! behind, and the bind-is-the-lock rule that decides which of two dreamds may
//! write at all.
//!
//! ```sh
//! cargo run --example marks_check
//! ```

use dreamd::annotations::{Id, Origin, Store};
use dreamd::marks_file;
use dreamd::mcp::server::{self, Bound};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

const A: &str = "alpha\nbeta\ngamma\ndelta\n";

fn main() {
    // Short on purpose: this path becomes `…/xdg/dreamd/run/<16hex>.sock`, and
    // macOS caps `sun_path` at ~104 bytes.
    let root = std::env::temp_dir().join("dreamd-marks");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("temp dir");
    // Before anything reaches `config_dir()`.
    std::env::set_var("XDG_CONFIG_HOME", root.join("xdg"));

    let repo = root.join("repo");
    std::fs::create_dir_all(repo.join("docs")).expect("repo dir");
    std::fs::write(repo.join("docs/a.md"), A).expect("a.md");
    let repo = repo.canonicalize().expect("canonical repo");
    let other = root.join("other");
    std::fs::create_dir_all(&other).expect("other dir");
    let other = other.canonicalize().expect("canonical other");

    let mut checks = Checks::default();

    paths(&mut checks, &repo, &other, &root);
    round_trip(&mut checks, &repo);
    modes(&mut checks, &repo);
    hostile_files(&mut checks, &repo);
    crash_artifacts(&mut checks, &repo);
    two_repos(&mut checks, &repo, &other);
    the_lock(&mut checks, &repo, &other);
    // Last: it moves the process's working directory, which is how `cli::run`
    // decides which repo it is talking about.
    prune(&mut checks, &repo);

    let _ = std::fs::remove_dir_all(&root);
    checks.finish();
}

// ---- fixtures ------------------------------------------------------------

/// Three marks in one file, two of them queued in a deliberately non-insertion
/// order — the acceptance scenario in miniature, and what a reload has to
/// reproduce exactly.
fn seeded(repo: &Path) -> (Store, Vec<Id>) {
    let file = repo.join("docs/a.md").to_string_lossy().into_owned();
    let mut store = Store::default();
    let ids: Vec<Id> = ["alpha", "beta", "gamma"]
        .iter()
        .map(|quote| {
            store.add_anchored(
                file.clone(),
                A,
                (*quote).to_string(),
                String::new(),
                String::new(),
                Origin::Human,
            )
        })
        .collect();
    store.set_annotation(&ids[2], "why gamma?".into());
    store.set_annotation(&ids[0], "why alpha?".into());
    (store, ids)
}

fn write_raw(repo: &Path, body: &str) {
    let path = marks_file::path_for(repo);
    std::fs::create_dir_all(path.parent().expect("marks dir")).expect("marks dir");
    std::fs::write(&path, body).expect("write raw marks");
}

fn mode_of(path: &Path) -> u32 {
    std::fs::metadata(path)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o777
}

fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("read marks dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

// ---- checks --------------------------------------------------------------

fn paths(checks: &mut Checks, repo: &Path, other: &Path, sandbox: &Path) {
    let path = marks_file::path_for(repo);
    checks.ok(
        "the marks file lands under XDG_CONFIG_HOME",
        path.starts_with(sandbox.join("xdg")),
    );
    checks.ok(
        "…in a directory called marks",
        path.parent() == Some(marks_file::dir().as_path()),
    );
    checks.eq(
        "the name is basename + FNV hash",
        path.file_name().unwrap().to_string_lossy(),
        marks_file::file_name_for(repo),
    );
    checks.ok(
        "two repos do not share a file",
        marks_file::path_for(repo) != marks_file::path_for(other),
    );
    // The reason `path_for` canonicalises: `dreamd .` and `dreamd /abs/path`
    // are the same repo and must not end up with two files.
    let dotted = repo.join("docs").join("..");
    checks.eq(
        "an uncanonical root resolves to the same file",
        marks_file::path_for(&dotted),
        path.clone(),
    );
    checks.ok(
        "a repo with nothing saved loads empty",
        marks_file::load(repo).parts().0.is_empty(),
    );
}

fn round_trip(checks: &mut Checks, repo: &Path) {
    let (mut store, ids) = seeded(repo);
    // A resolved mark, so the field an agent writes survives the trip too.
    store.resolve(&ids[0], Some("answered".into()));
    marks_file::save(repo, &store).expect("save");

    let back = marks_file::load(repo);
    let (highlights, stack) = back.parts();
    checks.eq("every mark comes back", highlights.len(), 3);
    checks.eq(
        "ids are unchanged — step 1's payoff, no counter in the file",
        highlights.iter().map(|h| h.id.clone()).collect::<Vec<_>>(),
        ids.clone(),
    );
    checks.eq(
        "the stack keeps the order the questions were asked in",
        stack.to_vec(),
        vec![ids[2].clone()],
    );

    let alpha = back.get(&ids[0]).expect("alpha");
    checks.eq("the quote survives", alpha.quote, "alpha");
    checks.eq("line numbers survive", alpha.line_start, 1);
    checks.eq(
        "the annotation survives",
        alpha.annotation,
        Some("why alpha?".to_string()),
    );
    checks.ok(
        "the resolution survives, note and all",
        alpha.resolved.as_ref().map(|r| r.note.as_deref()) == Some(Some("answered")),
    );
    checks.ok(
        "…with its timestamp",
        alpha.resolved.as_ref().is_some_and(|r| r.at > 0),
    );

    let beta = back.get(&ids[1]).expect("beta");
    checks.eq("an unannotated mark survives too", beta.annotation, None);
    checks.eq("line numbers are per mark", beta.line_start, 2);
    checks.ok(
        "…and it is not on the stack",
        !stack.iter().any(|id| id == &ids[1]),
    );

    // An agent's own mark, written and read back with its origin intact: the
    // frontend is free to paint it differently, and it can only do that if the
    // field round-trips.
    let mut store = Store::default();
    let id = store.add_anchored(
        repo.join("docs/a.md").to_string_lossy().into_owned(),
        A,
        "delta".to_string(),
        String::new(),
        String::new(),
        Origin::Agent,
    );
    marks_file::save(repo, &store).expect("save agent mark");
    let back = marks_file::load(repo);
    checks.ok(
        "an agent's origin survives",
        matches!(back.get(&id).map(|h| h.origin), Some(Origin::Agent)),
    );
    checks.eq(
        "a save replaces the file rather than appending to it",
        back.parts().0.len(),
        1,
    );

    // An empty store still writes: it is how "I deleted my last mark" is
    // recorded, and a skipped write would resurrect it on the next boot.
    marks_file::save(repo, &Store::default()).expect("save empty");
    checks.ok(
        "an emptied store is persisted as empty, not skipped",
        marks_file::load(repo).parts().0.is_empty(),
    );
}

fn modes(checks: &mut Checks, repo: &Path) {
    let dir = marks_file::dir();
    // Loosen both first: the interesting question is whether `save` tightens
    // them, not whether it happened to create them tight once.
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).expect("loosen dir");
    let path = marks_file::path_for(repo);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("loosen file");

    let (store, _) = seeded(repo);
    marks_file::save(repo, &store).expect("save");

    checks.eq("the marks directory is 0700", mode_of(&dir), 0o700);
    checks.eq("the marks file is 0600", mode_of(&path), 0o600);

    // The mode is set on the temp file *before* any bytes reach it, which is
    // the whole reason `save` uses `OpenOptionsExt` rather than a
    // `set_permissions` afterwards: marks carry quoted document text and the
    // questions the user asked about it, and the gap between create and chmod
    // is exactly when another user could open it. What is observable after the
    // fact is that the rename preserved 0600 rather than the umask's default.
    checks.ok(
        "no temp sibling is left behind",
        !path.with_extension("json.tmp").exists(),
    );
    checks.eq(
        "…and the directory holds only the marks file",
        entries(&dir),
        vec![marks_file::file_name_for(repo)],
    );

    // The directory is created on demand, still at 0700.
    std::fs::remove_dir_all(&dir).expect("remove marks dir");
    marks_file::save(repo, &store).expect("save into a missing directory");
    checks.eq("a recreated directory is 0700", mode_of(&dir), 0o700);
    checks.eq("a recreated file is 0600", mode_of(&path), 0o600);
}

fn hostile_files(checks: &mut Checks, repo: &Path) {
    let path = marks_file::path_for(repo);
    let file = repo.join("docs/a.md").to_string_lossy().into_owned();

    // Every one of these must yield an empty store rather than a panic: this
    // runs before the window exists, and refusing to start over a cache file
    // is worse than losing what it held.
    for (what, body) in [
        ("truncated json", "{\"version\":1,\"root\":\"".to_string()),
        ("not json at all", "corrupt\0\u{1}nonsense".to_string()),
        ("an empty file", String::new()),
        // Not the no-op it looks like: serde will build a struct out of a
        // *sequence*, so `[]` parses cleanly into an all-defaults `MarksDoc`
        // and it is `admit`'s root check — not the parser — that refuses it.
        ("a json array", "[]".to_string()),
        ("a json string", "\"marks\"".to_string()),
    ] {
        write_raw(repo, &body);
        checks.ok(
            &format!("{what} loads empty and does not panic"),
            marks_file::load(repo).parts().0.is_empty(),
        );
        checks.ok(
            &format!("…and {what} is left on disk to inspect"),
            path.exists(),
        );
    }

    // The size cap is checked against `metadata()` before the read, so it must
    // hold for a file that is perfectly valid JSON — the point is not to pull
    // 4 MiB of someone's mistake into memory on the startup path.
    let filler = "x".repeat(marks_file::MAX_FILE_BYTES as usize + 1024);
    write_raw(
        repo,
        &format!(
            "{{\"version\":1,\"root\":{:?},\"highlights\":[{{\"id\":\"h1\",\"file_path\":{:?},\"quote\":{:?}}}]}}",
            repo.to_string_lossy(),
            file,
            filler
        ),
    );
    checks.ok(
        "a file over the size cap is refused",
        marks_file::load(repo).parts().0.is_empty(),
    );
    checks.ok("…without deleting it", path.exists());

    // A real file carrying the escape `admit` refuses. The unit tests cover
    // the rule; this covers the rule surviving serde on the way in.
    write_raw(
        repo,
        &format!(
            "{{\"version\":1,\"root\":{:?},\"highlights\":[
               {{\"id\":\"h1\",\"file_path\":{:?},\"quote\":\"alpha\"}},
               {{\"id\":\"h2\",\"file_path\":\"/etc/passwd\",\"quote\":\"root:x:0:0\"}},
               {{\"id\":\"h3\",\"file_path\":{:?},\"quote\":\"escaped\"}}
             ],\"stack\":[\"h1\",\"h2\",\"h3\"]}}",
            repo.to_string_lossy(),
            file,
            format!("{}/docs/../../etc/passwd", repo.to_string_lossy()),
        ),
    );
    let store = marks_file::load(repo);
    checks.eq(
        "only the contained mark survives a hand-edited file",
        store
            .parts()
            .0
            .iter()
            .map(|h| h.id.clone())
            .collect::<Vec<_>>(),
        vec!["h1".to_string()],
    );
    checks.eq(
        "…and the stack cannot keep pointing at what went",
        store.parts().1.to_vec(),
        vec!["h1".to_string()],
    );

    // A file from a later dreamd: unknown keys at both levels, a version this
    // build never wrote. It must load what it understands rather than refuse —
    // the whole reason there is no `deny_unknown_fields`.
    write_raw(
        repo,
        &format!(
            "{{\"version\":99,\"root\":{:?},\"invented_later\":{{\"a\":1}},\"highlights\":[
               {{\"id\":\"h1\",\"file_path\":{:?},\"quote\":\"alpha\",\"annotation\":\"why?\",\"also_new\":42}}
             ],\"stack\":[\"h1\"]}}",
            repo.to_string_lossy(),
            file,
        ),
    );
    let store = marks_file::load(repo);
    checks.eq(
        "a future file still loads its marks",
        store.parts().0.len(),
        1,
    );
    checks.eq("…and its stack", store.stack_pairs().len(), 1);

    // A file belonging to another repo — a copied `~/.config`, or a hash
    // collision. Refused wholesale, not filtered.
    write_raw(
        repo,
        &format!(
            "{{\"version\":1,\"root\":\"/somewhere/else\",\"highlights\":[
               {{\"id\":\"h1\",\"file_path\":{file:?},\"quote\":\"alpha\"}}
             ]}}"
        ),
    );
    checks.ok(
        "a file claiming another root is refused wholesale",
        marks_file::load(repo).parts().0.is_empty(),
    );
}

fn crash_artifacts(checks: &mut Checks, repo: &Path) {
    let path = marks_file::path_for(repo);
    let tmp = path.with_extension("json.tmp");

    let (store, ids) = seeded(repo);
    marks_file::save(repo, &store).expect("save");

    // What a crash mid-write leaves: a `.tmp` sibling holding a half-written
    // document. It must never be mistaken for the real file, and the next save
    // must overwrite it rather than trip over it.
    std::fs::write(&tmp, "{\"version\":1,\"root\":\"/half").expect("orphan tmp");
    let back = marks_file::load(repo);
    checks.eq(
        "an orphaned .tmp is not what gets loaded",
        back.parts().0.len(),
        3,
    );
    checks.ok(
        "…and the real file is untouched",
        back.get(&ids[0]).is_some(),
    );

    let mut store = Store::default();
    store.add_anchored(
        repo.join("docs/a.md").to_string_lossy().into_owned(),
        A,
        "delta".to_string(),
        String::new(),
        String::new(),
        Origin::Human,
    );
    marks_file::save(repo, &store).expect("save over an orphaned tmp");
    checks.ok("a later save clears the orphan", !tmp.exists());
    checks.eq(
        "…and the marks directory holds one file again",
        entries(&marks_file::dir()),
        vec![marks_file::file_name_for(repo)],
    );
    checks.eq(
        "…carrying the newer store",
        marks_file::load(repo).parts().0.len(),
        1,
    );
}

fn two_repos(checks: &mut Checks, repo: &Path, other: &Path) {
    let (store, ids) = seeded(repo);
    marks_file::save(repo, &store).expect("save repo");

    let mut theirs = Store::default();
    theirs.add_highlight(
        other.join("notes.md").to_string_lossy().into_owned(),
        1,
        1,
        "theirs".to_string(),
        String::new(),
        String::new(),
    );
    marks_file::save(other, &theirs).expect("save other");

    checks.eq(
        "one repo's save does not disturb another's",
        marks_file::load(repo).parts().0.len(),
        3,
    );
    checks.ok(
        "…down to the ids",
        marks_file::load(repo).get(&ids[0]).is_some(),
    );
    checks.eq(
        "…and the second repo keeps its own",
        marks_file::load(other).parts().0.len(),
        1,
    );
    checks.eq(
        "two files, one per repo",
        entries(&marks_file::dir()).len(),
        2,
    );
    // Marks naming a file in the *other* repo are dropped, which is the same
    // containment rule doing the work a hash collision would otherwise defeat.
    let mut crossed = Store::default();
    crossed.add_highlight(
        other.join("notes.md").to_string_lossy().into_owned(),
        1,
        1,
        "theirs".to_string(),
        String::new(),
        String::new(),
    );
    marks_file::save(repo, &crossed).expect("save crossed");
    checks.ok(
        "a mark pointing into another repo is dropped on load",
        marks_file::load(repo).parts().0.is_empty(),
    );
}

fn the_lock(checks: &mut Checks, repo: &Path, other: &Path) {
    // `cli::repo_is_claimed` is what `main.rs` asks before it decides whether
    // this process may write the marks file at all, and what `marks prune`
    // asks before it rewrites one under a running window. It lives in the
    // library precisely so it can be driven here — in `main.rs` it would be
    // unreachable.
    let socket = server::socket_path(repo);
    checks.ok(
        "an unopened repo is unclaimed",
        !dreamd::cli::repo_is_claimed(repo),
    );

    let bound = server::bind(&socket).expect("bind");
    let listener = match bound {
        Bound::Primary(listener) => listener,
        Bound::Secondary => {
            checks.ok("the first bind is primary", false);
            return;
        }
    };
    checks.ok("the first bind is primary", true);
    checks.ok(
        "a live owner makes the repo claimed",
        dreamd::cli::repo_is_claimed(repo),
    );
    checks.ok(
        "…and only for that repo",
        !dreamd::cli::repo_is_claimed(other),
    );
    checks.ok(
        "a second bind stands down — exactly one writer",
        matches!(server::bind(&socket), Ok(Bound::Secondary)),
    );

    // A crash leaves the socket *file* behind with nothing listening. That must
    // read as unclaimed, or the marks file would be orphaned for good the first
    // time dreamd was killed.
    drop(listener);
    checks.ok("the socket file outlives the listener", socket.exists());
    checks.ok(
        "a crash leftover is not a live owner",
        !dreamd::cli::repo_is_claimed(repo),
    );
    checks.ok(
        "…and rebinding over it succeeds",
        matches!(server::bind(&socket), Ok(Bound::Primary(_))),
    );
    let _ = std::fs::remove_file(&socket);
}

/// `dreamd marks prune`, driven through `cli::run` — the real entry point, not
/// the predicates underneath it.
///
/// The predicates have their own unit tests and real teeth; what had none was
/// the `retain` that calls them, which is the line that actually deletes a
/// user's work. Swapping `is_stale_droppable(h)` there for a bare
/// `h.state == Stale` — so `--stale` starts eating annotated questions — left
/// every unit test and every other check in this file green.
fn prune(checks: &mut Checks, repo: &Path) {
    use dreamd::cli::{self, Cmd, MarksCmd};

    // `cli::run` resolves the repo from the working directory, the way the
    // shell invocation does.
    std::env::set_current_dir(repo).expect("cd into the repo");
    let file = repo.join("docs/a.md").to_string_lossy().into_owned();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());

    // stale+unannotated, stale+annotated, active+unannotated, answered long
    // ago, answered just now.
    let seed = |checks: &mut Checks| {
        let doc = format!(
            "{{\"version\":1,\"root\":{:?},\"highlights\":[
               {{\"id\":\"stale_bare\",\"file_path\":{file:?},\"quote\":\"alpha\",\"state\":\"stale\"}},
               {{\"id\":\"stale_asked\",\"file_path\":{file:?},\"quote\":\"beta\",\"state\":\"stale\",\"annotation\":\"why?\"}},
               {{\"id\":\"active_bare\",\"file_path\":{file:?},\"quote\":\"gamma\"}},
               {{\"id\":\"answered_old\",\"file_path\":{file:?},\"quote\":\"delta\",\"annotation\":\"why?\",
                 \"resolved\":{{\"note\":\"yes\",\"at\":{}}}}},
               {{\"id\":\"answered_now\",\"file_path\":{file:?},\"quote\":\"alpha\",\"annotation\":\"why?\",
                 \"resolved\":{{\"note\":\"yes\",\"at\":{now}}}}}
             ],\"stack\":[\"stale_asked\",\"answered_old\"]}}",
            repo.to_string_lossy(),
            now - 40 * 24 * 60 * 60,
        );
        write_raw(repo, &doc);
        checks.eq(
            "the prune fixture loads whole",
            marks_file::load(repo).parts().0.len(),
            5,
        );
    };
    let ids = |repo: &Path| {
        marks_file::load(repo)
            .parts()
            .0
            .iter()
            .map(|h| h.id.clone())
            .collect::<Vec<_>>()
    };

    // A bare `prune` is a dry run — a destructive verb whose default changes
    // nothing.
    seed(checks);
    checks.ok(
        "a bare prune succeeds",
        cli::run(Cmd::Marks {
            action: MarksCmd::Prune {
                stale: false,
                older_than: None,
            },
        })
        .is_ok(),
    );
    checks.eq("…and removes nothing", ids(repo).len(), 5);

    // `--stale` takes the stale mark nobody asked a question about, and
    // **nothing else**. `stale_asked` is the one that matters: a question in
    // flight, on a quote a `git checkout` stranded, which is exactly the mark a
    // careless filter deletes.
    cli::run(Cmd::Marks {
        action: MarksCmd::Prune {
            stale: true,
            older_than: None,
        },
    })
    .expect("prune --stale");
    checks.eq(
        "--stale takes the unasked stale mark only",
        ids(repo),
        vec![
            "stale_asked".to_string(),
            "active_bare".to_string(),
            "answered_old".to_string(),
            "answered_now".to_string(),
        ],
    );
    checks.eq(
        "…and leaves the stack alone",
        marks_file::load(repo).parts().1.to_vec(),
        vec!["stale_asked".to_string(), "answered_old".to_string()],
    );

    // `--older-than` is the one that *does* take annotated marks — the question
    // was answered — and only those, by age.
    seed(checks);
    cli::run(Cmd::Marks {
        action: MarksCmd::Prune {
            stale: false,
            older_than: Some(30 * 24 * 60 * 60),
        },
    })
    .expect("prune --older-than");
    checks.eq(
        "--older-than takes the mark answered 40 days ago",
        ids(repo),
        vec![
            "stale_bare".to_string(),
            "stale_asked".to_string(),
            "active_bare".to_string(),
            "answered_now".to_string(),
        ],
    );
    checks.eq(
        "…and its stack entry goes with it",
        marks_file::load(repo).parts().1.to_vec(),
        vec!["stale_asked".to_string()],
    );

    // Both flags at once, and the invariant that outlives either: an annotated
    // mark nobody answered is never deleted by any combination.
    seed(checks);
    cli::run(Cmd::Marks {
        action: MarksCmd::Prune {
            stale: true,
            older_than: Some(30 * 24 * 60 * 60),
        },
    })
    .expect("prune --stale --older-than");
    let left = ids(repo);
    checks.ok(
        "an unanswered question survives every flag",
        left.contains(&"stale_asked".to_string()),
    );
    checks.eq(
        // Both criteria in one pass — and the mark answered *this second* is
        // still here, because `--older-than 30d` means answered longer ago
        // than that, not "answered at all".
        "…and both flags apply in one pass",
        left,
        vec![
            "stale_asked".to_string(),
            "active_bare".to_string(),
            "answered_now".to_string(),
        ],
    );

    // A prune of a repo with nothing saved is an error, not an empty file.
    let path = marks_file::path_for(repo);
    std::fs::remove_file(&path).expect("remove marks file");
    checks.ok(
        "pruning a repo with no marks file fails loudly",
        cli::run(Cmd::Marks {
            action: MarksCmd::Prune {
                stale: true,
                older_than: None,
            },
        })
        .is_err(),
    );
    checks.ok("…and writes nothing", !path.exists());
    checks.ok(
        "marks path succeeds with nothing saved",
        cli::run(Cmd::Marks {
            action: MarksCmd::Path,
        })
        .is_ok(),
    );
    // Out of the sandbox before `main` deletes it from under us.
    let _ = std::env::set_current_dir("/");
}

// ---- harness -------------------------------------------------------------

#[derive(Default)]
struct Checks {
    passed: usize,
    failed: usize,
}

impl Checks {
    fn ok(&mut self, what: &str, cond: bool) {
        if cond {
            self.passed += 1;
        } else {
            self.failed += 1;
            println!("FAIL  {what}");
        }
    }

    /// Two type parameters so a `String` can be compared against a literal.
    fn eq<A, B>(&mut self, what: &str, got: A, want: B)
    where
        A: std::fmt::Debug + PartialEq<B>,
        B: std::fmt::Debug,
    {
        if got == want {
            self.passed += 1;
        } else {
            self.failed += 1;
            println!("FAIL  {what}\n        got  {got:?}\n        want {want:?}");
        }
    }

    fn finish(self) {
        println!(
            "marks_check: {} passed, {} failed",
            self.passed, self.failed
        );
        if self.failed > 0 {
            std::process::exit(1);
        }
    }
}
