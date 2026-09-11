//! Ask the web process for a PDF of the page, and write it to a file.
//!
//! **This replaced an `NSPrintOperation` over the live `WKWebView`, which ran
//! away.** That path never stopped paginating: a single `SKILL.md` produced a
//! 2.1GB file across a five-minute freeze, and a `.hang` report and a live
//! `sample` both put the main thread in `runOperation` ->
//! `_renderCurrentPageForPrintOperation` -> `NSView canDraw` ->
//! `dyld_image_header_containing_address`. That is AppKit walking a *view
//! hierarchy* once per page rather than WebKit paginating its own content, so
//! the operation was drawing the wrong thing; sizing its view was tried and
//! changed nothing.
//!
//! `createPDFWithConfiguration:completionHandler:` cannot fail that way, and
//! the reason is structural rather than a fix to the old path: the work
//! happens in the **web content process**, the output is one snapshot bounded
//! by a content rect, and AppKit's view drawing is not involved at all. There
//! is no loop here to run away.
//!
//! Two consequences worth knowing before changing anything here.
//!
//! The completion handler is called on the **main thread**, so a caller that
//! blocks the main thread waiting for it deadlocks. That is why `share_pdf`
//! and `save_pdf` are `#[tauri::command(async)]`: a plain command body runs on
//! the main thread, which is exactly the thread this needs free.
//!
//! And the page shape is WebKit's, taken from the content rather than from a
//! paper size. What the document *looks like* is still the `#print-css` block
//! in `ui/index.html`, because the frontend stages the export into `#content`
//! before calling — see `withStagedExport` in `ui/app.js`.

use block2::RcBlock;
use objc2::runtime::AnyObject;
use objc2::{msg_send, sel, MainThreadMarker};
use objc2_foundation::{NSData, NSError};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use objc2_web_kit::{WKPDFConfiguration, WKWebView};
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

/// How long to wait for the web process before giving up.
///
/// Generous, because a long document is legitimately slow, and bounded because
/// a wait with no end is the failure this module exists to stop being. The
/// wait is on a worker thread, so overrunning it costs the export and never
/// the window.
const PDF_TIMEOUT: Duration = Duration::from_secs(120);

/// A size past which something has gone wrong rather than a reader having a
/// long document.
///
/// Insurance, not policy. The replaced path filled 3.3GB of the temp directory
/// across three attempts, and nothing in the code noticed — every layer happily
/// wrote whatever it was handed. This cannot recur through the same mechanism,
/// since there is no pagination loop left to run away; the cap is here so that
/// if it recurs through some *other* mechanism it ends with a sentence naming
/// the size instead of a full disk.
const MAX_PDF_BYTES: usize = 512 * 1024 * 1024;

/// The finished document, or why there isn't one.
pub type PdfResult = Result<Vec<u8>, String>;

/// Is the selector this module needs actually on `WKWebView`?
///
/// `createPDFWithConfiguration:completionHandler:` is macOS 11 and
/// `tauri.conf.json` still declares a `minimumSystemVersion` of 10.15. A
/// missing selector would be an `objc_msgSend` to nothing, so [`start`] asks
/// before it sends and a machine that old loses the PDF format with a message
/// rather than losing the window.
///
/// # Safety
///
/// `webview` must be null or a live Objective-C object.
pub unsafe fn supported(webview: *mut AnyObject) -> bool {
    if webview.is_null() {
        return false;
    }
    unsafe {
        msg_send![webview, respondsToSelector: sel!(createPDFWithConfiguration:completionHandler:)]
    }
}

/// Ask the web process for a PDF, and return at once.
///
/// **Main thread only, and it must not be the thread that waits.** The
/// completion handler is called on the main thread, so a caller that blocks it
/// waiting for the answer deadlocks against itself — which is why this is
/// split from [`collect`] rather than being one blocking call, and why
/// `share_pdf`/`save_pdf` are `#[tauri::command(async)]`.
///
/// Every failure is reported *through the channel* rather than returned, so
/// `collect` is the one place a caller has to look.
///
/// # Safety
///
/// `webview` must be a live `WKWebView` — what `PlatformWebview::inner()`
/// hands back for as long as the window is open.
pub unsafe fn start(
    webview: *mut AnyObject,
    mtm: MainThreadMarker,
    size: Option<(f64, f64)>,
    tx: mpsc::Sender<PdfResult>,
) {
    if !unsafe { supported(webview) } {
        let _ = tx.send(Err("exporting a PDF needs macOS 11 or newer".into()));
        return;
    }
    let view: &WKWebView = unsafe { &*(webview as *const WKWebView) };

    // `Fn`, not `FnOnce`: a block is callable more than once as far as this
    // type is concerned, and `Sender::send` takes `&self`, which is what lets
    // the channel live inside one. A second call would find the receiver gone
    // and be ignored, which is the right answer either way.
    let handler = RcBlock::new(move |data: *mut NSData, error: *mut NSError| {
        let out = unsafe {
            if !data.is_null() {
                Ok((*data).to_vec())
            } else if !error.is_null() {
                Err((*error).localizedDescription().to_string())
            } else {
                Err("the web process returned neither a PDF nor an error".into())
            }
        };
        let _ = tx.send(out);
    });

    // **The default rect is the visible view, not the document.** Left to it,
    // this produces a screenshot of the window — which is exactly what the
    // first version did. So the frontend measures the laid-out export and
    // sends its size, and the rect is set from that.
    //
    // The size is in CSS pixels in the view's coordinate space, which is what
    // `scrollWidth`/`scrollHeight` are, so it crosses unconverted.
    let config = unsafe { WKPDFConfiguration::new(mtm) };
    if let Some((w, h)) = size {
        if w > 0.0 && h > 0.0 && w.is_finite() && h.is_finite() {
            unsafe {
                config.setRect(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, h)));
            }
        }
    }
    unsafe { view.createPDFWithConfiguration_completionHandler(Some(&config), &handler) };
}

/// Wait for [`start`]'s answer and write it to `dest`.
///
/// Any thread but the main one.
pub fn collect(rx: mpsc::Receiver<PdfResult>, dest: &Path) -> Result<(), String> {
    let bytes = rx
        .recv_timeout(PDF_TIMEOUT)
        .map_err(|_| "the PDF export timed out".to_string())??;
    if bytes.is_empty() {
        return Err("the web process produced an empty PDF".into());
    }
    if bytes.len() > MAX_PDF_BYTES {
        return Err(format!(
            "refusing to write a {}MB PDF — something is wrong with the export",
            bytes.len() / (1024 * 1024)
        ));
    }
    std::fs::write(dest, &bytes).map_err(|e| format!("could not write the PDF: {e}"))?;
    Ok(())
}
