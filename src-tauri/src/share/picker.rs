//! The macOS share sheet — `NSSharingServicePicker`, raised over dreamd's own
//! window so the reader never leaves it.
//!
//! Why this is a module and not four lines in `main.rs`: `main.rs` is a
//! `[[bin]]` and cannot be imported, and every unsafe block dreamd owns should
//! sit where something can at least read it in isolation. The *decision* about
//! what may be shared is [`super::resolve`]'s and is tested; what is here is
//! only the handing over.
//!
//! Two constraints the shape follows from:
//!
//! * `showRelativeToRect:ofView:preferredEdge:` is main-thread-only, so every
//!   caller reaches this through `AppHandle::run_on_main_thread` and proves it
//!   with a [`MainThreadMarker`].
//! * The picker is retained by AppKit once shown, so nothing here has to keep
//!   it alive — but it must not be shown from a thread that then unwinds,
//!   which is the other half of the main-thread rule.
//!
//! No entitlement is involved. The picker is an AppKit menu, and the services
//! behind it (Mail, Messages, AirDrop) are XPC rather than Apple Events — the
//! same reason `trash` is pinned to `NsFileManager`. Adding an Apple-Events
//! caller here would re-open a question the packaging notes settled.

use objc2::rc::Retained;
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{NSSharingServicePicker, NSView, NSWindow};
use objc2_foundation::{NSArray, NSPoint, NSRect, NSRectEdge, NSSize, NSString, NSURL};
use std::ffi::c_void;
use std::path::Path;

/// Where the sheet should point, in CSS pixels from the top-left of the
/// webview — i.e. exactly what `getBoundingClientRect()` returns for the share
/// button.
///
/// AppKit measures from the bottom-left, so this is flipped on the way in. The
/// window is created with `FullSizeContentView`, which is what makes the
/// webview and the content view share an origin and keeps the conversion to a
/// single subtraction.
#[derive(Debug, Clone, Copy)]
pub struct Anchor {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Anchor {
    /// A rect for a window whose frontend sent nothing usable — the top-centre
    /// of the content view, which is where a sheet with no button to point at
    /// reads least like a mistake.
    fn fallback(view_size: NSSize) -> NSRect {
        NSRect::new(
            NSPoint::new(view_size.width / 2.0, view_size.height - 1.0),
            NSSize::new(1.0, 1.0),
        )
    }

    fn to_ns_rect(self, view_size: NSSize) -> NSRect {
        // `>` against NaN is false, which is the point: a NaN width from a
        // hidden button falls back rather than reaching AppKit.
        if !(self.width > 0.0 && self.height > 0.0) {
            return Self::fallback(view_size);
        }
        NSRect::new(
            NSPoint::new(self.x, view_size.height - (self.y + self.height)),
            NSSize::new(self.width, self.height),
        )
    }
}

/// Raise the share sheet over `ns_window` for `files`.
///
/// `ns_window` is what `WebviewWindow::ns_window()` returns. The pointer is
/// borrowed for the length of the call and never stored.
///
/// # Safety
///
/// `ns_window` must be a live `NSWindow`, which is the contract
/// `WebviewWindow::ns_window` already meets for as long as the window is open.
pub unsafe fn show(
    ns_window: *mut c_void,
    files: &[std::path::PathBuf],
    anchor: Anchor,
    mtm: MainThreadMarker,
) -> Result<(), String> {
    if files.is_empty() {
        return Err("nothing to share".into());
    }
    if ns_window.is_null() {
        return Err("no window to share from".into());
    }
    let _ = mtm; // the marker is the proof, not an argument to anything below.

    let window: &NSWindow = unsafe { &*(ns_window as *const NSWindow) };
    let view: Retained<NSView> = window
        .contentView()
        .ok_or_else(|| "window has no content view".to_string())?;

    let urls: Vec<Retained<NSURL>> = files.iter().map(|p| url_for(p)).collect::<Result<_, _>>()?;
    // `initWithItems:` is typed `NSArray<AnyObject>` because a share item may
    // be a URL, a string or an image. dreamd only ever sends file URLs, so the
    // cast is widening a concrete array to the one the selector declares.
    let items: Retained<NSArray> =
        unsafe { Retained::cast_unchecked(NSArray::from_retained_slice(&urls)) };

    let picker =
        unsafe { NSSharingServicePicker::initWithItems(NSSharingServicePicker::alloc(), &items) };
    let rect = anchor.to_ns_rect(view.bounds().size);
    // `MinY`: the sheet hangs *below* the button, which is where every other
    // popover in this window opens and the only edge that cannot collide with
    // the titlebar.
    picker.showRelativeToRect_ofView_preferredEdge(rect, &view, NSRectEdge::NSMinYEdge);
    Ok(())
}

/// A `file://` URL for a path already validated by [`super::resolve`].
///
/// `fileURLWithPath:` is the only construction used — never `URLWithString:`,
/// which would parse the path as a URL and quietly accept a `javascript:` or
/// `http:` string if one ever reached here.
fn url_for(path: &Path) -> Result<Retained<NSURL>, String> {
    let s = path
        .to_str()
        .ok_or_else(|| format!("path is not valid UTF-8: {}", path.display()))?;
    Ok(NSURL::fileURLWithPath(&NSString::from_str(s)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size() -> NSSize {
        NSSize::new(1000.0, 800.0)
    }

    #[test]
    fn an_anchor_is_flipped_into_appkit_coordinates() {
        // A 30x30 button 20px from the top-left of the webview: AppKit's y
        // counts from the bottom, so its origin is 800 - (20 + 30) = 750.
        let r = Anchor {
            x: 20.0,
            y: 20.0,
            width: 30.0,
            height: 30.0,
        }
        .to_ns_rect(size());
        assert_eq!(r.origin.x, 20.0);
        assert_eq!(r.origin.y, 750.0);
        assert_eq!(r.size.width, 30.0);
        assert_eq!(r.size.height, 30.0);
    }

    #[test]
    fn a_degenerate_anchor_falls_back_to_the_top_centre() {
        // A frontend that sent a hidden button's rect (all zeroes) must not
        // put the sheet in the bottom-left corner, which reads as a bug.
        for bad in [
            Anchor {
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 0.0,
            },
            Anchor {
                x: 10.0,
                y: 10.0,
                width: -5.0,
                height: 30.0,
            },
            Anchor {
                x: 10.0,
                y: 10.0,
                width: f64::NAN,
                height: 30.0,
            },
        ] {
            let r = bad.to_ns_rect(size());
            assert_eq!(r.origin.x, 500.0, "{bad:?}");
            assert_eq!(r.origin.y, 799.0, "{bad:?}");
        }
    }

    #[test]
    fn a_path_that_is_not_utf8_is_refused_rather_than_lossily_converted() {
        // A lossy conversion would hand the picker a URL naming a *different*
        // file, which is the one failure mode worse than refusing.
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let bad = std::path::PathBuf::from(std::ffi::OsStr::from_bytes(b"/tmp/\xff\xfe.md"));
            assert!(url_for(&bad).is_err());
        }
        assert!(url_for(Path::new("/tmp/fine.md")).is_ok());
    }
}
