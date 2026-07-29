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

use dreamd::config::{self, Config, Mode, PermissionMode, Position};
use dreamd::theme;
use std::path::Path;

fn main() {
    let root = std::env::temp_dir().join("dreamd-config-check");
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
    checks.eq(
        "local file preserves global keybind",
        cfg.keymap.palette,
        "Ctrl+Space",
    );
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
    checks.eq(
        "a repo naming a theme clears a global theme_css",
        cfg.theme_css,
        None,
    );
    checks.eq("and that theme wins", cfg.theme, Some("tokyo-night".into()));

    // --- repo-local theme_css is untrusted input --------------------------
    write_global("");
    write_local(&repo, Some("theme_css = \"/etc/passwd\"\n"));
    let cfg = Config::load(&repo);
    checks.eq("repo-local theme_css is refused", cfg.theme_css, None);

    // --- appearance mode --------------------------------------------------
    // Unlike theme_css, `mode` reads no files and injects nothing, so a
    // repo-local file is allowed to set it.
    write_global("");
    write_local(&repo, None);
    checks.eq(
        "default mode is system",
        Config::load(&repo).mode(),
        Mode::System,
    );

    write_global("mode = \"light\"\n");
    checks.eq("global mode", Config::load(&repo).mode(), Mode::Light);

    write_local(&repo, Some("mode = \"dark\"\n"));
    checks.eq(
        "repo-local mode wins",
        Config::load(&repo).mode(),
        Mode::Dark,
    );
    checks.ok(
        "and is reported as an override",
        config::local_override_keys(&repo).contains(&"mode".to_string()),
    );

    write_local(&repo, None);
    write_global("");
    config::set_global_key("mode", "dark".into()).expect("set mode");
    checks.eq(
        "written mode reloads",
        Config::load(&repo).mode(),
        Mode::Dark,
    );

    let before = std::fs::read_to_string(config::global_path()).expect("read");
    checks.ok(
        "an unknown mode is rejected",
        config::set_global_key("mode", "sepia".into()).is_err(),
    );
    checks.ok(
        "and the file is untouched",
        before == std::fs::read_to_string(config::global_path()).expect("read"),
    );

    // A legacy palette name implies an appearance, but only while `mode` is
    // still the default — an explicit `mode` is the user saying otherwise.
    write_global("theme = \"gruvbox-dark\"\n");
    write_local(&repo, None);
    checks.eq(
        "a legacy theme name pins its appearance",
        theme::scheme_for(&Config::load(&repo), theme::Scheme::Light),
        theme::Scheme::Dark,
    );
    write_global("theme = \"gruvbox-dark\"\nmode = \"system\"\n");
    checks.eq(
        "an explicit mode beats the implied one",
        theme::scheme_for(&Config::load(&repo), theme::Scheme::Light),
        theme::Scheme::Light,
    );

    // --- the agent section ------------------------------------------------
    write_global("");
    write_local(&repo, None);
    let cfg = Config::load(&repo);
    checks.eq(
        "default pane position",
        cfg.agent.position,
        Position::Bottom,
    );
    checks.eq(
        "default permission mode",
        cfg.agent.permission_mode,
        PermissionMode::AcceptEdits,
    );
    checks.eq(
        "default tree width",
        cfg.ui.tree_width,
        config::TREE_WIDTH_DEFAULT,
    );

    write_global("[agent]\nposition = \"right\"\npermission_mode = \"plan\"\n");
    write_local(&repo, Some("[agent]\nposition = \"bottom\"\n"));
    let cfg = Config::load(&repo);
    checks.eq(
        "a repo may move the pane",
        cfg.agent.position,
        Position::Bottom,
    );
    checks.eq(
        "and leaves the global mode standing",
        cfg.agent.permission_mode,
        PermissionMode::Plan,
    );

    // Same shape as the theme_css check above: repo content does not get to
    // decide how much the agent may do without asking.
    write_local(
        &repo,
        Some("[agent]\npermission_mode = \"bypass-permissions\"\n"),
    );
    checks.eq(
        "repo-local permission_mode is refused",
        Config::load(&repo).agent.permission_mode,
        PermissionMode::Plan,
    );

    // A local file naming only `[agent]` must not blank the global `[keymap]` —
    // the merge bug, re-checked for the section this pass adds.
    write_global("[agent]\nposition = \"right\"\n\n[keymap]\npalette = \"Ctrl+Space\"\n");
    write_local(&repo, Some("[agent]\nposition = \"bottom\"\n"));
    checks.eq(
        "an [agent]-only local file preserves global keybinds",
        Config::load(&repo).keymap.palette,
        "Ctrl+Space",
    );

    // --- tree width write-back --------------------------------------------
    write_global("");
    write_local(&repo, None);
    config::set_global_key("ui.tree_width", 320.into()).expect("set width");
    checks.eq(
        "a written width reloads",
        Config::load(&repo).ui.tree_width,
        320u32,
    );
    // Out of range is clamped rather than rejected: the drag handle's job is to
    // leave a usable tree, not to take the rest of the config down with a bad
    // number. The file keeps what was written; every reader sees the clamp.
    let cfg = config::set_global_key("ui.tree_width", 5.into()).expect("set narrow width");
    checks.eq("a too-narrow width clamps up", cfg.ui.tree_width, 140u32);
    checks.eq(
        "and the clamp survives a reload",
        Config::load(&repo).ui.tree_width,
        140u32,
    );
    let cfg = config::set_global_key("ui.tree_width", 4000.into()).expect("set wide width");
    checks.eq("a too-wide width clamps down", cfg.ui.tree_width, 600u32);
    checks.ok(
        "a negative width is rejected outright",
        config::set_global_key("ui.tree_width", (-40).into()).is_err(),
    );

    // --- window chrome write-back ------------------------------------------
    //
    // The settings panel's two toggles, through the same path they take: a
    // write to the global file and a reload. `strip_untrusted` is unit-tested;
    // what only a real file can show is that the pair survives a round trip
    // and that a repo-local file next to it changes nothing.
    write_global("");
    write_local(&repo, None);
    checks.eq(
        "the menubar ships off",
        Config::load(&repo).ui.menubar,
        false,
    );
    checks.eq(
        "the titlebar ships at the platform default",
        Config::load(&repo).ui.titlebar,
        config::TITLEBAR_DEFAULT,
    );
    config::set_global_key("ui.menubar", true.into()).expect("show the menubar");
    config::set_global_key("ui.titlebar", true.into()).expect("show the titlebar");
    let cfg = Config::load(&repo);
    checks.ok("a shown menubar reloads", cfg.ui.menubar);
    checks.ok("a shown titlebar reloads", cfg.ui.titlebar);
    write_local(&repo, Some("[ui]\nmenubar = false\ntitlebar = false\n"));
    let cfg = Config::load(&repo);
    checks.ok("a repo cannot hide the menubar", cfg.ui.menubar);
    checks.ok("a repo cannot hide the titlebar", cfg.ui.titlebar);
    // And the harmless sibling in the same section still lands, so the refusal
    // is per-key rather than a dropped `[ui]` table.
    write_local(&repo, Some("[ui]\ntree_width = 300\ntitlebar = false\n"));
    let cfg = Config::load(&repo);
    checks.eq(
        "the same [ui] table still resizes the tree",
        cfg.ui.tree_width,
        300u32,
    );
    checks.ok("while its titlebar key is dropped", cfg.ui.titlebar);

    // --- the hidden tmux keybind -------------------------------------------
    write_global("");
    write_local(&repo, None);
    checks.eq(
        "send_stack_tmux ships unbound",
        Config::load(&repo).keymap.send_stack_tmux,
        None,
    );
    write_global("[keymap]\nsend_stack_tmux = \"Ctrl+Alt+Enter\"\n");
    checks.eq(
        "and is read when bound by hand",
        Config::load(&repo).keymap.send_stack_tmux,
        Some("Ctrl+Alt+Enter".to_string()),
    );
    // "Reset all shortcuts" patches the global file with `default_keymap()`.
    // The binding is skipped on serialize, so it is not in that patch and the
    // hand-set value survives — which is the whole point of a hidden binding.
    config::patch_global(
        toml::Table::try_from(dreamd::config::Keymap::default())
            .map(|t| {
                let mut outer = toml::Table::new();
                outer.insert("keymap".into(), t.into());
                outer
            })
            .expect("keymap table"),
    )
    .expect("reset shortcuts");
    checks.eq(
        "and a shortcut reset does not clear it",
        Config::load(&repo).keymap.send_stack_tmux,
        Some("Ctrl+Alt+Enter".to_string()),
    );

    // --- write-back -------------------------------------------------------
    write_global(
        "# hand-written\ntmux_target = \"work:0.1\"\n\n[keymap]\npalette = \"Ctrl+Space\"\n",
    );
    write_local(&repo, None);
    config::set_global_key("theme", "nord".into()).expect("set theme");
    config::set_global_key("keymap.toggle_stack", "Ctrl+B".into()).expect("set keybind");
    let text = std::fs::read_to_string(config::global_path()).expect("read back");
    checks.ok("write keeps unrelated keys", text.contains("work:0.1"));
    checks.ok(
        "write keeps unrelated keybinds",
        text.contains("Ctrl+Space"),
    );
    checks.ok("write applies the new key", text.contains("Ctrl+B"));
    checks.ok(
        "write does not spell out defaults",
        !text.contains("send_stack")
            && !text.contains("tmux_autodetect")
            // `patch_global` writes the raw table, not a serialized `Config`, so a
            // defaulted key that was never patched is never emitted. Pinned here
            // because it is the reason adding `mode` did not start rewriting
            // everyone's config file.
            && !text.contains("mode"),
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
        println!(
            "config_check: {} passed, {} failed",
            self.passed, self.failed
        );
        if self.failed > 0 {
            std::process::exit(1);
        }
    }
}
