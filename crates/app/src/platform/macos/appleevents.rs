//! macOS: the Apple events the Finder and the Dock send the running app. Opening an audio
//! file from the Finder, dropping files on the Dock icon or choosing the app in Open With
//! starts no second process with arguments: AppKit sends the running app (or the one it has
//! just launched) an open-documents event (`kAEOpenDocuments`) with the files' URLs. Clicking
//! the Dock icon while the window is hidden in the menu bar sends a reopen event
//! (`kAEReopenApplication`). winit passes neither on, so this module takes both from the
//! Apple event manager: the files go where a second launch's go, and both show the window
//! (`instance::hand_over`).
//!
//! The handlers are put in when AppKit is about to finish launching
//! (`NSApplicationWillFinishLaunchingNotification`), as Apple asks: AppKit installs its own
//! for these events in `finishLaunching`, and a handler set before that would be replaced;
//! the event for the files a launch was made with comes after it.

use std::path::PathBuf;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
use objc2::{ClassType, define_class, msg_send, sel};
use objc2_foundation::{NSAppleEventDescriptor, NSAppleEventManager, NSNotification, NSNotificationCenter, NSURL};

/// Four-character codes (`AEDataModel.h`, `AppleEvents.h`).
const fn code(c: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*c)
}
const CORE_EVENT_CLASS: u32 = code(b"aevt");
const OPEN_DOCUMENTS: u32 = code(b"odoc");
const REOPEN_APPLICATION: u32 = code(b"rapp");
const DIRECT_OBJECT: u32 = code(b"----");
const FILE_URL: u32 = code(b"furl");

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and this class has no Drop.
    #[unsafe(super = NSObject)]
    #[name = "MarqueePlayerAppleEvents"]
    struct Handler;

    unsafe impl NSObjectProtocol for Handler {}

    impl Handler {
        #[unsafe(method(handleEvent:withReplyEvent:))]
        fn handle_event(&self, event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
            handle(event);
        }
    }
);

/// What an event asks for: the files of an open-documents event (none for a reopen).
fn handle(event: &NSAppleEventDescriptor) {
    // SAFETY: plain getters on a valid descriptor; four-character codes are `UInt32`.
    let id: u32 = unsafe { msg_send![event, eventID] };
    let paths = if id == OPEN_DOCUMENTS { file_paths(event) } else { Vec::new() };
    log::info!("{} from the Finder or the Dock ({} file(s))", if id == OPEN_DOCUMENTS { "open documents" } else { "reopen" }, paths.len());
    crate::platform::unix::instance::hand_over(paths);
}

/// The files of an open-documents event: its direct object, a list of file URLs.
fn file_paths(event: &NSAppleEventDescriptor) -> Vec<PathBuf> {
    // SAFETY: as above.
    let list: Option<Retained<NSAppleEventDescriptor>> = unsafe { msg_send![event, paramDescriptorForKeyword: DIRECT_OBJECT] };
    let Some(list) = list else { return Vec::new() };
    let n = list.numberOfItems();
    // A list counts from 1; a single file may come as the descriptor itself (0 items).
    let items: Vec<Retained<NSAppleEventDescriptor>> = if n > 0 { (1..=n).filter_map(|i| list.descriptorAtIndex(i)).collect() } else { vec![list] };
    items
        .iter()
        .filter_map(|item| {
            // SAFETY: as above.
            let url: Option<Retained<NSAppleEventDescriptor>> = unsafe { msg_send![&**item, coerceToDescriptorType: FILE_URL] };
            let url: Retained<NSURL> = url?.fileURLValue()?;
            Some(PathBuf::from(url.path()?.to_string()))
        })
        .collect()
}

/// Wait for AppKit to be about to finish launching, then take the two events. Once, on
/// the main thread, before the event loop runs (`instance::acquire`).
pub fn install() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::AcqRel) {
        return;
    }
    let block = block2::RcBlock::new(|_: NonNull<NSNotification>| set_handlers());
    let center = NSNotificationCenter::defaultCenter();
    // SAFETY: the name is AppKit's constant; the block runs on the posting (main) thread, no
    // object or queue is given, and the observer it returns is kept for the whole run.
    let observer = unsafe {
        center.addObserverForName_object_queue_usingBlock(Some(objc2_app_kit::NSApplicationWillFinishLaunchingNotification), None, None, &block)
    };
    std::mem::forget(observer);
}

fn set_handlers() {
    // SAFETY: `new` on a plain NSObject subclass.
    let handler: Retained<Handler> = unsafe { msg_send![Handler::class(), new] };
    let manager = NSAppleEventManager::sharedAppleEventManager();
    let target: &AnyObject = &handler;
    for id in [OPEN_DOCUMENTS, REOPEN_APPLICATION] {
        // SAFETY: the selector is the handler's (`handleEvent:withReplyEvent:`, two
        // descriptors), and the handler lives for the whole run (forgotten below).
        let _: () = unsafe {
            msg_send![&*manager, setEventHandler: target, andSelector: sel!(handleEvent:withReplyEvent:), forEventClass: CORE_EVENT_CLASS, andEventID: id]
        };
    }
    std::mem::forget(handler);
    log::info!("Apple events: the Finder's open documents and the Dock's reopen are the app's");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_character_codes_read_as_apple_writes_them() {
        assert_eq!(CORE_EVENT_CLASS, 0x6165_7674);
        assert_eq!(OPEN_DOCUMENTS, 0x6F64_6F63);
        assert_eq!(DIRECT_OBJECT, 0x2D2D_2D2D);
    }
}
