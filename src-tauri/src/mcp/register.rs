//! `claude mcp add dreamd`, run for the reader instead of typed by them.
//!
//! The pane's status strip has always been able to say *that* MCP is
//! unregistered and to print the command that fixes it. Printing a command is a
//! strange thing for a GUI to do when it already knows where `claude` is — the
//! agent it just launched came from there — so this is the same sentence with
//! the shell step removed.
//!
//! # Nothing is assembled
//!
//! Tenet 3, and the same shape as [`crate::agent::claude`]: the `claude` binary
//! is resolved once through a login shell and then spawned with one
//! `Command::arg` per argument. The only value here that is not a compiled-in
//! literal is the launcher path, and it never reaches a shell — Claude Code
//! stores an MCP stdio server as a command plus an argument vector and spawns
//! it the same way.
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
//! `--scope user`, deliberately. [`shim`](super::shim) derives the repo root
//! from its own cwd and the socket path from that root, so one registration
//! serves every repository the reader ever opens — where the default `local`
//! scope would put this button back on screen in the next repo, having taught
//! nobody anything.

use std::path::Path;
use std::process::Command;

/// The name the server is registered under.
///
/// Not free to change: Claude Code spells a server's tools
/// `mcp__<name>__<tool>`, and [`crate::agent::claude::GRANTS`] pre-grants five
/// of them by that spelling. A test below pins the two together.
pub const SERVER_NAME: &str = "dreamd";

/// The argument dreamd is invoked with to become the stdio shim.
const SHIM_ARG: &str = "mcp";

/// The full argv for `claude`, given the launcher to register.
///
/// Pure and separate from [`register`] so the shape of the registration can be
/// asserted without spawning anything.
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

/// What one press of the button turned out to mean.
#[derive(serde::Serialize, Debug, PartialEq)]
pub struct Registration {
    /// False when Claude Code already knew about dreamd and this call changed
    /// nothing. **Not a rare case**: the strip appears whenever no agent has
    /// yet *called* a dreamd tool, which includes every correctly-registered
    /// session up to its first call. A reader who presses the button then is
    /// asking a question ("is it registered?") whose answer happens to be yes,
    /// and telling them it failed would be a lie about the state of their
    /// machine.
    pub added: bool,
    /// The launcher path written, when this call is what wrote it. `None` for
    /// an existing registration, whose command is Claude Code's to report —
    /// `claude mcp get dreamd` prints it, and guessing would risk naming a path
    /// other than the one that will actually run.
    pub launcher: Option<String>,
}

/// Register dreamd with Claude Code.
///
/// `cwd` is the repo root: irrelevant to a user-scope write, and set anyway so
/// that this runs where the rest of the agent surface runs rather than wherever
/// the window happened to be launched from.
///
/// The already-registered case is settled by [`is_registered`] — an exit
/// status — rather than by reading the add's own refusal, which is prose in
/// another program's voice and would be a thing to keep in sync forever. Any
/// *other* failure is passed through verbatim, first line only, for the same
/// reason inverted: dreamd has nothing better to say about it.
pub fn register(cwd: &Path) -> Result<Registration, String> {
    let launcher = launcher();
    let out = Command::new(crate::agent::claude::resolve())
        .args(add_args(&launcher))
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("could not run claude: {e}"))?;
    if out.status.success() {
        return Ok(Registration {
            added: true,
            launcher: Some(launcher),
        });
    }
    // Only on the failure path, so the common press costs one process and not
    // two.
    if is_registered(cwd) {
        return Ok(Registration {
            added: false,
            launcher: None,
        });
    }
    Err(first_line(&out.stderr)
        .or_else(|| first_line(&out.stdout))
        .unwrap_or_else(|| format!("claude exited with {}", out.status)))
}

/// Does Claude Code already know a server by this name, in any scope?
///
/// `claude mcp get` answers in an exit status, which is the whole reason it is
/// the question asked: a `false` here has to mean "absent", not "the wording
/// changed". It does also *probe* the server, and that probe is harmless — the
/// shim answers the handshake out of its own compiled-in schema and never
/// touches the socket, so this cannot move the client count the strip is
/// reading.
fn is_registered(cwd: &Path) -> bool {
    Command::new(crate::agent::claude::resolve())
        .args(["mcp", "get", SERVER_NAME])
        .current_dir(cwd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// The first non-empty line of a child's output, as a sentence to show a reader.
fn first_line(bytes: &[u8]) -> Option<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
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

    #[test]
    fn the_outcome_crosses_the_wire_under_the_names_the_strip_reads() {
        // `registerMcp` in ui/app.js branches on `.added` and prints
        // `.launcher`. Renaming either field would silently give every reader
        // the already-registered sentence.
        let json = serde_json::to_value(Registration {
            added: true,
            launcher: Some("/usr/bin/dreamd".into()),
        })
        .unwrap();
        assert_eq!(json["added"], true);
        assert_eq!(json["launcher"], "/usr/bin/dreamd");
        let none = serde_json::to_value(Registration {
            added: false,
            launcher: None,
        })
        .unwrap();
        assert!(none["launcher"].is_null());
    }

    #[test]
    fn a_child_that_said_nothing_still_yields_a_sentence() {
        assert_eq!(first_line(b"\n\n  boom  \nmore\n").as_deref(), Some("boom"));
        assert_eq!(first_line(b"   \n\n"), None);
    }
}
