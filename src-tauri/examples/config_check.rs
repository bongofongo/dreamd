//! Correctness harness for config layering and write-back. Exits non-zero on
//! the first failure.
//!
//! This crate has no `#[cfg(test)]` unit tests — correctness lives in runnable
//! examples (see `locate_check.rs`). Config layering earns one because it is
//! pure, table-driven logic whose failure mode is silent: a merge that quietly
//! resets a user's keybinds looks exactly like a config that loaded fine.
//!
//! ```sh
//! cargo run --example config_check
//! ```

use dreamd::config::{self, Config};
use std::path::{Path, PathBuf};

fn main() {
    let root = PathBuf::from(std::env::temp_dir()).join("dreamd-config-check");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("temp dir");

    // `config_dir()` reads XDG_CONFIG_HOME on every call, so pointing it at a
    // scratch directory keeps the real config untouched.
    std::env::set_var("XDG_CONFIG_HOME", root.join("xdg"));
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).expect("repo dir");

    let mut checks = Checks::default();

    // --- defaults ---------------------------------------------------------
    write_global("");
    write_local(&repo, None);
    let cfg = Config::load(&repo);
    checks.eq("default palette key is unset", cfg.theme, None);
    checks.eq("default keybind", cfg.keymap.palette, "Ctrl+F");
    checks.eq("default quick_highlight", cfg.keymap.quick_highlight, true);

    // --- global only ------------------------------------------------------
    write_global("theme = \"nord\"\n\n[keymap]\npalette = \"Ctrl+Space\"\n");
    let cfg = Config::load(&repo);
    checks.eq("global theme", cfg.theme, Some("nord".into()));
    checks.eq("global keybind", cfg.keymap.palette, "Ctrl+Space");
    checks.eq(
        "untouched keybind keeps its default",
        cfg.keymap.toggle_stack,
        "Ctrl+O",
    );

    // --- the merge bug this replaced --------------------------------------
    // A repo-local file that mentions neither `[keymap]` nor `tmux_autodetect`
    // used to reset both to their defaults, silently wiping global keybinds in
    // that repo only.
    write_global("tmux_autodetect = false\n\n[keymap]\npalette = \"Ctrl+Space\"\n");
    write_local(&repo, Some("tmux_target = \"%3\"\n"));
    let cfg = Config::load(&repo);
    checks.eq("local file preserves global keybind", cfg.keymap.palette, "Ctrl+Space");
    checks.eq(
        "local file preserves global tmux_autodetect",
        cfg.tmux_autodetect,
        false,
    );
    checks.eq("local value applies", cfg.tmux_target, Some("%3".into()));

    // --- per-key keymap merge ---------------------------------------------
    write_global("[keymap]\npalette = \"Ctrl+Space\"\nhighlight = \"Ctrl+K\"\n");
    write_local(&repo, Some("[keymap]\nhighlight = \"Ctrl+J\"\n"));
    let cfg = Config::load(&repo);
    checks.eq("local overrides one key", cfg.keymap.highlight, "Ctrl+J");
    checks.eq("and leaves the rest", cfg.keymap.palette, "Ctrl+Space");

    // --- theme / theme_css pairing ----------------------------------------
    write_global("theme_css = \"/tmp/does-not-exist.css\"\n");
    write_local(&repo, Some("theme = \"tokyo-night\"\n"));
    let cfg = Config::load(&repo);
    checks.eq("a repo naming a theme clears a global theme_css", cfg.theme_css, None);
    checks.eq("and that theme wins", cfg.theme, Some("tokyo-night".into()));

    // --- repo-local theme_css is untrusted input --------------------------
    write_global("");
    write_local(&repo, Some("theme_css = \"/etc/passwd\"\n"));
    let cfg = Config::load(&repo);
    checks.eq("repo-local theme_css is refused", cfg.theme_css, None);

    // --- write-back -------------------------------------------------------
    write_global("# hand-written\ntmux_target = \"work:0.1\"\n\n[keymap]\npalette = \"Ctrl+Space\"\n");
    write_local(&repo, None);
    config::set_global_key("theme", "nord".into()).expect("set theme");
    config::set_global_key("keymap.toggle_stack", "Ctrl+B".into()).expect("set keybind");
    let text = std::fs::read_to_string(config::global_path()).expect("read back");
    checks.ok("write keeps unrelated keys", text.contains("work:0.1"));
    checks.ok("write keeps unrelated keybinds", text.contains("Ctrl+Space"));
    checks.ok("write applies the new key", text.contains("Ctrl+B"));
    checks.ok(
        "write does not spell out defaults",
        !text.contains("send_stack") && !text.contains("tmux_autodetect"),
    );
    let cfg = Config::load(&repo);
    checks.eq("written config reloads", cfg.theme, Some("nord".into()));
    checks.eq("written keybind reloads", cfg.keymap.toggle_stack, "Ctrl+B");

    // --- a bad value is rejected before it reaches the file ---------------
    let before = std::fs::read_to_string(config::global_path()).expect("read");
    let bad = config::set_global_key("keymap.palette", 42.into());
    checks.ok("a mistyped value is rejected", bad.is_err());
    let after = std::fs::read_to_string(config::global_path()).expect("read");
    checks.ok("and the file is untouched", before == after);

    // --- dotted lookups ---------------------------------------------------
    let table = config::global_table();
    checks.eq(
        "dotted get",
        config::get_key(&table, "keymap.palette").and_then(|v| v.as_str()),
        Some("Ctrl+Space"),
    );
    checks.ok(
        "flat_keys reaches nested keys",
        config::flat_keys(&table).contains(&"keymap.toggle_stack".to_string()),
    );
    write_local(&repo, Some("theme = \"nord\"\n"));
    checks.eq(
        "local override keys are reported",
        config::local_override_keys(&repo),
        vec!["theme".to_string()],
    );

    let _ = std::fs::remove_dir_all(&root);
    checks.finish();
}

fn write_global(text: &str) {
    let path = config::global_path();
    std::fs::create_dir_all(path.parent().unwrap()).expect("config dir");
    std::fs::write(path, text).expect("write global");
}

fn write_local(repo: &Path, text: Option<&str>) {
    let path = config::local_path(repo);
    match text {
        Some(t) => std::fs::write(path, t).expect("write local"),
        None => {
            let _ = std::fs::remove_file(path);
        }
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
        println!("config_check: {} passed, {} failed", self.passed, self.failed);
        if self.failed > 0 {
            std::process::exit(1);
        }
    }
}
