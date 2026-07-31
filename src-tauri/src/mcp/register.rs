//! How dreamd names itself to Claude Code — as a config the window hands its own
//! agent, and as a command a reader can type for the sessions it does not launch.
//!
//! # dreamd wires its own agent up
//!
//! [`crate::agent::claude`] spawns the native session itself, and already hands
//! it an inline `--settings` document for the permission hook. [`config_json`]
//! is the second such document: `--mcp-config` takes an inline JSON string too,
//! so the session dreamd starts carries dreamd's own shim, pointed at the
//! launcher path *this* build resolved. That surface therefore needs no
//! registration at all, and cannot inherit a stale one naming a binary that no
//! longer exists.
//!
//! There used to be a Register button here that ran `claude mcp add` for the
//! reader. It is gone, and with it dreamd's one write outside
//! `~/.config/dreamd` (tenet 2). What the button could never fix is what
//! remains: a Claude Code the reader starts *themselves* — in tmux, in their own
//! shell, or through the undocumented `agent.surface = "terminal"` pane, whose
//! command is four fixed literals in [`crate::pty`] with no room to interpolate
//! a path into. Those read the reader's own config, so [`add_command`] prints
//! the line that writes it and the reader runs it where their shell is.
//!
//! # Nothing is assembled
//!
//! Tenet 3. [`config_json`] is built by `serde_json` and crosses as one
//! `Command::arg`; no shell is involved on that path at all. [`add_command`] is
//! the one string here shaped *for* a shell, and it is *shown*, never executed —
//! dreamd runs no part of it. The launcher is single-quoted on the way in for
//! the same reason [`crate::agent::gate_server::settings_json`] quotes its two
//! paths: what is displayed has to mean what it says when pasted.
//!
//! # The launcher is not always `current_exe`
//!
//! A registration outlives the process that wrote it, so the path written into
//! it has to be one that still exists tomorrow. `current_exe` is that path for
//! a `.deb`, a tarball install, a `.app` and `cargo tauri dev` — and is exactly
//! wrong for an AppImage, whose binary lives on a mount point under `/tmp` that
//! is gone the moment the window closes. AppImage exports `$APPIMAGE` as the
//! stable path of the bundle itself, and the bundle forwards its arguments, so
//! `dreamd-1.2.3.AppImage mcp` is the shim. [`launcher_from`] is that rule,
//! kept pure so it can be tested without either environment.
//!
//! # Scope
//!
//! `--scope user`, deliberately, and it is now the *printed* command's problem
//! rather than a button's. [`shim`](super::shim) derives the repo root from its
//! own cwd and the socket path from that root, so one registration serves every
//! repository the reader ever opens — where the default `local` scope would put
//! the strip back on screen in the next repo, having taught nobody anything.
//! This is not hypothetical: an earlier tooltip printed the command *without* the
//! flag and with a bare `dreamd` in place of the path, and the development
//! machine ended up with `dreamd` defined in two scopes with different
//! endpoints. Whatever this module prints is a command someone will run, so it
//! is built from [`add_args`] rather than written out a second time.

use std::path::Path;
use std::process::Command;

/// The name the server is registered under.
///
/// Not free to change: Claude Code spells a server's tools
/// `mcp__<name>__<tool>`, and [`crate::agent::claude::GRANTS`] pre-grants six
/// of them by that spelling. A test below pins the two together.
pub const SERVER_NAME: &str = "dreamd";

/// The argument dreamd is invoked with to become the stdio shim.
const SHIM_ARG: &str = "mcp";

