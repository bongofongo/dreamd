//! Correctness harness for the MCP transport: a real socket, a real client,
//! and the shim's own forwarder driven against it. Exits non-zero on the first
//! failure.
//!
//! It is an example rather than a `#[cfg(test)]` module for the reason
//! `CLAUDE.md` gives: the socket lives under `config_dir()`, which reads the
//! real `~/.config/dreamd`, and `cargo test` is not allowed anywhere near it.
//! Here `XDG_CONFIG_HOME` is repointed process-wide before anything runs, the
//! same trick `config_check.rs` uses.
//!
//! What is *not* here: anything `mcp/server.rs`'s unit tests already cover
//! without a socket — method routing, the emit filter, the line cap. This
//! harness exists for what only a real socket can show: the file mode, the
//! bind-is-the-lock rule, the shim's wire format, and a cancel that actually
//! retires the thread.
//!
//! ```sh
//! cargo run --example mcp_check
//! ```

use dreamd::annotations::{Origin, Store};
use dreamd::mcp::{jsonrpc, schema, server, shim};
use dreamd::notify;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const A: &str = "alpha\nbeta\ngamma\n";
const B: &str = "one\ntwo\nthree\n";

fn main() {
    let root = std::env::temp_dir().join("dreamd-mcp-check");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("temp dir");
    // Before anything reaches `config_dir()`.
    std::env::set_var("XDG_CONFIG_HOME", root.join("xdg"));

    let repo = root.join("repo");
    std::fs::create_dir_all(repo.join("docs")).expect("repo dir");
    std::fs::write(repo.join("docs/a.md"), A).expect("a.md");
    std::fs::write(repo.join("docs/b.md"), B).expect("b.md");
    let repo = repo.canonicalize().expect("canonical repo");

    let mut checks = Checks::default();

    // Three marks across two files, annotated in a deliberate order: this is
    // the acceptance scenario's fixture, in miniature.
    let store = Arc::new(Mutex::new(Store::default()));
    let ids = {
        let mut s = store.lock().unwrap();
        let mut mint = |file: &Path, source: &str, quote: &str| {
            let id = s.add_anchored(
                file.to_string_lossy().into_owned(),
                source,
                quote.to_string(),
                String::new(),
                String::new(),
                Origin::Human,
            );
            s.set_annotation(&id, format!("what about {quote}?"));
            id
        };
        vec![
            mint(&repo.join("docs/a.md"), A, "beta"),
            mint(&repo.join("docs/b.md"), B, "two"),
            mint(&repo.join("docs/a.md"), A, "gamma"),
        ]
    };

    let (notifier, events) = notify::recording();
    let cancel = Arc::new(AtomicBool::new(false));
    server::spawn(store.clone(), repo.clone(), notifier, cancel.clone());

    let path = server::socket_path(&repo);
    checks.ok("the socket appears", wait_for(&path, true));
    if !path.exists() {
        checks.finish();
        return;
    }

    // --- the file itself --------------------------------------------------
    let mode = |p: &Path| {
        std::fs::metadata(p)
            .map(|m| m.permissions().mode() & 0o777)
            .unwrap_or(0)
    };
    checks.eq("socket mode is 0600", mode(&path), 0o600);
    checks.eq("run directory mode is 0700", mode(&server::dir()), 0o700);
    checks.ok(
        "the socket path fits in sun_path",
        path.as_os_str().len() < 104,
    );

    // --- the handshake ----------------------------------------------------
    let mut conn = Conn::open(&path);
    let init = conn.call("initialize", json!({}));
    checks.eq(
        "initialize answers from the compiled-in schema",
        init,
        schema::initialize_result(),
    );
    checks.eq(
        "tools/list matches the const both halves read",
        conn.call("tools/list", json!({})),
        schema::tools_list_result(),
    );
    checks.eq("ping answers", conn.call("ping", json!({})), json!({}));

    // A notification gets silence, and the connection survives it. Sent on the
    // same connection as the next request, so a spurious reply would be read
    // in its place and fail the assertion below.
    conn.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    // A malformed line must not end the session either.
    conn.send_raw("{not json\n");
    let malformed = conn.read_reply();
    checks.eq(
        "a malformed line is a parse error",
        malformed["error"]["code"].as_i64().unwrap_or(0),
        jsonrpc::PARSE_ERROR,
    );
    checks.eq(
        "and the connection still serves",
        conn.call("ping", json!({}))
            .as_object()
            .map(|o| o.len())
            .unwrap_or(9),
        0,
    );

    let unknown = conn.request("resources/list", json!({}));
    checks.eq(
        "an unknown method is method-not-found",
        unknown["error"]["code"].as_i64().unwrap_or(0),
        jsonrpc::METHOD_NOT_FOUND,
    );

    // --- the tools, over the shim's own wire ------------------------------
    // From here on everything goes through `shim::forward`, which is what an
    // agent's calls actually travel through: a fresh connection per call, the
    // shim's framing, the shim's reply unwrapping.
    let call = |name: &str, args: Value| -> Value {
        shim::forward(&repo, &json!({"name": name, "arguments": args}))
    };
    let text_of = |v: &Value| -> String {
        v["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    let errored = |v: &Value| -> bool { v["isError"].as_bool().unwrap_or(true) };
    let parsed = |v: &Value| -> Value { serde_json::from_str(&text_of(v)).unwrap_or(Value::Null) };

    let stack = call("get_stack", json!({}));
    checks.ok("get_stack succeeds over the shim", !errored(&stack));
    let questions = parsed(&stack);
    checks.eq(
        "get_stack returns every annotated mark",
        questions.as_array().map(Vec::len).unwrap_or(0),
        3,
    );
    checks.eq(
        "in the order the human asked",
        questions[0]["mark"]["id"].as_str().unwrap_or_default(),
        ids[0].as_str(),
    );
    checks.eq(
        "and the third is the third",
        questions[2]["mark"]["id"].as_str().unwrap_or_default(),
        ids[2].as_str(),
    );
    checks.ok(
        "the anchoring context is delimited too",
        text_of(&call("get_highlight", json!({"id": ids[1]})))
            .contains("<untrusted-context-before sentinel="),
    );
    checks.ok(
        "paths leaving dreamd are repo-relative",
        !text_of(&stack).contains(&repo.to_string_lossy().to_string()),
    );
    checks.ok(
        "the quote crosses the wire delimited",
        text_of(&stack).contains("<untrusted-quote sentinel="),
    );

    let detail = call("get_highlight", json!({"id": ids[1]}));
    checks.ok("get_highlight succeeds", !errored(&detail));
    checks.eq(
        "and names the right file",
        parsed(&detail)["file"].as_str().unwrap_or_default(),
        "docs/b.md",
    );
    checks.ok(
        "an unknown id is a tool error, not a protocol one",
        errored(&call("get_highlight", json!({"id": "h0000000000000000"}))),
    );

    let listed = call("list_highlights", json!({"file": "docs/a.md"}));
    checks.eq(
        "list_highlights filters by file",
        parsed(&listed).as_array().map(Vec::len).unwrap_or(0),
        2,
    );

    // --- the write tools ---------------------------------------------------
    let marked = call(
        "mark_passage",
        json!({"file": "docs/b.md", "quote": "three", "note": "and this?"}),
    );
    checks.ok("mark_passage succeeds", !errored(&marked));
    let agent_id = parsed(&marked)["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    checks.ok("mark_passage returns an id", agent_id.starts_with('h'));
    checks.eq(
        "an agent's mark is not a question and stays off the stack",
        parsed(&call("get_stack", json!({})))
            .as_array()
            .map(Vec::len)
            .unwrap_or(0),
        3,
    );
    checks.eq(
        "and it is recorded as the agent's",
        store
            .lock()
            .unwrap()
            .get(&agent_id)
            .map(|h| h.origin)
            .unwrap_or(Origin::Human),
        Origin::Agent,
    );

    // Security-shaped, over the real transport: `mark_passage` must refuse a
    // path outside the root even when the socket itself is reachable.
    //
    // **Every target here has to exist, and the quote has to be in it.**
    // `mark_passage` refuses for three different reasons — the path will not
    // canonicalise, the path is outside the root, the quote is not in the file
    // — and only the middle one is containment. A target that does not exist,
    // or a quote that is not in it, produces a refusal that survives deleting
    // `guard::inside_root` outright: green, and meaningless. An earlier version
    // of this loop was exactly that, twice over.
    const SECRET: &str = "the secret line\n";
    let outside = root.join("outside");
    std::fs::create_dir_all(&outside).expect("outside dir");
    std::fs::write(outside.join("secret.md"), SECRET).expect("outside file");
    // A sibling whose path shares the root's prefix textually — `/w/notes` vs
    // `/w/notes-private` — which is why `inside_root` compares components.
    let sibling = root.join("repo-private");
    std::fs::create_dir_all(&sibling).expect("sibling dir");
    std::fs::write(sibling.join("x.md"), SECRET).expect("sibling file");

    for (what, file) in [
        (
            "an absolute path outside the root",
            outside.join("secret.md").to_string_lossy().into_owned(),
        ),
        ("a dot-dot escape", "../outside/secret.md".to_string()),
        (
            "a sibling sharing the root's prefix",
            "../repo-private/x.md".to_string(),
        ),
    ] {
        let out = call(
            "mark_passage",
            json!({"file": file, "quote": SECRET.trim(), "note": "n"}),
        );
        checks.ok(&format!("mark_passage refuses {what}"), errored(&out));
        // Not just "it refused" — refused *for containment*. Without this the
        // check passes on "no such file" and on "that quote is not in there",
        // neither of which says anything about the guard.
        checks.ok(
            &format!("and refuses {what} on containment, not on some other ground"),
            text_of(&out).contains("outside the repo root"),
        );
        // The refusal echoes back what the caller asked for, which is fine —
        // what must never appear is dreamd's own absolute path, the same leak
        // `paths_leaving_dreamd_are_repo_relative` guards on the happy path.
        checks.ok(
            &format!("and the refusal for {what} leaks no local path"),
            !text_of(&out).contains(repo.to_string_lossy().as_ref()),
        );
    }

    let before = events.lock().unwrap().len();
    let resolved = call(
        "resolve_highlight",
        json!({"id": ids[0], "note": "answered"}),
    );
    checks.ok("resolve_highlight succeeds", !errored(&resolved));
    checks.eq(
        "and reports what is left",
        parsed(&resolved)["remainingOnStack"].as_u64().unwrap_or(99),
        2,
    );
    checks.ok(
        "the mark survives resolution",
        store.lock().unwrap().get(&ids[0]).is_some(),
    );
    {
        let seen = events.lock().unwrap();
        checks.eq("one marks-changed event was pushed", seen.len(), before + 1);
        checks.eq(
            "naming the mark's file",
            seen.last().and_then(|e| e.file_path.clone()),
            Some(repo.join("docs/a.md").to_string_lossy().into_owned()),
        );
        checks.ok(
            "and flagging the stack",
            seen.last().is_some_and(|e| e.stack),
        );
    }

    checks.ok(
        "an unknown tool is a tool error",
        errored(&call("nope", json!({}))),
    );

    // --- the bind is the lock ---------------------------------------------
    match server::bind(&path) {
        Ok(server::Bound::Secondary) => checks.ok("a second bind stands down", true),
        _ => checks.ok("a second bind stands down", false),
    }

    // A stale socket file — the crash case — is unlinked and rebound rather
    // than mistaken for a live owner.
    let orphan = server::dir().join("0000000000000000.sock");
    drop(std::os::unix::net::UnixListener::bind(&orphan).expect("orphan socket"));
    checks.ok("a stale socket file is left behind", orphan.exists());
    match server::bind(&orphan) {
        Ok(server::Bound::Primary(_)) => checks.ok("and is rebound, not adopted", true),
        _ => checks.ok("and is rebound, not adopted", false),
    }
    let _ = std::fs::remove_file(&orphan);

    // --- retirement --------------------------------------------------------
    cancel.store(true, Ordering::Relaxed);
    checks.ok("cancelling unlinks the socket", wait_for(&path, false));
    checks.ok(
        "and a call afterwards reports dreamd as closed",
        text_of(&call("get_stack", json!({}))).contains("dreamd is not open"),
    );

    checks.finish();
}

/// Poll for a path appearing or disappearing. The server binds and unlinks on
/// its own thread, and the accept loop only notices cancellation on its next
/// poll, so both are inherently "soon" rather than "now".
fn wait_for(path: &Path, want: bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if path.exists() == want {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

/// A raw client, for the protocol-level checks the shim's forwarder hides.
struct Conn {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
    next_id: u64,
}

impl Conn {
    fn open(path: &Path) -> Self {
        let stream = UnixStream::connect(path).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        Self {
            reader: BufReader::new(stream.try_clone().expect("clone")),
            writer: stream,
            next_id: 1,
        }
    }

    fn send(&mut self, value: &Value) {
        self.send_raw(&format!("{value}\n"));
    }

    fn send_raw(&mut self, line: &str) {
        self.writer.write_all(line.as_bytes()).expect("write");
        self.writer.flush().expect("flush");
    }

    fn read_reply(&mut self) -> Value {
        let mut line = String::new();
        self.reader.read_line(&mut line).expect("read");
        serde_json::from_str(line.trim()).unwrap_or(Value::Null)
    }

    /// Send a request, return the whole reply envelope.
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        self.read_reply()
    }

    /// Send a request, return its `result`.
    fn call(&mut self, method: &str, params: Value) -> Value {
        self.request(method, params)["result"].clone()
    }
}

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
        println!("mcp_check: {} passed, {} failed", self.passed, self.failed);
        if self.failed > 0 {
            std::process::exit(1);
        }
    }
}
