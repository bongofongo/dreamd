//! Opt-in performance instrumentation.
//!
//! Entirely compiled out unless built with `--features perf`, so shipping
//! builds carry no timing code, no extra allocation, and no stderr chatter.
//!
//! When enabled, every mark is written to **stderr** as one NDJSON record:
//!
//! ```text
//! {"dreamd_perf":1,"phase":"walk_done","ms":12.418}
//! ```
//!
//! stderr rather than stdout because `console.log` inside WKWebView never
//! reaches the process's stdout — the frontend pushes its own marks back
//! through the `perf_mark` command so both halves land in one ordered stream
//! that `perf/scripts/*.sh` can parse with `jq`.
//!
//! [`span`] is the other shape: it times a closure and emits a `d:`-prefixed
//! duration. It exists because the frontend's `d:ipc_*` spans measure the wall
//! time of an `await`, and the first `await` after `innerHTML` also contains
//! the webview's deferred layout of the whole document — nearly a second on a
//! 2MB file. That cost landed on whichever IPC call happened to come first,
//! which made `d:ipc_get_highlights` read as 1248ms for a command criterion
//! measures at well under one. Timing the body here is what separates the two:
//! the gap between the frontend's span and the matching `d:rust_*` mark is the
//! webview, not dreamd.
//!
//! The clock origin is the first call to [`init`], which `main` makes as its
//! very first statement. That excludes dyld and pre-`main` runtime setup;
//! hyperfine measures whole-process wall time separately, and the difference
//! between the two is itself a useful number.

#[cfg(feature = "perf")]
mod imp {
    use std::io::Write;
    use std::sync::OnceLock;
    use std::time::Instant;

    static ORIGIN: OnceLock<Instant> = OnceLock::new();

    fn origin() -> Instant {
        *ORIGIN.get_or_init(Instant::now)
    }

    pub fn init() {
        let _ = origin();
    }

    pub fn enabled() -> bool {
        true
    }

    pub fn mark(phase: &str) {
        let ms = origin().elapsed().as_secs_f64() * 1000.0;
        emit(phase, ms);
    }

    /// Time `f` and emit its duration as a `d:`-prefixed mark.
    ///
    /// The value is a duration, not a timestamp, which is the convention the
    /// `d:` prefix carries throughout the stream.
    pub fn span<T>(phase: &str, f: impl FnOnce() -> T) -> T {
        let start = Instant::now();
        let out = f();
        emit(
            &format!("d:{phase}"),
            start.elapsed().as_secs_f64() * 1000.0,
        );
        out
    }

    /// Record a mark whose timestamp was taken elsewhere — used for frontend
    /// marks, where `performance.now()` is relative to the webview's own
    /// origin, not ours.
    pub fn emit(phase: &str, ms: f64) {
        // Built through serde_json rather than string-formatted: `phase` can
        // originate in the frontend, and a stray quote would corrupt the
        // NDJSON stream the scripts parse.
        let line = serde_json::json!({ "dreamd_perf": 1, "phase": phase, "ms": ms });
        let mut err = std::io::stderr().lock();
        let _ = writeln!(err, "{line}");
        let _ = err.flush();
    }
}

#[cfg(not(feature = "perf"))]
mod imp {
    #[inline(always)]
    pub fn init() {}

    #[inline(always)]
    pub fn enabled() -> bool {
        false
    }

    #[inline(always)]
    pub fn mark(_phase: &str) {}

    #[inline(always)]
    pub fn emit(_phase: &str, _ms: f64) {}

    /// Calls `f` and nothing else — the timing disappears with the feature.
    #[inline(always)]
    pub fn span<T>(_phase: &str, f: impl FnOnce() -> T) -> T {
        f()
    }
}

pub use imp::{emit, enabled, init, mark, span};
