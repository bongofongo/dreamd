//! Correctness harness for the bundled palettes. Exits non-zero if any of them
//! fails `theme::contract::check` — a missing variable, an unparseable `--bg`,
//! a syntect theme this build does not carry, a value copied across both mode
//! blocks — or prints a warning nobody has acknowledged.
//!
//! Every failure mode is silent at runtime: a bad `--bg` just skips the
//! pre-paint and a bad `--syntax-theme` quietly falls back, so neither shows up
//! as anything but "that theme looks a bit off".
//!
//! The rules live in the library, where `dreamd theme check` runs the same
//! function over a user's file. What stays here is the bundled-only policy:
//! every family must carry both modes, every alias must resolve, and the two
//! compatibility guarantees for a flat pre-family file hold.
//!
//! ```sh
//! cargo run --example theme_check
//! ```

use dreamd::theme::contract::{self, Level};
use dreamd::theme::Scheme;
use dreamd::{markdown, theme};

const SCHEMES: [Scheme; 2] = [Scheme::Light, Scheme::Dark];

/// A palette in the shape everything shipped before families existed: one bare
/// `:root`, no mode blocks. Kept here so the compatibility guarantee — such a
/// file reads identically in both schemes — is asserted rather than assumed.
/// Every user palette on disk is still one of these.
const FLAT: &str = ":root { --bg: #123456; --syntax-theme: \"InspiredGitHub\"; }";

/// Warnings a bundled family carries on purpose. Anything else is a failure:
/// a shipped palette with a typo or an unlooked-at contrast is a bug, and a
/// warning nobody reads is one nobody fixes.
const ACKNOWLEDGED: &[(&str, &str)] = &[
    // Three published light palettes whose link blue is the upstream's own
    // and sits under AA on its own ground: Nord's frost (3.5:1, said in the
    // palette's header), Catppuccin Latte's blue (4.3:1) and Tokyo Night
    // Day's (3.1:1). Darkening any of them would make it not that theme.
    ("nord", "light: --link on --bg is"),
    ("catppuccin", "light: --link on --bg is"),
    ("tokyo-night", "light: --link on --bg is"),
];

fn label(scheme: Scheme) -> &'static str {
    match scheme {
        Scheme::Light => "light",
        Scheme::Dark => "dark",
    }
}

fn main() {
    let available = markdown::syntax_theme_names();
    let mut failed = 0;

    for (name, css) in theme::BUNDLED {
        // Through the parser the app uses, not a substring test: a bare
        // `css.contains("--bg:")` passes the light pass for a variable only
        // the dark block declares, which is the exact mistake this harness
        // exists to catch. `check` reads every variable per scheme.
        for f in contract::check(css, &available) {
            let acknowledged = f.level == Level::Warning
                && ACKNOWLEDGED
                    .iter()
                    .any(|(n, prefix)| *n == *name && f.message.starts_with(prefix));
            if acknowledged {
                println!("ok    {name}: {} (acknowledged)", f.message);
            } else {
                println!("FAIL  {name}: {}", f.message);
                failed += 1;
            }
        }

        // A family whose light block was never written renders dark-in-light
        // and nothing else complains — the palette is "valid" in both passes
        // above because the shared block satisfies them. `check` only warns,
        // because a user's flat file is legitimate; a bundled one is not.
        if !theme::has_mode_blocks(css) {
            println!("FAIL  {name}: declares no [data-mode] block");
            failed += 1;
        }

        // What the app actually injects, not just the palette on its own.
        if theme::css_for(name).is_none() {
            println!("FAIL  {name}: does not resolve through css_for");
            failed += 1;
        }
    }

    // Legacy names from before families. This is the only automated check that
    // an existing config.toml still boots.
    for (alias, family, scheme) in theme::ALIASES {
        match theme::palette(alias) {
            None => {
                println!("FAIL  alias {alias}: does not resolve");
                failed += 1;
            }
            Some(css) if theme::background(&css, *scheme).is_none() => {
                println!(
                    "FAIL  alias {alias}: no parseable --bg in {}",
                    label(*scheme)
                );
                failed += 1;
            }
            Some(_) => {}
        }
        if !theme::BUNDLED.iter().any(|(n, _)| n == family) {
            println!("FAIL  alias {alias}: names missing family {family}");
            failed += 1;
        }
    }

    // The compatibility guarantee, pinned: a palette with no mode blocks reads
    // the same either way.
    if theme::background(FLAT, Scheme::Light) != theme::background(FLAT, Scheme::Dark) {
        println!("FAIL  flat palette: --bg differs by mode");
        failed += 1;
    }

    // ...and it fades. A palette written before `--hl-prior` existed — which is
    // every user file on disk — must not show last week's highlights at full
    // strength, so the undeclared case resolves to a value rather than to
    // nothing. This is the one required variable whose absence is survivable,
    // which is exactly why it needs its own assertion.
    for scheme in SCHEMES {
        if theme::prior_fade(FLAT, scheme) != theme::PRIOR_FADE_FALLBACK {
            println!(
                "FAIL  flat palette [{}]: --hl-prior did not fall back",
                label(scheme)
            );
            failed += 1;
        }
    }

    println!(
        "theme_check: {} families x {} modes, {} required vars each, {} aliases, {failed} failed",
        theme::BUNDLED.len(),
        SCHEMES.len(),
        contract::VARS.iter().filter(|v| v.required).count(),
        theme::ALIASES.len(),
    );
    if failed > 0 {
        std::process::exit(1);
    }
}
