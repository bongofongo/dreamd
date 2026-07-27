//! The headless half of the CLI: everything reachable from the settings panel
//! is also reachable from the shell. These subcommands run and exit before the
//! Tauri builder is ever touched, so they cost a config read and nothing else.
//!
//! They deliberately share the panel's code paths — `config::set_global_key`
//! and `theme::save_user` — so a value set here and a value set in the panel
//! land in the same file in the same shape.

use crate::config::{self, Config};
use crate::theme;
use clap::Subcommand;
use std::path::PathBuf;

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

/// Run a subcommand. The `Err` message is printed to stderr by the caller,
/// which then exits non-zero.
pub fn run(cmd: Cmd) -> Result<(), String> {
    let repo_root = crate::resolve_repo_root(None);
    let cfg = Config::load(&repo_root);
    match cmd {
        Cmd::Theme { action } => theme_cmd(action, &cfg),
        Cmd::Config { action } => config_cmd(action, &repo_root),
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
