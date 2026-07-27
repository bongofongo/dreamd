//! The headless half of the CLI: everything reachable from the settings panel
//! is also reachable from the shell. These subcommands run and exit before the
//! Tauri builder is ever touched, so they cost a config read and nothing else.
//!
//! They deliberately share the panel's code paths — `config::set_global_key`
//! and `theme::save_user` — so a value set here and a value set in the panel
//! land in the same file in the same shape.

use crate::annotations::{Highlight, HighlightState, Store};
use crate::config::{self, Config};
use crate::marks_file;
use crate::theme;
use clap::Subcommand;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Subcommand)]
pub enum Cmd {
    /// List, switch, inspect and create reading themes.
    Theme {
        #[command(subcommand)]
        action: ThemeCmd,
    },
    /// Inspect and edit `~/.config/dreamd/config.toml`.
    Config {
        #[command(subcommand)]
        action: ConfigCmd,
    },
    /// Inspect and prune this repo's saved marks.
    Marks {
        #[command(subcommand)]
        action: MarksCmd,
    },
    /// Serve dreamd's MCP tools over stdio, for an agent to spawn.
    ///
    /// `claude mcp add dreamd -- dreamd mcp`, from anywhere inside the repo.
    /// This process is a shim: it answers the handshake itself and forwards
    /// tool calls to the running dreamd window over that repo's socket, so
    /// Claude Code sees the tools whether or not dreamd is open, and a call
    /// made while it is closed says so instead of failing the session.
    Mcp,
}

