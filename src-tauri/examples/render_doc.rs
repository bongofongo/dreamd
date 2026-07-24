//! Render a markdown file through dreamd's real pipeline and write the HTML.
//!
//!     cargo run --release --example render_doc -- in.md [out.html]
//!
//! Used by the Chromium harness: measuring frontend cost against hand-written
//! HTML would be meaningless, because the thing that makes dreamd's payloads
//! large is syntect emitting an inline `style=` attribute per token. The
//! harness has to receive exactly what the app receives.
//!
//! An example rather than a bin — it never becomes part of the shipped binary.

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(input) = args.next() else {
        eprintln!("usage: render_doc <in.md> [out.html]");
        std::process::exit(2);
    };
    let source = match std::fs::read_to_string(&input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot read {input}: {e}");
            std::process::exit(1);
        }
    };
    let html = dreamd::markdown::render(&source);
    match args.next() {
        Some(out) => {
            if let Err(e) = std::fs::write(&out, html) {
                eprintln!("cannot write {out}: {e}");
                std::process::exit(1);
            }
        }
        None => print!("{html}"),
    }
}
