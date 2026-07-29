//! The transport: a Unix domain socket in the GUI process, and the method
//! routing on top of it.
//!
//! ## Why a socket and not a port
//!
//! The agent runs `dreamd mcp` (see [`super::shim`]), which speaks stdio to
//! Claude Code and proxies `tools/call` here. Authorisation is therefore the
//! filesystem's: mode `0600` under `~/.config/dreamd/run/`, no port, no token,
//! no firewall prompt. Routing solves itself — the shim resolves the same repo
//! root the GUI did, from a cwd inside the repo, so it derives the same socket
//! path with no registry and no discovery.
//!
//! ## The bind is the lock
//!
//! Binding is how a dreamd claims a repo. `AddrInUse` is disambiguated by
//! connecting: a socket that answers belongs to a live dreamd and this process
//! stands down as a **secondary** (no MCP, and in step 4 no persistence
//! either); a socket that refuses is a leftover from a crash and is unlinked
//! and rebound. If the transport ever stops being a UDS, that lock has to
//! become an explicit lockfile — the coupling is deliberate but it is real.
//!
//! ## Tauri-free
//!
//! Nothing here mentions `AppHandle`. Mutations reach the window through a
//! [`Notifier`] closure, which is what lets `examples/mcp_check.rs` drive the
//! whole transport with no window and no event loop.

use crate::annotations::Store;
use crate::mcp::{jsonrpc, schema, tools};
use crate::notify::{MarksChanged, Notifier};
use crate::{config, marks_file};
use serde_json::{json, Value};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long the accept loop blocks before rechecking `cancel`.
///
/// The listener is non-blocking and polled rather than blocking on `accept`,
/// because retiring the thread has to work when nothing is connected —
/// `adopt_root` moves the repo root and the old socket must go with it. The
/// cost is one syscall every 200ms on an idle background thread.
const ACCEPT_POLL: Duration = Duration::from_millis(200);

/// What the window can say about its own MCP socket, without asking the socket.
///
/// The whole of the agent half of dreamd is invisible from the reading side: a
/// reader whose `claude mcp add dreamd` never ran, or who is looking at a
/// second window on a repo the first one claimed, sees a pane, sends a stack,
/// and gets an agent that cannot resolve a single mark. Nothing on screen says
/// why. These two counters are the smallest thing that can say it.
///
/// [`serving`](Status::serving) is this process's answer to "did I get the
/// lock" — false for a secondary, and false for a bind that failed outright.
/// [`clients`](Status::clients) counts accepted connections since that bind, so
/// zero means **no agent has ever spoken to this window**. It is not liveness:
/// the shim connects per call and disconnects, so a working session's count
/// only ever goes up. That is deliberate — "has this ever worked" is the
/// question a status line can answer honestly, and "is it connected right now"
/// is one a per-call transport cannot.
///
/// Each [`spawn`] gets its own, so a retiring thread on the previous root
/// cannot write `serving = false` over its replacement's `true`.
#[derive(Default)]
pub struct Status {
    serving: AtomicBool,
    clients: AtomicUsize,
}

impl Status {
    /// Whether this process holds this repo's socket.
    pub fn serving(&self) -> bool {
        self.serving.load(Ordering::Relaxed)
    }

    /// Connections accepted since the bind. Zero means no agent has reached
    /// this window.
    pub fn clients(&self) -> usize {
        self.clients.load(Ordering::Relaxed)
    }
}

/// `~/.config/dreamd/run`.
pub fn dir() -> PathBuf {
    config::config_dir().join("run")
}

/// The socket path for a repo root.
///
/// Deliberately *without* the readable basename prefix the marks file carries:
/// macOS caps `sun_path` at ~104 bytes, and a long home directory plus a long
/// repo name would silently truncate. The hash is FNV-1a over the canonical
/// root — the same one `marks_file` uses, so a repo's socket and its marks file
/// carry the same identity.
pub fn socket_path(root: &Path) -> PathBuf {
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    dir().join(format!("{}.sock", marks_file::root_hash(&canonical)))
}

/// What binding produced.
pub enum Bound {
    /// This process owns the repo.
    Primary(UnixListener),
    /// Another live dreamd owns it. No MCP here.
    Secondary,
}

