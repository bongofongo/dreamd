//! Print the open document to a PDF file, with no dialog.
//!
//! The `print_document` command already opens the OS print dialog, whose
//! Save-as-PDF destination is the reader's own export path. This is the same
//! machinery pointed at a file dreamd chose instead, because the share sheet
//! needs an artifact on disk *before* it can offer to send one — a dialog the
//! reader has to fill in first is exactly the trip out of the app this whole
//! feature exists to remove.
//!
//! What the page looks like is entirely the `#print-css` block in
//! `ui/index.html`: it hides the chrome, unwraps the scroller, neutralises the
//! theme's colours for paper and forces `--zoom: 1`. So this prints the *live*
//! webview and needs no second render — the document on screen is already the
//! document on the page.
//!
//! The consequence, and the reason `share_pdf` takes no file list: printing
//! the live webview can only ever produce the document that is *in* it. A PDF
//! share is the open document, and the frontend narrows the selection step to
//! match rather than offering a choice this cannot honour.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{msg_send, sel};
use objc2_app_kit::{NSPrintInfo, NSPrintSaveJob};
use objc2_foundation::{NSNumber, NSString, NSURL};
use std::path::Path;

/// Is the selector this module needs actually on `WKWebView`?
///
/// `printOperationWithPrintInfo:` is macOS 11, and `tauri.conf.json` still
/// declares a `minimumSystemVersion` of 10.15. Tauri's own `WebviewWindow::print`
/// already calls it, so a 10.15 machine has this problem with or without the
/// share feature — but a missing selector here would be an `objc_msgSend` to
/// nothing, which is a crash rather than a refusal. So [`print_to_file`] asks
/// before it sends, and a machine that old loses the PDF format with a message
/// naming the reason rather than losing the window.
///
/// # Safety
///
/// `webview` must be null or a live Objective-C object.
pub unsafe fn supported(webview: *mut AnyObject) -> bool {
    if webview.is_null() {
        return false;
    }
    unsafe { msg_send![webview, respondsToSelector: sel!(printOperationWithPrintInfo:)] }
}

/// Print `webview` to `dest`, blocking until AppKit has written the file.
///
/// # Safety
///
/// `webview` must be a live `WKWebView` — what `PlatformWebview::inner()`
/// hands back for as long as the window is open.
pub unsafe fn print_to_file(webview: *mut AnyObject, dest: &Path) -> Result<(), String> {
    if !supported(webview) {
        return Err("printing to a file needs macOS 11 or newer".into());
    }
    let path = dest
        .to_str()
        .ok_or_else(|| format!("export path is not valid UTF-8: {}", dest.display()))?;

    // A *copy* of the shared print info: mutating the shared one would change
    // what the reader's next File > Print does, which is not this feature's to
    // touch.
    let info: Retained<NSPrintInfo> = unsafe {
        let shared = NSPrintInfo::sharedPrintInfo();
        msg_send![&*shared, copy]
    };

    unsafe {
        let dict = info.dictionary();
        let url = NSURL::fileURLWithPath(&NSString::from_str(path));
        // The two keys that turn a print operation into a save. They are set
        // on the info's dictionary rather than through setters because
        // `NSPrintJobSavingURL` has none.
        let _: () = msg_send![&*dict, setObject: &*url, forKey: &*NSString::from_str("NSPrintJobSavingURL")];
        info.setJobDisposition(NSPrintSaveJob);
        // Margins are the `@page` rule's job, not this module's: WebKit reads
        // `@page { margin: 16mm }` out of the print sheet and a margin set here
        // would silently outrank a decision made in CSS beside the rules it
        // has to agree with.
        let _: () = msg_send![&*dict, setObject: &*NSNumber::new_bool(false), forKey: &*NSString::from_str("NSPrintHeaderAndFooter")];

        let op: *mut AnyObject = msg_send![webview, printOperationWithPrintInfo: &*info];
        if op.is_null() {
            return Err("the webview refused to make a print operation".into());
        }
        let _: () = msg_send![op, setShowsPrintPanel: false];
        let _: () = msg_send![op, setShowsProgressPanel: false];
        let ok: bool = msg_send![op, runOperation];
        if !ok {
            return Err("the print operation failed".into());
        }
    }

    // `runOperation` reports that it *ran*, not that it wrote — a destination
    // the sandbox refuses produces a true and no file. The share sheet would
    // then be handed a URL to nothing, so the file is the assertion.
    if !dest.exists() {
        return Err("the print operation produced no file".into());
    }
    Ok(())
}
