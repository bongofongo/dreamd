// The selector surface `dreamd theme guide` prints is scanned out of the page
// here, at build time, so the binary carries a few kilobytes of names rather
// than a second copy of `ui/index.html`. `selectors.rs` is included by path
// because it is also `theme::selectors`, and one scanner is what keeps the
// guide's list and the test that checks it from disagreeing.
#[path = "src/theme/selectors.rs"]
#[allow(dead_code)]
mod selectors;

fn main() {
    println!("cargo:rerun-if-changed=../ui/index.html");
    println!("cargo:rerun-if-changed=../ui/theme.css");
    let html = std::fs::read_to_string("../ui/index.html").expect("ui/index.html");
    let base = std::fs::read_to_string("../ui/theme.css").expect("ui/theme.css");
    let css = format!("{}\n{base}", selectors::style_blocks(&html));
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    std::fs::write(out.join("selectors.txt"), selectors::render(&css)).expect("write selectors");
    tauri_build::build()
}
