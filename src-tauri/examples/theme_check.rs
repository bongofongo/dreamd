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

use dreamd::{markdown, theme};

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

fn main() {
    let available = markdown::syntax_theme_names();
    let mut failed = 0;

    for (name, css) in theme::BUNDLED {
        for var in REQUIRED {
            // Declarations only — `var(--x)` uses don't count.
            if !css.contains(&format!("{var}:")) {
                println!("FAIL  {name}: missing {var}");
                failed += 1;
            }
        }
        if theme::background(css).is_none() {
            println!("FAIL  {name}: --bg is not a parseable hex colour");
            failed += 1;
        }
        match theme::syntax_theme(css) {
            Some(syntax) if !available.contains(&syntax) => {
                println!("FAIL  {name}: unknown syntect theme {syntax:?}");
                println!("        available: {}", available.join(", "));
                failed += 1;
            }
            Some(_) => {}
            None => {
                println!("FAIL  {name}: --syntax-theme did not parse");
                failed += 1;
            }
        }
        // What the app actually injects, not just the palette on its own.
        if theme::css_for(name).is_none() {
            println!("FAIL  {name}: does not resolve through css_for");
            failed += 1;
        }
    }

    println!(
        "theme_check: {} palettes, {} required vars each, {failed} failed",
        theme::BUNDLED.len(),
        REQUIRED.len()
    );
    if failed > 0 {
        std::process::exit(1);
    }
}