#[derive(Subcommand)]
pub enum ThemeCmd {
    /// Every bundled and user theme, with the active one marked.
    List,
    /// Switch the active theme and save it to the global config.
    Set { name: String },
    /// Print a theme's full stylesheet (base rules + palette) to stdout.
    Show { name: Option<String> },
    /// Copy a theme into `~/.config/dreamd/themes/` so you can edit it.
    New {
        name: String,
        /// The theme to copy. Defaults to the active one.
        #[arg(long)]
        from: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum ConfigCmd {
    /// Print the path of the global config file.
    Path,
    /// Open the global config in `$VISUAL`/`$EDITOR`.
    Edit,
    /// Print one dotted key, e.g. `keymap.palette`.
    Get { key: String },
    /// Set one dotted key, e.g. `dreamd config set keymap.palette Ctrl+Space`.
    Set { key: String, value: String },
}

#[derive(Subcommand)]
pub enum MarksCmd {
    /// Print the path of this repo's marks file.
    Path,
    /// Drop marks from this repo's file. With no flag, report and change
    /// nothing.
    ///
    /// A destructive verb with a read-only default: `prune` on its own prints
    /// what each flag would remove, so the first thing anyone types is a dry
    /// run rather than a deletion.
    Prune {
        /// Drop stale marks carrying no question — the ones a `git checkout`
        /// stranded and nobody annotated.
        #[arg(long)]
        stale: bool,
        /// Drop marks an agent answered longer ago than this: `30d`, `12h`,
        /// `90m`. Annotated marks are exactly what this removes, which is the
        /// difference between it and `--stale`: the question was answered.
        #[arg(long, value_name = "AGE", value_parser = parse_age)]
        older_than: Option<u64>,
    },
}

/// Is another dreamd already serving this repo?
///
/// The lock is the MCP socket bind (`mcp::server`'s module doc), and this is a
/// read of it: a socket that accepts a connection has a live owner. It lives in
/// the library rather than in `main.rs` for the reason [`crate::guard`] does —
/// `main.rs` is a `[[bin]]` and cannot be imported, so a predicate that decides
/// whether this process may write the user's marks would have no test at all.
/// `examples/marks_check.rs` drives it against a real socket; `marks prune` and
/// `main.rs`'s startup are the two callers.
pub fn repo_is_claimed(root: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(crate::mcp::server::socket_path(root)).is_ok()
}

/// Run a subcommand. The `Err` message is printed to stderr by the caller,
/// which then exits non-zero.
pub fn run(cmd: Cmd) -> Result<(), String> {
    let repo_root = crate::resolve_repo_root(None);
    // `mcp` runs before the config read the other two need: it reads no
    // configuration at all, and every millisecond here is on the path of an
    // agent's first tool call. It also never returns until stdin closes.
    if matches!(cmd, Cmd::Mcp) {
        return crate::mcp::shim::run(&repo_root);
    }
    let cfg = Config::load(&repo_root);
    match cmd {
        Cmd::Theme { action } => theme_cmd(action, &cfg),
        Cmd::Config { action } => config_cmd(action, &repo_root),
        Cmd::Marks { action } => marks_cmd(action, &repo_root),
        // Handled above, before the config read.
        Cmd::Mcp => unreachable!(),
    }
}

/// The appearance to resolve a theme for from the shell.
///
/// There is no window here, and `Window::theme()` is the only way to read the
/// OS appearance, so `mode = "system"` cannot be answered honestly. Dark is the
/// documented fallback everywhere else in the app; `theme show` prints the whole
/// stylesheet including both mode blocks either way, so the only thing riding on
/// this is which `--syntax-theme` `theme list` would report.
fn cli_scheme(cfg: &Config) -> theme::Scheme {
    theme::scheme_for(cfg, theme::Scheme::Dark)
}

fn theme_cmd(action: ThemeCmd, cfg: &Config) -> Result<(), String> {
    match action {
        ThemeCmd::List => {
            let scheme = cli_scheme(cfg);
            let active = theme::resolve(cfg, scheme).name;
            for info in theme::list() {
                let mark = if Some(&info.name) == active.as_ref() {
                    "*"
                } else {
                    " "
                };
                let modes = match theme::palette(&info.name) {
                    Some(css) if theme::has_mode_blocks(&css) => "dark+light",
                    _ => "one mode",
                };
                println!("{mark} {:<22} {:<9} {}", info.name, info.kind, modes);
            }
            println!(
                "\nappearance: {} ({})",
                match cfg.mode() {
                    config::Mode::System => "system",
                    config::Mode::Light => "light",
                    config::Mode::Dark => "dark",
                },
                match scheme {
                    theme::Scheme::Light => "light",
                    theme::Scheme::Dark => "dark",
                },
            );
            // Not listed as themes — they resolve, but they are not names
            // anyone should be typing now.
            let aliases: Vec<&str> = theme::ALIASES.iter().map(|(a, _, _)| *a).collect();
            println!("older names still accepted: {}", aliases.join(", "));
            if let Some(path) = &cfg.theme_css {
                println!("\ntheme_css overrides the palette: {}", path.display());
            }
            Ok(())
        }
        ThemeCmd::Set { name } => {
            if theme::palette(&name).is_none() {
                return Err(format!("no theme named {name:?} (try `dreamd theme list`)"));
            }
            // A legacy name is rewritten into the family plus the mode it used
            // to mean, in one atomic patch rather than two writes. This is the
            // self-healing path: a config migrates the first time it is
            // touched, and the alias table stays a shim rather than a second
            // naming scheme.
            let migrate =
                theme::dealias(&name).filter(|_| theme::BUNDLED.iter().all(|(n, _)| *n != name));
            match migrate {
                Some((family, scheme)) => {
                    let mode = match scheme {
                        theme::Scheme::Light => "light",
                        theme::Scheme::Dark => "dark",
                    };
                    let mut patch = toml::Table::new();
                    patch.insert("theme".into(), family.into());
                    patch.insert("mode".into(), mode.into());
                    config::patch_global(patch)?;
                    println!("theme = {family}, mode = {mode}");
                    eprintln!("dreamd: {name} is now {family} plus mode = {mode}");
                }
                None => {
                    config::set_global_key("theme", name.as_str().into())?;
                    println!("theme = {name}");
                }
            }
            if cfg.theme_css.is_some() {
                eprintln!(
                    "dreamd: warning — theme_css is set, so the palette is ignored until you clear it"
                );
            }
            Ok(())
        }
        ThemeCmd::Show { name } => {
            let css = match name {
                Some(name) => {
                    theme::css_for(&name).ok_or_else(|| format!("no theme named {name:?}"))?
                }
                None => theme::resolve(cfg, cli_scheme(cfg)).css,
            };
            print!("{css}");
            Ok(())
        }
        ThemeCmd::New { name, from } => {
            let source = from.unwrap_or_else(|| {
                theme::resolve(cfg, cli_scheme(cfg))
                    .name
                    .unwrap_or_else(|| theme::DEFAULT_THEME.to_string())
            });
            let css =
                theme::palette(&source).ok_or_else(|| format!("no theme named {source:?}"))?;
            if let Some(existing) = theme::user_path(&name) {
                if existing.exists() {
                    return Err(format!("{} already exists", existing.display()));
                }
            }
            let path = theme::save_user(&name, &css)?;
            println!("{}", path.display());
            eprintln!("dreamd: edit it, then `dreamd theme set {name}`");
            Ok(())
        }
    }
}

fn config_cmd(action: ConfigCmd, repo_root: &std::path::Path) -> Result<(), String> {
    match action {
        ConfigCmd::Path => {
            println!("{}", config::global_path().display());
            let local = config::local_path(repo_root);
            if local.exists() {
                println!("{}  (overrides the above in this repo)", local.display());
            }
            Ok(())
        }
        ConfigCmd::Edit => edit(config::global_path()),
        ConfigCmd::Get { key } => {
            // Read the merged view, so `get` answers "what is in effect here?"
            // rather than "what does the global file happen to say?".
            let mut table = config::global_table();
            if let Some(local) = std::fs::read_to_string(config::local_path(repo_root))
                .ok()
                .and_then(|t| t.parse::<toml::Table>().ok())
            {
                for (k, v) in local {
                    table.insert(k, v);
                }
            }
            match config::get_key(&table, &key) {
                Some(value) => {
                    println!("{}", render_value(value));
                    Ok(())
                }
                None => Err(format!("{key} is unset (using the default)")),
            }
        }
        ConfigCmd::Set { key, value } => {
            config::set_global_key(&key, config::parse_value(&value))?;
            println!("{key} = {value}");
            if config::local_override_keys(repo_root).contains(&key) {
                eprintln!(
                    "dreamd: warning — {} sets {key} too, and wins in this repo",
                    config::local_path(repo_root).display()
                );
            }
            Ok(())
        }
    }
}

// ---- marks ---------------------------------------------------------------

/// The age `prune` reports against when no `--older-than` was given. Only a
/// number in a dry run — nothing is deleted on it.
const REPORT_AGE: u64 = 30 * 24 * 60 * 60;

/// `30d`, `12h`, `90m`, `45s`, `2w` -> seconds.
///
/// A bare number is refused rather than assumed: "prune --older-than 30" means
/// days to one person and seconds to another, and the wrong guess deletes
/// answered questions a month early.
fn parse_age(raw: &str) -> Result<u64, String> {
    let raw = raw.trim();
    // Split on the last *character*, never on `len() - 1`: this string comes
    // straight off the command line, and `str::split_at` panics on a byte index
    // inside a multi-byte character — `--older-than 30é` would abort with a
    // backtrace where clap should have printed a usage error.
    let mut chars = raw.chars();
    let unit = chars.next_back();
    let digits = chars.as_str();
    let scale = match unit {
        Some('s') => 1,
        Some('m') => 60,
        Some('h') => 60 * 60,
        Some('d') => 24 * 60 * 60,
        Some('w') => 7 * 24 * 60 * 60,
        _ => return Err(format!("expected a duration like 30d or 12h, got {raw:?}")),
    };
    let n: u64 = digits
        .parse()
        .map_err(|_| format!("expected a duration like 30d or 12h, got {raw:?}"))?;
    Ok(n * scale)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Stale, unanswered, and carrying no question — the `--stale` set.
///
/// The same policy `marks_file::enforce_cap` applies at the cap: an annotated
/// mark is a question in flight and `--stale` never touches one, however stale
/// its quote has gone.
fn is_stale_droppable(h: &Highlight) -> bool {
    h.state == HighlightState::Stale && h.annotation.is_none() && h.resolved.is_none()
}

/// Answered before `cutoff` — the `--older-than` set.
///
/// This one *does* remove annotated marks, and that is the point: the question
/// was answered, and the answer is what makes the mark disposable.
fn resolved_before(h: &Highlight, cutoff: u64) -> bool {
    h.resolved.as_ref().is_some_and(|r| r.at < cutoff)
}

fn marks_cmd(action: MarksCmd, repo_root: &Path) -> Result<(), String> {
    let path = marks_file::path_for(repo_root);
    match action {
        MarksCmd::Path => {
            println!("{}", path.display());
            if !path.exists() {
                eprintln!("dreamd: nothing saved for this repo yet");
            }
            Ok(())
        }
        MarksCmd::Prune { stale, older_than } => {
            if !path.exists() {
                return Err(format!("nothing saved for this repo ({})", path.display()));
            }
            // Through `load`, so what is written back has already been through
            // every admission rule — a prune is also the moment a file that
            // drifted gets tidied.
            let (mut highlights, stack) = marks_file::load(repo_root).into_parts();
            let now = now_secs();
            let report_cutoff = now.saturating_sub(REPORT_AGE);
            let queued = stack.len();

            if !stale && older_than.is_none() {
                println!("{}", path.display());
                println!("  {} marks, {queued} queued", highlights.len());
                println!(
                    "  {} stale and unannotated       (--stale)",
                    highlights.iter().filter(|h| is_stale_droppable(h)).count()
                );
                println!(
                    "  {} resolved over 30 days ago   (--older-than 30d)",
                    highlights
                        .iter()
                        .filter(|h| resolved_before(h, report_cutoff))
                        .count()
                );
                eprintln!("dreamd: nothing removed; pass a flag to remove it");
                return Ok(());
            }

            let cutoff = older_than.map(|age| now.saturating_sub(age));
            let before = highlights.len();
            highlights.retain(|h| {
                let by_stale = stale && is_stale_droppable(h);
                let by_age = cutoff.is_some_and(|c| resolved_before(h, c));
                !by_stale && !by_age
            });
            let removed = before - highlights.len();

            // `admit` rather than a hand-rolled filter, so a stack entry whose
            // mark just went is dropped by the same code the loader uses.
            let root = repo_root
                .canonicalize()
                .unwrap_or_else(|_| repo_root.to_path_buf());
            let kept = Store::from_parts(highlights, stack);
            let kept = marks_file::admit(&root, marks_file::doc_from(&root, &kept));
            if repo_is_claimed(repo_root) {
                eprintln!(
                    "dreamd: warning — dreamd is open on this repo and will overwrite this file"
                );
            }
            marks_file::save(repo_root, &kept)
                .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
            println!(
                "removed {removed}, kept {} ({} queued)",
                kept.parts().0.len(),
                kept.parts().1.len()
            );
            Ok(())
        }
    }
}

/// Print a TOML value the way a shell caller wants it: bare strings, no quotes.
fn render_value(value: &toml::Value) -> String {
    match value {
        toml::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn edit(path: PathBuf) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    if !path.exists() {
        std::fs::write(&path, "").map_err(|e| format!("cannot create {}: {e}", path.display()))?;
    }
    let editor = std::env::var("VISUAL").or_else(|_| std::env::var("EDITOR"));
    match editor {
        Ok(editor) if !editor.trim().is_empty() => {
            // `$EDITOR` may carry flags ("code -w"), so split on whitespace
            // rather than treating the whole string as a program name.
            let mut parts = editor.split_whitespace();
            let program = parts.next().unwrap_or("vi");
            let status = std::process::Command::new(program)
                .args(parts)
                .arg(&path)
                .status()
                .map_err(|e| format!("cannot run {program}: {e}"))?;
            status
                .success()
                .then_some(())
                .ok_or_else(|| format!("{program} exited with {status}"))
        }
        _ => open::that(&path).map_err(|e| format!("cannot open {}: {e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    //! Only the pure half of `marks prune`: what it decides to drop, and how it
    //! reads a duration. Everything that touches a file is
    //! `examples/marks_check.rs`'s — `marks_file::path_for` reads the real
    //! `config_dir()`, and `cargo test` is not allowed near it.
    use super::*;
    use crate::annotations::Resolution;

    fn mark(
        state: HighlightState,
        annotation: Option<&str>,
        resolved_at: Option<u64>,
    ) -> Highlight {
        Highlight {
            id: "h1".into(),
            quote: "q".into(),
            state,
            annotation: annotation.map(str::to_string),
            resolved: resolved_at.map(|at| Resolution { note: None, at }),
            ..Highlight::default()
        }
    }

    #[test]
    fn a_duration_needs_a_unit() {
        assert_eq!(parse_age("30d"), Ok(30 * 24 * 60 * 60));
        assert_eq!(parse_age("12h"), Ok(12 * 60 * 60));
        assert_eq!(parse_age("90m"), Ok(90 * 60));
        assert_eq!(parse_age("45s"), Ok(45));
        assert_eq!(parse_age(" 2w "), Ok(14 * 24 * 60 * 60));
        // A bare number is refused rather than guessed: "30" means days to one
        // person and seconds to another, and one of those deletes a month of
        // answered questions early.
        assert!(parse_age("30").is_err());
        assert!(parse_age("").is_err());
        assert!(parse_age("d").is_err());
        assert!(parse_age("-1d").is_err());
        assert!(parse_age("30 days").is_err());
        // An error, not a panic: this string comes off the command line, and
        // splitting it at `len() - 1` lands inside the `é`.
        assert!(parse_age("30é").is_err());
        assert!(parse_age("あ").is_err());
        assert!(parse_age("３０日").is_err());
    }

    #[test]
    fn stale_pruning_never_takes_a_question() {
        // The same line `enforce_cap` draws: an annotated mark is a question in
        // flight, and a stale quote is usually mid-`git checkout`.
        assert!(is_stale_droppable(&mark(HighlightState::Stale, None, None)));
        assert!(!is_stale_droppable(&mark(
            HighlightState::Stale,
            Some("why?"),
            None
        )));
        assert!(!is_stale_droppable(&mark(
            HighlightState::Active,
            None,
            None
        )));
        // Answered marks are `--older-than`'s business, not `--stale`'s, even
        // when the quote has gone stale since.
        assert!(!is_stale_droppable(&mark(
            HighlightState::Stale,
            None,
            Some(1)
        )));
    }

    #[test]
    fn age_pruning_only_takes_answered_marks() {
        let cutoff = 1_000;
        assert!(resolved_before(
            &mark(HighlightState::Active, Some("why?"), Some(999)),
            cutoff
        ));
        assert!(!resolved_before(
            &mark(HighlightState::Active, Some("why?"), Some(1_001)),
            cutoff
        ));
        // Never answered — no age to be older than, however old the mark is.
        assert!(!resolved_before(
            &mark(HighlightState::Stale, None, None),
            cutoff
        ));
    }
}