/// The full argv for `claude`, given the launcher to register.
///
/// dreamd no longer runs this itself — [`add_command`] renders it for a reader
/// to run. It stays an argv rather than a string because that is the form whose
/// shape can be asserted, and the printed version is derived from it precisely
/// so the two cannot say different things.
pub fn add_args(launcher: &str) -> Vec<String> {
    [
        "mcp",
        "add",
        SERVER_NAME,
        "--scope",
        "user",
        // Everything after this is the server's own command line, not
        // `claude`'s. Without it a launcher path beginning with `-` would be
        // read as a flag.
        "--",
        launcher,
        SHIM_ARG,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// The `--mcp-config` value: dreamd's own shim, for the session dreamd spawns.
///
/// The key **must** be [`SERVER_NAME`]: Claude Code spells a server's tools
/// `mcp__<name>__<tool>`, and [`crate::agent::claude::GRANTS`] pre-grants six
/// of them by that spelling, so a different key here would give the reader a
/// permission card for every dreamd call. A test pins the two together.
///
/// Passed **without** `--strict-mcp-config`, which would make this the *only*
/// MCP config the session sees and silently drop every other server the reader
/// has — inside dreamd's pane and nowhere else, which is the worst way to lose
/// them. Merging is the whole point: dreamd adds itself and leaves the rest.
pub fn config_json(launcher: &str) -> String {
    serde_json::json!({
        "mcpServers": {
            SERVER_NAME: { "command": launcher, "args": [SHIM_ARG] }
        }
    })
    .to_string()
}

/// The registration as a line a reader can type, for the strip to show.
///
/// **Display only.** dreamd executes no part of this — it is the sentence for
/// the surfaces dreamd does not launch, and the reader's own shell is what runs
/// it. Built from [`add_args`] rather than written out again, because the last
/// hand-written copy of this command drifted from the real one and cost a
/// duplicate registration (see the module docs).
pub fn add_command(launcher: &str) -> String {
    let mut out = String::from("claude");
    for arg in add_args(launcher) {
        out.push(' ');
        out.push_str(&shell_quoted(&arg));
    }
    out
}

/// Quote an argument for display, if it needs it.
///
/// Deliberately not [`crate::agent::gate_server`]'s refuse-on-quote rule: that
/// one guards a command dreamd is about to *run*, where a path it cannot quote
/// is a reason not to launch. This one is a sentence, and a reader whose home
/// directory has an apostrophe in it is better served by a correct line than by
/// no line — so `'` is spliced the usual way rather than refused.
fn shell_quoted(arg: &str) -> String {
    if !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:@+=".contains(c))
    {
        return arg.to_string();
    }
    format!("'{}'", arg.replace('\'', r"'\''"))
}

/// Which path to register dreamd itself under. See the module docs.
pub fn launcher() -> String {
    let exe = std::env::current_exe().ok();
    launcher_from(std::env::var("APPIMAGE").ok().as_deref(), exe.as_deref())
}

/// The rule, without the environment.
///
/// Falls back to the bare name — resolved against `PATH` at spawn time, which
/// is right for exactly the case that reaches it: a dreamd running from a
/// throwaway copy of itself, where the installed one is the thing an agent
/// should be talking to.
pub fn launcher_from(appimage: Option<&str>, exe: Option<&Path>) -> String {
    if let Some(p) = appimage.map(str::trim).filter(|p| !p.is_empty()) {
        return p.to_string();
    }
    match exe.filter(|p| !is_ephemeral(p)).and_then(Path::to_str) {
        Some(p) => p.to_string(),
        None => SERVER_NAME.to_string(),
    }
}

/// Whether a path is one that will not survive this run.
///
/// The two that exist in practice: an AppImage mount (`/tmp/.mount_dreamdXXXX`,
/// reached only when `$APPIMAGE` is somehow unset) and macOS app translocation,
/// which runs a quarantined `.app` from a read-only image under
/// `/private/var/folders`. Neither is a path worth writing into a file that
/// outlives the process.
fn is_ephemeral(path: &Path) -> bool {
    let s = path.to_string_lossy();
    s.starts_with("/tmp/") || s.contains("/AppTranslocation/")
}

/// Does Claude Code already know a server by this name, in any scope?
///
/// `claude mcp get` answers in an exit status, which is the whole reason it is
/// the question asked: a `false` here has to mean "absent", not "the wording
/// changed". It does also *probe* the server, and that probe is harmless — the
/// shim answers the handshake out of its own compiled-in schema and never
/// touches the socket, so this cannot move the client count.
///
/// **Three answers, not two.** `None` is a `claude` that could not be run at
/// all, which is a different sentence from one that ran and said no: telling a
/// reader Claude Code does not know about dreamd, when the reason is that
/// Claude Code is not installed, sends them to fix the wrong thing.
///
/// `cwd` is the repo root — irrelevant to a user-scope registration, and set
/// anyway so this asks the question from where the rest of the agent surface
/// runs rather than from wherever the window was launched.
pub fn registered(cwd: &Path) -> Option<bool> {
    Command::new(crate::agent::claude::resolve())
        .args(["mcp", "get", SERVER_NAME])
        .current_dir(cwd)
        .output()
        .ok()
        .map(|o| o.status.success())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn the_name_is_the_one_the_pre_granted_tools_are_spelled_with() {
        // Renaming the server without renaming the grants would leave an agent
        // that can reach dreamd and gets a permission card for every call to
        // it. Neither half can move alone.
        let prefix = format!("mcp__{SERVER_NAME}__");
        for grant in crate::agent::claude::GRANTS {
            assert!(
                grant == "Read" || grant.starts_with(&prefix),
                "{grant} is not a {SERVER_NAME} tool"
            );
        }
        // And the injected config is the other half of the same pin: the key
        // here *is* the `<name>` in that spelling, so a rename that missed this
        // line would grant tools no server provides.
        let cfg: serde_json::Value = serde_json::from_str(&config_json("/usr/bin/dreamd")).unwrap();
        let server = &cfg["mcpServers"][SERVER_NAME];
        assert_eq!(server["command"], "/usr/bin/dreamd");
        assert_eq!(server["args"], serde_json::json!([SHIM_ARG]));
        assert_eq!(
            cfg["mcpServers"].as_object().map(|m| m.len()),
            Some(1),
            "dreamd adds itself and nothing else"
        );
    }

    #[test]
    fn the_injected_config_is_json_and_not_a_command_line() {
        // The whole tenet-3 argument for this path: it crosses as one
        // `Command::arg` carrying a serde-built document, so a launcher full of
        // shell metacharacters is a string value and never a second command.
        let cfg: serde_json::Value =
            serde_json::from_str(&config_json("/tmp/a b; rm -rf /$(whoami)")).unwrap();
        assert_eq!(
            cfg["mcpServers"][SERVER_NAME]["command"],
            "/tmp/a b; rm -rf /$(whoami)"
        );
    }

    #[test]
    fn the_printed_command_is_the_argv_it_claims_to_be() {
        // The failure this exists for: a hand-written copy of this line drifted
        // from `add_args`, lost `--scope user`, and put a second registration on
        // the development machine. It is derived now, and this is the assertion
        // that keeps it derived.
        let cmd = add_command("/usr/bin/dreamd");
        let mut want = vec!["claude".to_string()];
        want.extend(add_args("/usr/bin/dreamd"));
        assert_eq!(cmd.split(' ').collect::<Vec<_>>(), want);
        assert!(cmd.contains("--scope user"), "{cmd}");
    }

    #[test]
    fn a_launcher_needing_quotes_gets_them() {
        // A `.app` lives under "Application Support"; pasting the unquoted form
        // would register the first word and silently drop the rest.
        let cmd = add_command("/Users/r/Library/Application Support/dreamd");
        assert!(
            cmd.ends_with("-- '/Users/r/Library/Application Support/dreamd' mcp"),
            "{cmd}"
        );
        // An apostrophe is spliced rather than refused — see `shell_quoted`.
        assert_eq!(
            shell_quoted("/home/o'brien/dreamd"),
            r"'/home/o'\''brien/dreamd'"
        );
        // And the common path stays bare, so the line reads like one a person
        // would have typed.
        assert_eq!(shell_quoted("/usr/bin/dreamd"), "/usr/bin/dreamd");
    }

    #[test]
    fn the_registration_names_the_shim_after_a_separator() {
        let args = add_args("/usr/bin/dreamd");
        assert_eq!(args[..3], ["mcp", "add", SERVER_NAME]);
        let sep = args.iter().position(|a| a == "--").expect("a separator");
        assert_eq!(args[sep + 1..], ["/usr/bin/dreamd", SHIM_ARG]);
        // The scope is the whole reason one click is enough for every repo.
        assert_eq!(args[sep - 2..sep], ["--scope", "user"]);
    }

    #[test]
    fn a_leading_dash_in_the_launcher_stays_an_argument() {
        // The separator's actual job. `-dreamd` is not a path anyone has, but
        // the guarantee is what lets this take a path from the environment.
        let args = add_args("-dreamd");
        let sep = args.iter().position(|a| a == "--").unwrap();
        assert_eq!(args[sep + 1], "-dreamd");
    }

    #[test]
    fn an_appimage_registers_the_bundle_and_not_its_mount() {
        let mount = PathBuf::from("/tmp/.mount_dreamdAbC123/usr/bin/dreamd");
        assert_eq!(
            launcher_from(Some("/home/r/Apps/dreamd_0.2.0.AppImage"), Some(&mount)),
            "/home/r/Apps/dreamd_0.2.0.AppImage"
        );
    }

    #[test]
    fn an_installed_binary_registers_its_own_path() {
        let exe = PathBuf::from("/usr/bin/dreamd");
        assert_eq!(launcher_from(None, Some(&exe)), "/usr/bin/dreamd");
        // An empty `$APPIMAGE` is not an AppImage.
        assert_eq!(launcher_from(Some(""), Some(&exe)), "/usr/bin/dreamd");
    }

    #[test]
    fn an_ephemeral_copy_registers_the_bare_name_instead() {
        for p in [
            "/tmp/.mount_dreamdAbC123/usr/bin/dreamd",
            "/private/var/folders/xx/AppTranslocation/8F1/d/dreamd.app/Contents/MacOS/dreamd",
        ] {
            assert_eq!(launcher_from(None, Some(Path::new(p))), SERVER_NAME);
        }
        assert_eq!(launcher_from(None, None), SERVER_NAME);
    }
}
