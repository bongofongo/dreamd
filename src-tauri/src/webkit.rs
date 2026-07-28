//! The one environment variable WebKitGTK needs set before GTK initialises.
//!
//! WebKitGTK 2.46+ renders through a DMA-BUF path that allocates its buffers
//! with GBM. Against the NVIDIA proprietary driver that allocation fails
//! (`Failed to create GBM buffer of size WxH: Invalid argument`), and on a
//! Wayland session the malformed `wl_buffer` that follows is a *protocol*
//! error, not a recoverable one: the compositor drops the connection and GDK
//! aborts the process with
//!
//! ```text
//! Gdk-Message: Error 71 (Protocol error) dispatching to Wayland display.
//! ```
//!
//! before dreamd's window ever exists. There is nothing to catch — the failure
//! is inside GTK, one frame into startup. The only lever is `WEBKIT_DISABLE_DMABUF_RENDERER`,
//! which has to be in the environment before webkit reads it, so this runs at
//! the top of `main` ahead of every other startup step.
//!
//! Detection is a file probe rather than a `#[cfg]`: `/proc/driver/nvidia/version`
//! exists only where the proprietary module is loaded, so this compiles and
//! runs identically on macOS and on Mesa Linux, where it simply answers "no"
//! and the accelerated path is left alone. That is deliberate — see the
//! platform-surface note in CLAUDE.md. Nouveau does not create the file and
//! does not have the bug.
//!
//! **No CI job covers this.** Reproducing it needs the proprietary driver and a
//! compositor, and no hosted runner has either — not even `canary.yml`, which
//! runs on Arch but never launches the GUI. The tests below pin the *decision*
//! and would pass on any machine whether or not the workaround still works;
//! changing anything here means launching dreamd on an NVIDIA Wayland session
//! by hand.

use std::path::Path;

const VAR: &str = "WEBKIT_DISABLE_DMABUF_RENDERER";

/// Created by the NVIDIA proprietary kernel module, and by nothing else.
const NVIDIA_PROC: &str = "/proc/driver/nvidia/version";

/// Should this process disable the DMA-BUF renderer?
///
/// Split out from the `set_var` so it is testable — the decision is the part
/// worth asserting on, and a test cannot safely mutate the process environment.
/// An existing value always wins, including an explicit `0`: someone who set it
/// by hand is debugging exactly this, and silently overriding them would make
/// the accelerated path unreachable.
pub fn should_disable_dmabuf(nvidia_present: bool, already_set: bool) -> bool {
    nvidia_present && !already_set
}

/// Apply the workaround. Call once, first thing in `main`, before any thread
/// exists — `set_var` is only sound while the process is single-threaded.
pub fn prepare_env() {
    if should_disable_dmabuf(
        Path::new(NVIDIA_PROC).exists(),
        std::env::var_os(VAR).is_some(),
    ) {
        std::env::set_var(VAR, "1");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disables_only_on_nvidia() {
        assert!(should_disable_dmabuf(true, false));
        assert!(!should_disable_dmabuf(false, false));
    }

    #[test]
    fn an_existing_value_always_wins() {
        // Including `WEBKIT_DISABLE_DMABUF_RENDERER=0`, which is how someone
        // re-enables the accelerated path to test it on NVIDIA.
        assert!(!should_disable_dmabuf(true, true));
        assert!(!should_disable_dmabuf(false, true));
    }

    #[test]
    fn probes_the_proprietary_module_not_the_gpu() {
        // A vendor-ID or PCI-class probe would also match nouveau, which
        // allocates through Mesa's GBM and renders fine.
        assert!(NVIDIA_PROC.starts_with("/proc/driver/nvidia"));
    }
}
