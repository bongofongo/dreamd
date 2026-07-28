//! Correctness harness for the bundled palettes. Exits non-zero if any of them
//! is missing a variable, has an unparseable `--bg`, or names a syntect theme
//! this build does not carry.
//!
//! Both failure modes are silent at runtime: a bad `--bg` just skips the
//! pre-paint and a bad `--syntax-theme` quietly falls back, so neither shows up
//! as anything but "that theme looks a bit off".
//!
//! ```sh
//! cargo run --example theme_check
//! ```

use dreamd::theme::Scheme;
use dreamd::{markdown, theme};

const SCHEMES: [Scheme; 2] = [Scheme::Light, Scheme::Dark];

/// A palette in the shape everything shipped before families existed: one bare
/// `:root`, no mode blocks. Kept here so the compatibility guarantee — such a
/// file reads identically in both schemes — is asserted rather than assumed.
/// Every user palette on disk is still one of these.
const FLAT: &str = ":root { --bg: #123456; --syntax-theme: \"InspiredGitHub\"; }";

/// Every variable a palette must declare. The base stylesheet and the app
/// chrome consume these; a missing one falls through to a hardcoded fallback
/// that belongs to a different theme.
const REQUIRED: &[&str] = &[
    "--bg",
    "--sidebar-bg",
    "--btn-bg",
    "--hover",
    "--border",
    "--text",
    "--muted",
    "--link",
    "--accent",
    "--accent-dim",
    "--hl",
    "--hl-prior",
    "--stale",
    "--stale-bg",
    "--font-body",
    "--font-mono",
    "--font-size",
    "--line-height",
    "--content-width",
    "--ui-font-size",
    "--syntax-theme",
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
        for scheme in SCHEMES {
            let m = format!("{name} [{}]", label(scheme));
            for var in REQUIRED {
                // Through the parser the app uses, not a substring test: a bare
                // `css.contains("--bg:")` passes the light pass for a variable
                // only the dark block declares, which is the exact mistake this
                // harness exists to catch.
                if theme::custom_property(css, var, scheme).is_none() {
                    println!("FAIL  {m}: missing {var}");
                    failed += 1;
                }
            }
            if theme::background(css, scheme).is_none() {
                println!("FAIL  {m}: --bg is not a parseable hex colour");
                failed += 1;
            }
            match theme::syntax_theme(css, scheme) {
                Some(syntax) if !available.contains(&syntax) => {
                    println!("FAIL  {m}: unknown syntect theme {syntax:?}");
                    println!("        available: {}", available.join(", "));
                    failed += 1;
                }
                Some(_) => {}
                None => {
                    println!("FAIL  {m}: --syntax-theme did not parse");
                    failed += 1;
                }
            }
        }

        // A family whose light block was never written renders dark-in-light
        // and nothing else complains — the palette is "valid" in both passes
        // above because the shared block satisfies them.
        if !theme::has_mode_blocks(css) {
            println!("FAIL  {name}: declares no [data-mode] block");
            failed += 1;
        } else {
            if theme::background(css, Scheme::Light) == theme::background(css, Scheme::Dark) {
                println!("FAIL  {name}: --bg is the same in both modes");
                failed += 1;
            }
            // A copy-pasted --syntax-theme is what keeps code blocks dark under
            // a light theme, and it is invisible except as "that looks a bit
            // off".
            if theme::syntax_theme(css, Scheme::Light) == theme::syntax_theme(css, Scheme::Dark) {
                println!("FAIL  {name}: --syntax-theme is the same in both modes");
                failed += 1;
            }
            // D15, pinned: the same fade strength cannot be right for both
            // members of a family. A bright `--hl` on a near-black background is
            // far more present at a given percentage than the same hue on paper,
            // so a value copied across the two blocks is a value that was never
            // looked at in one of them.
            if theme::prior_fade(css, Scheme::Light) == theme::prior_fade(css, Scheme::Dark) {
                println!("FAIL  {name}: --hl-prior is the same in both modes");
                failed += 1;
            }
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
        REQUIRED.len(),
        theme::ALIASES.len(),
    );
    if failed > 0 {
        std::process::exit(1);
    }
}
