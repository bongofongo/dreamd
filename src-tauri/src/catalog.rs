//! The repo walk's two products — the file tree and the fuzzy search index —
//! behind one readiness gate.
//!
//! They come from a single walk and share a single lifecycle, so they are one
//! type rather than two independent `Mutex`es. The gate is what makes a
//! *deferred* build possible: on `dreamd file.md` the walk moves to a
//! background thread and the commands that need it block until it lands
//! instead of the whole process blocking before the window exists.
//!
//! A directory or no-arg launch calls [`Catalog::build`] synchronously before
//! the Tauri builder, exactly as before, so it never waits on anything.

use std::path::Path;
use std::sync::{Condvar, Mutex, MutexGuard};

use crate::fs_walk::{self, FileNode};
use crate::perf;
use crate::search::SearchIndex;

struct Built {
    tree: FileNode,
    index: SearchIndex,
}

pub struct Catalog {
    built: Mutex<Option<Built>>,
    ready: Condvar,
}

impl Catalog {
    /// An empty catalog. Every `wait_*` blocks until a `build` or `rebuild`
    /// fills it.
    pub fn pending() -> Self {
        Catalog {
            built: Mutex::new(None),
            ready: Condvar::new(),
        }
    }

    /// Walk, index, build the tree, store the result and wake every waiter.
    /// Safe to call on a background thread: the walk happens outside the lock,
    /// so waiters are only blocked for the handover.
    pub fn build(&self, repo_root: &Path, extra_ignores: &[String]) {
        self.replace(walk(repo_root, extra_ignores));
    }

    /// Settle the gate with an empty tree, without walking anything.
    ///
    /// For the launch that has no repo to walk at all: `wait_tree` blocks
    /// forever on a gate nothing will ever open, so the frontend's boot would
    /// hang on `list_markdown_files` rather than reaching its empty state.
    pub fn settle_empty(&self, repo_root: &Path) {
        self.replace(Built {
            tree: fs_walk::build_tree(repo_root, &[]),
            index: SearchIndex::build(repo_root, &[]),
        });
    }

    /// Re-walk and replace, handing back the fresh tree — the watcher path.
    ///
    /// Racing an in-flight initial build is last-writer-wins. That is
    /// deliberate: two rapid `rebuild_index` calls already race the same way,
    /// and the window is tiny in practice because `watcher` only spawns inside
    /// `.setup()` and debounces 60 ms on top of that.
    pub fn rebuild(&self, repo_root: &Path, extra_ignores: &[String]) -> FileNode {
        let built = walk(repo_root, extra_ignores);
        let tree = built.tree.clone();
        self.replace(built);
        tree
    }

    /// Block until built, then clone the tree.
    pub fn wait_tree(&self) -> FileNode {
        self.wait().as_ref().unwrap().tree.clone()
    }

    /// Block until built, then run the fuzzy query.
    pub fn wait_query(&self, q: &str) -> Vec<FileNode> {
        self.wait().as_ref().unwrap().index.query(q)
    }

    fn replace(&self, built: Built) {
        *self.built.lock().unwrap() = Some(built);
        self.ready.notify_all();
    }

    fn wait(&self) -> MutexGuard<'_, Option<Built>> {
        let guard = self.built.lock().unwrap();
        self.ready.wait_while(guard, |b| b.is_none()).unwrap()
    }
}

/// The marks are emitted here rather than at the call site so the synchronous
/// and the deferred path report identical phases. `perf::mark` measures from a
/// global origin set in `main`'s first statement and writes one line under the
/// stderr lock, so a `walk_done` emitted late from a background thread is still
/// a correct timestamp and still lands in one uncorrupted NDJSON stream.
fn walk(repo_root: &Path, extra_ignores: &[String]) -> Built {
    let paths = fs_walk::markdown_paths(repo_root, extra_ignores);
    perf::mark("walk_done");

    let index = SearchIndex::build(repo_root, &paths);
    perf::mark("index_built");

    let tree = fs_walk::build_tree(repo_root, &paths);
    perf::mark("tree_built");

    Built { tree, index }
}