/// Claim the socket for `root`, or discover that someone else already has.
///
/// Public because `mcp_check` asserts on the outcome, and because it is the
/// whole multi-instance rule in one function.
pub fn bind(path: &Path) -> io::Result<Bound> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        // Best-effort: an existing directory with looser modes is the user's
        // business, and failing to start MCP over it would be worse.
        let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
    }
    let listener = match UnixListener::bind(path) {
        Ok(listener) => listener,
        Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
            // A socket file exists. Only a connect can tell a live owner from
            // a crash leftover: the inode survives the process that made it.
            if UnixStream::connect(path).is_ok() {
                return Ok(Bound::Secondary);
            }
            std::fs::remove_file(path)?;
            UnixListener::bind(path)?
        }
        Err(e) => return Err(e),
    };
    // The socket is the authorisation boundary; a world-writable one would let
    // any local process drive the agent's view of the human's queue.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(Bound::Primary(listener))
}

/// Start the server in a background thread.
///
/// Shaped like `watcher::spawn`, including the cancel token: `adopt_root`
/// retires this the same way it retires the watcher, because a server left
/// listening on the previous root would hand an agent another repo's marks.
///
/// The returned [`Status`] is this thread's, and it is the only way the window
/// learns whether the bind it just asked for succeeded — `spawn` cannot answer
/// that itself without blocking on the thread it just started.
pub fn spawn(
    store: Arc<Mutex<Store>>,
    root: PathBuf,
    notify: Notifier,
    cancel: Arc<AtomicBool>,
) -> Arc<Status> {
    let status = Arc::new(Status::default());
    let mine = status.clone();
    std::thread::spawn(move || serve(&store, &root, &notify, &cancel, &mine));
    status
}

/// Bind, accept until cancelled, then unlink. Never panics: this is a
/// background thread and losing MCP must not take the window with it.
pub fn serve(
    store: &Arc<Mutex<Store>>,
    root: &Path,
    notify: &Notifier,
    cancel: &Arc<AtomicBool>,
    status: &Status,
) {
    let path = socket_path(root);
    let listener = match bind(&path) {
        Ok(Bound::Primary(listener)) => listener,
        Ok(Bound::Secondary) => {
            eprintln!(
                "dreamd: another dreamd is already serving {} — this window will not answer MCP",
                root.display()
            );
            return;
        }
        Err(e) => {
            eprintln!("dreamd: could not open the MCP socket at {path:?}: {e}");
            return;
        }
    };
    if listener.set_nonblocking(true).is_err() {
        eprintln!("dreamd: MCP socket cannot be polled; not serving");
        let _ = std::fs::remove_file(&path);
        return;
    }
    // Only here, past every way the bind could have failed. The status line
    // this feeds says "not serving" for a secondary, a permissions failure and
    // an unpollable socket alike, which is the right grain: all three mean the
    // agent in this window's pane is talking to nothing.
    status.serving.store(true, Ordering::Relaxed);

    while !cancel.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                status.clients.fetch_add(1, Ordering::Relaxed);
                let (store, root, notify, cancel) = (
                    store.clone(),
                    root.to_path_buf(),
                    notify.clone(),
                    cancel.clone(),
                );
                std::thread::spawn(move || {
                    // An accepted socket can inherit the listener's
                    // non-blocking flag on macOS, which would turn every read
                    // into a spin.
                    let _ = stream.set_nonblocking(false);
                    serve_connection(stream, &store, &root, &notify, &cancel);
                });
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => std::thread::sleep(ACCEPT_POLL),
            Err(e) => {
                eprintln!("dreamd: MCP socket stopped accepting: {e}");
                break;
            }
        }
    }

    // Safe to clear unconditionally: this `Status` belongs to this thread
    // alone, so a retiring server cannot contradict the one that replaced it.
    status.serving.store(false, Ordering::Relaxed);
    // Unlink on the way out so the next bind — a rebind after `adopt_root`, or
    // the next run — doesn't have to go through the stale-socket probe.
    drop(listener);
    let _ = std::fs::remove_file(&path);
}

/// One connection: NDJSON in, NDJSON out, until EOF or cancellation.
fn serve_connection(
    stream: UnixStream,
    store: &Mutex<Store>,
    root: &Path,
    notify: &Notifier,
    cancel: &AtomicBool,
) {
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(read_half);
    let mut writer = stream;
    let mut line = String::new();

    while !cancel.load(Ordering::Relaxed) {
        line.clear();
        match read_capped(&mut reader, &mut line) {
            Ok(0) => return,
            Ok(_) => {}
            Err(Capped::TooLong) => {
                // An unbounded `read_line` on a socket is a memory DoS, so the
                // cap is enforced before the bytes are kept. The connection
                // goes with it: the rest of that line is unbounded too, and a
                // client that overruns a 1 MiB frame has lost sync anyway. The
                // shim opens a connection per call, so this costs one call.
                let e = jsonrpc::Error::parse_error(format!(
                    "line exceeds the {}-byte cap",
                    jsonrpc::MAX_LINE
                ));
                let _ =
                    writer.write_all(jsonrpc::Response::error(Value::Null, e).line().as_bytes());
                return;
            }
            Err(Capped::Io(e)) => {
                if e.kind() != io::ErrorKind::UnexpectedEof {
                    eprintln!("dreamd: MCP connection ended: {e}");
                }
                return;
            }
        }

        let reply = jsonrpc::handle_line(&line, |request| respond(store, root, notify, request));
        if let Some(reply) = reply {
            if writer.write_all(reply.as_bytes()).is_err() || writer.flush().is_err() {
                return;
            }
        }
    }
}

enum Capped {
    TooLong,
    Io(io::Error),
}

/// `read_line`, refusing to allocate past [`jsonrpc::MAX_LINE`].
///
/// Reading through a `take` is the point: a plain `read_line` would buffer the
/// whole oversized line first and only then let anyone object.
fn read_capped<R: BufRead>(reader: &mut R, out: &mut String) -> Result<usize, Capped> {
    let cap = jsonrpc::MAX_LINE as u64;
    let read = reader.take(cap + 1).read_line(out).map_err(Capped::Io)?;
    if read as u64 > cap {
        return Err(Capped::TooLong);
    }
    Ok(read)
}

/// Route one JSON-RPC method.
///
/// `initialize` and `tools/list` are answered here as well as in the shim.
/// That is not duplication — both read `schema`'s one const — and it is what
/// lets a client talk to the socket directly (`mcp_check` does).
pub fn respond(
    store: &Mutex<Store>,
    root: &Path,
    notify: &Notifier,
    request: &jsonrpc::Request,
) -> Result<Value, jsonrpc::Error> {
    match request.method.as_str() {
        "initialize" => Ok(schema::initialize_result()),
        // A notification. Dispatch still runs; `handle_line` suppresses the
        // reply, so what this returns is never written.
        m if m.starts_with("notifications/") => Ok(Value::Null),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(schema::tools_list_result()),
        "tools/call" => tools_call(store, root, notify, request.params()),
        other => Err(jsonrpc::Error::method_not_found(other)),
    }
}

fn tools_call(
    store: &Mutex<Store>,
    root: &Path,
    notify: &Notifier,
    params: &Value,
) -> Result<Value, jsonrpc::Error> {
    let call: tools::ToolCall = serde_json::from_value(params.clone())
        .map_err(|e| jsonrpc::Error::invalid_params(format!("not a tools/call: {e}")))?;

    let (result, change) = {
        let mut store = store.lock().unwrap();
        let result = tools::call(&mut store, root, &call);
        let change = change_after(&store, &call, &result);
        (result, change)
    };
    // Outside the lock: the notifier reaches the webview, whose commands take
    // the same lock on their way back in.
    if let Some(change) = change {
        notify(change);
    }
    Ok(result)
}

/// What the window has to repaint after a tool ran, if anything.
///
/// Only the two write tools produce an event, and a failed call produces none.
/// `tools::call` reports a refusal as `isError: true` inside a *successful*
/// JSON-RPC result, so that flag — not an `Err` — is what "the call failed"
/// looks like from here.
///
/// The two arms lean on it differently, and it is worth knowing which. For
/// `mark_passage` there is a second, independent gate: a refused call has no
/// `structuredContent.id` to read, so it would fall out anyway. For
/// `resolve_highlight` the `isError` check is the *only* thing standing between
/// a refusal and a phantom repaint — the id comes from the caller's arguments,
/// which are just as present on a call that failed. An asymmetry like that is
/// how a later edit removes a check that looked redundant on the arm the author
/// happened to be reading, so both arms are pinned by their own test.
fn change_after(store: &Store, call: &tools::ToolCall, result: &Value) -> Option<MarksChanged> {
    if result["isError"].as_bool().unwrap_or(false) {
        return None;
    }
    let file_of = |id: &str| store.get(id).map(|h| h.file_path);
    match call.name.as_str() {
        // The mark survives resolution, so its file is still knowable.
        "resolve_highlight" => {
            let id = call.arguments.get("id")?.as_str()?;
            Some(MarksChanged {
                file_path: file_of(id),
                stack: true,
            })
        }
        // An agent's mark is never annotated, so it is never on the stack.
        "mark_passage" => {
            let id = result["structuredContent"]["id"].as_str()?;
            Some(MarksChanged {
                file_path: file_of(id),
                stack: false,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    //! Nothing here binds a socket or touches `config_dir()` — that is
    //! `examples/mcp_check.rs`'s job, for the reason `CLAUDE.md` gives. What is
    //! testable without a directory is the method routing and the emit filter,
    //! and those are where the mistakes are.

    use super::*;
    use crate::annotations::Origin;
    use crate::notify;
    use serde_json::json;

    fn request(method: &str, params: Value) -> jsonrpc::Request {
        serde_json::from_value(json!({
            "jsonrpc": "2.0", "id": 1, "method": method, "params": params
        }))
        .expect("a request")
    }

    fn store_with_a_question() -> (Mutex<Store>, String) {
        let mut store = Store::default();
        let id = store.add_anchored(
            "/repo/a.md".into(),
            "alpha\nbeta\n",
            "beta".into(),
            String::new(),
            String::new(),
            Origin::Human,
        );
        store.set_annotation(&id, "why?".into());
        (Mutex::new(store), id)
    }

    #[test]
    fn initialize_and_tools_list_come_from_the_compiled_in_schema() {
        let (store, _) = store_with_a_question();
        let notify = notify::discard();
        let root = Path::new("/repo");

        let init = respond(&store, root, &notify, &request("initialize", Value::Null)).expect("ok");
        assert_eq!(init, schema::initialize_result());

        let list = respond(&store, root, &notify, &request("tools/list", Value::Null)).expect("ok");
        assert_eq!(list, schema::tools_list_result());
    }

    #[test]
    fn an_unknown_method_is_method_not_found_not_a_disconnect() {
        let (store, _) = store_with_a_question();
        let err = respond(
            &store,
            Path::new("/repo"),
            &notify::discard(),
            &request("tools/invoke", Value::Null),
        )
        .expect_err("an error");
        assert_eq!(err.code, jsonrpc::METHOD_NOT_FOUND);
    }

    #[test]
    fn a_tools_call_with_unusable_params_is_invalid_params() {
        let (store, _) = store_with_a_question();
        let err = respond(
            &store,
            Path::new("/repo"),
            &notify::discard(),
            &request("tools/call", json!("not an object")),
        )
        .expect_err("an error");
        assert_eq!(err.code, jsonrpc::INVALID_PARAMS);
    }

    #[test]
    fn resolving_over_the_wire_emits_one_event_naming_the_file_and_the_stack() {
        // The push path in one assertion: without it an agent closing a mark is
        // invisible until the human presses a key.
        let (store, id) = store_with_a_question();
        let (notify, log) = notify::recording();
        respond(
            &store,
            Path::new("/repo"),
            &notify,
            &request(
                "tools/call",
                json!({"name": "resolve_highlight",
                                          "arguments": {"id": id}}),
            ),
        )
        .expect("ok");

        let events = log.lock().unwrap();
        assert_eq!(
            *events,
            vec![MarksChanged::file("/repo/a.md", true)],
            "one event, naming the mark's file, flagging the stack"
        );
    }

    #[test]
    fn a_read_tool_emits_nothing() {
        // The filter that keeps the webview from repainting for every question
        // an agent reads.
        let (store, _) = store_with_a_question();
        let (notify, log) = notify::recording();
        for method in ["get_stack", "list_highlights"] {
            respond(
                &store,
                Path::new("/repo"),
                &notify,
                &request("tools/call", json!({"name": method, "arguments": {}})),
            )
            .expect("ok");
        }
        assert!(log.lock().unwrap().is_empty());
    }

    #[test]
    fn a_refused_write_emits_nothing() {
        // A containment refusal arrives as `isError: true` inside a
        // *successful* JSON-RPC result, so nothing upstream of here filters it.
        //
        // The `mark_passage` half of this is double-gated — no id in the
        // result either — which means it alone would stay green with the
        // `isError` check deleted. `resolving_a_refused_call_emits_nothing`
        // below is the half with teeth; keep both.
        let (store, _) = store_with_a_question();
        let (notify, log) = notify::recording();
        let out = respond(
            &store,
            Path::new("/repo"),
            &notify,
            &request(
                "tools/call",
                json!({"name": "mark_passage",
                       "arguments": {"file": "../../etc/passwd", "quote": "x", "note": "n"}}),
            ),
        )
        .expect("a result envelope, not a protocol error");
        assert_eq!(out["isError"], true);
        assert!(log.lock().unwrap().is_empty());
    }

    #[test]
    fn resolving_a_refused_call_emits_nothing() {
        // The arm where `isError` is load-bearing on its own: the id comes
        // from the caller's *arguments*, which are just as readable on a call
        // that failed as on one that worked. Drop the flag check and this
        // pushes a repaint for a mark that was never resolved.
        //
        // A real mark, unlike `resolving_an_unknown_id_emits_nothing` below —
        // so `store.get` would succeed and the event would even name the right
        // file. The only thing making it wrong is that the call was refused.
        let (store, id) = store_with_a_question();
        let (notify, log) = notify::recording();
        // Refused for a reason unrelated to the id: the arguments carry a key
        // the schema does not allow.
        let out = respond(
            &store,
            Path::new("/repo"),
            &notify,
            &request(
                "tools/call",
                json!({"name": "resolve_highlight",
                       "arguments": {"id": id, "note": 42}}),
            ),
        )
        .expect("a result envelope, not a protocol error");
        assert_eq!(out["isError"], true, "the call must actually have failed");
        assert!(
            log.lock().unwrap().is_empty(),
            "a refused resolve must not repaint the window"
        );
    }

    #[test]
    fn resolving_an_unknown_id_emits_nothing() {
        let (store, _) = store_with_a_question();
        let (notify, log) = notify::recording();
        respond(
            &store,
            Path::new("/repo"),
            &notify,
            &request(
                "tools/call",
                json!({"name": "resolve_highlight", "arguments": {"id": "h0000000000000000"}}),
            ),
        )
        .expect("ok");
        assert!(log.lock().unwrap().is_empty());
    }

    #[test]
    fn a_notification_runs_but_answers_nothing() {
        let (store, _) = store_with_a_question();
        let line = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let reply = jsonrpc::handle_line(line, |r| {
            respond(&store, Path::new("/repo"), &notify::discard(), r)
        });
        assert!(reply.is_none(), "a notification gets silence");
    }

    #[test]
    fn an_over_long_line_is_refused_without_being_buffered() {
        let mut oversized = Vec::with_capacity(jsonrpc::MAX_LINE + 64);
        oversized.extend(std::iter::repeat(b'x').take(jsonrpc::MAX_LINE + 8));
        oversized.push(b'\n');
        let mut reader = BufReader::new(&oversized[..]);
        let mut out = String::new();
        assert!(matches!(
            read_capped(&mut reader, &mut out),
            Err(Capped::TooLong)
        ));
        assert!(
            out.len() <= jsonrpc::MAX_LINE + 1,
            "the cap must bound what is kept, not just what is reported"
        );
    }

    #[test]
    fn a_line_at_the_cap_is_still_read() {
        // The boundary in the other direction: the cap refuses what is *over*
        // it, and an off-by-one here would reject legitimate traffic.
        let mut line = String::with_capacity(jsonrpc::MAX_LINE);
        line.push_str(&"x".repeat(jsonrpc::MAX_LINE - 1));
        line.push('\n');
        let mut reader = BufReader::new(line.as_bytes());
        let mut out = String::new();
        assert_eq!(
            read_capped(&mut reader, &mut out).ok(),
            Some(jsonrpc::MAX_LINE)
        );
    }

    #[test]
    fn the_socket_path_is_short_enough_for_sun_path() {
        // macOS caps `sun_path` at 104 bytes and truncates silently. The
        // basename prefix the marks file carries is dropped here for exactly
        // this reason, and this is what would notice it coming back.
        let path = socket_path(Path::new("/some/deeply/nested/repository/name"));
        assert!(
            path.as_os_str().len() < 104,
            "{} is {} bytes",
            path.display(),
            path.as_os_str().len()
        );
        assert!(path.to_string_lossy().ends_with(".sock"));
    }

    #[test]
    fn two_roots_get_two_sockets_and_one_root_is_stable() {
        let a = socket_path(Path::new("/repo/one"));
        let b = socket_path(Path::new("/repo/two"));
        assert_ne!(a, b);
        assert_eq!(a, socket_path(Path::new("/repo/one")));
    }
}
