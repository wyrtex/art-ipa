//! iOS / iPadOS backend (added by art-ipa).
//!
//! * Open: `UIDocumentPickerViewController` in import mode, so the picked files are copied into
//!   the app's sandbox and the returned paths can be read without security-scoped bookmarks.
//! * Save: there is no "save as" panel on iOS that can hand back a path before the file exists,
//!   so the file goes to the app's `Documents` folder (visible in the Files app, because the
//!   bundle sets `UIFileSharingEnabled` and `LSSupportsOpeningDocumentsInPlace`).
//! * Folders: the app's `Documents` folder.
//! * Messages: `UIAlertController`.
//!
//! Nothing here ever blocks the main thread: winit's iOS event loop aborts the app when a nested
//! run loop re-enters it (which is what a blocking modal panel does on the desktop).
//!
//! * Async dialogs present the picker and resolve their future when it is dismissed.
//! * Sync dialogs called off the main thread present the picker on the main queue and wait.
//! * Sync file pickers called on the main thread (the UI thread) present the picker and return
//!   `None` at once; the chosen files reach the app on the next frame as files dropped on its
//!   window (`craft_ios::deliver_files`, drained by the patched `egui-winit`). Every Craft app
//!   opens or imports dropped files.
//! * Sync questions called on the main thread can't wait for an answer: an `Ok`-only message is
//!   shown and answered `Ok`; any other question gets its cautious answer (`Cancel` / `No`)
//!   without being shown.

use std::cell::RefCell;
use std::ffi::c_void;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use block2::RcBlock;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, NSObject};
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_foundation::{NSArray, NSString, NSURL};

use crate::backend::{
    AsyncFilePickerDialogImpl, AsyncFileSaveDialogImpl, AsyncFolderPickerDialogImpl,
    AsyncMessageDialogImpl, DialogFutureType, FilePickerDialogImpl, FileSaveDialogImpl,
    FolderPickerDialogImpl, MessageDialogImpl,
};
use crate::message_dialog::{MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
use crate::{FileDialog, FileHandle};

// ---------------------------------------------------------------------------------------------
// Main-queue plumbing
// ---------------------------------------------------------------------------------------------

extern "C" {
    static _dispatch_main_q: c_void;
    fn dispatch_async_f(queue: *const c_void, context: *mut c_void, work: extern "C" fn(*mut c_void));
}

fn run_on_main(f: Box<dyn FnOnce() + Send>) {
    extern "C" fn trampoline(ctx: *mut c_void) {
        // SAFETY: `ctx` is the box leaked below, consumed exactly once.
        let f = unsafe { Box::from_raw(ctx as *mut Box<dyn FnOnce() + Send>) };
        f();
    }
    let ctx = Box::into_raw(Box::new(f)) as *mut c_void;
    // SAFETY: the main queue is a process-wide static; `trampoline` frees `ctx`.
    unsafe { dispatch_async_f(&raw const _dispatch_main_q, ctx, trampoline) };
}

/// A dialog presenter: shows the dialog and calls its callback exactly once when it is done, or
/// returns `false` (and never calls it) when it could not be shown.
type Present<T> = Box<dyn FnOnce(MainThreadMarker, Box<dyn FnOnce(T)>) -> bool + Send>;

/// Shows a dialog; `done` gets the result, or `fallback` when nothing could be shown. Never
/// blocks. Off the main thread the dialog is dispatched to the main queue.
fn show<T: Send + 'static>(present: Present<T>, fallback: T, done: Box<dyn FnOnce(T) + Send>) {
    let go = move |mtm: MainThreadMarker| {
        let slot: Rc<RefCell<Option<Box<dyn FnOnce(T) + Send>>>> = Rc::new(RefCell::new(Some(done)));
        let cb = slot.clone();
        let shown = present(
            mtm,
            Box::new(move |v| {
                if let Some(done) = cb.borrow_mut().take() {
                    done(v)
                }
            }),
        );
        if !shown {
            if let Some(done) = slot.borrow_mut().take() {
                done(fallback)
            }
        }
    };
    match MainThreadMarker::new() {
        Some(mtm) => go(mtm),
        // SAFETY: dispatched to the main queue.
        None => run_on_main(Box::new(move || go(unsafe { MainThreadMarker::new_unchecked() }))),
    }
}

/// Off the main thread only: show the dialog and wait for it.
fn show_and_wait<T: Send + Clone + 'static>(present: Present<T>, fallback: T) -> T {
    let (tx, rx) = mpsc::channel::<T>();
    show(present, fallback.clone(), Box::new(move |v| {
        let _ = tx.send(v);
    }));
    rx.recv().unwrap_or(fallback)
}

/// The view controller new dialogs are presented from.
fn top_view_controller(_mtm: MainThreadMarker) -> Option<Retained<AnyObject>> {
    unsafe {
        let app_cls = AnyClass::get(c"UIApplication")?;
        let app: Option<Retained<AnyObject>> = msg_send![app_cls, sharedApplication];
        let app = app?;
        let mut window: Option<Retained<AnyObject>> = msg_send![&*app, keyWindow];
        if window.is_none() {
            let windows: Option<Retained<NSArray<AnyObject>>> = msg_send![&*app, windows];
            window = windows.and_then(|w| w.firstObject());
        }
        let window = window?;
        let mut vc: Option<Retained<AnyObject>> = msg_send![&*window, rootViewController];
        loop {
            let current = vc.clone()?;
            let presented: Option<Retained<AnyObject>> = msg_send![&*current, presentedViewController];
            match presented {
                Some(p) => {
                    // An earlier picker on its way out: present on what stays.
                    let leaving: bool = msg_send![&*p, isBeingDismissed];
                    if leaving {
                        return Some(current);
                    }
                    vc = Some(p)
                }
                None => return Some(current),
            }
        }
    }
}

fn present(vc: &AnyObject, controller: &AnyObject) {
    unsafe {
        let none: Option<&block2::DynBlock<dyn Fn()>> = None;
        let _: () = msg_send![vc, presentViewController: controller, animated: true, completion: none];
    }
}

fn documents_dir() -> PathBuf {
    let docs = craft_ios::documents_dir();
    let _ = std::fs::create_dir_all(&docs);
    docs
}

// ---------------------------------------------------------------------------------------------
// Document picker
// ---------------------------------------------------------------------------------------------

type PickCallback = RefCell<Option<Box<dyn FnOnce(Option<Vec<PathBuf>>)>>>;

pub struct PickerIvars {
    callback: PickCallback,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ArtIpaRfdDocumentPickerDelegate"]
    #[ivars = PickerIvars]
    struct PickerDelegate;

    impl PickerDelegate {
        #[unsafe(method(documentPicker:didPickDocumentsAtURLs:))]
        fn did_pick(&self, _picker: &AnyObject, urls: &NSArray<NSURL>) {
            let paths: Vec<PathBuf> = urls
                .iter()
                .filter_map(|u| u.path())
                .map(|p| PathBuf::from(p.to_string()))
                .collect();
            self.finish(if paths.is_empty() { None } else { Some(paths) });
        }

        #[unsafe(method(documentPickerWasCancelled:))]
        fn cancelled(&self, _picker: &AnyObject) {
            self.finish(None);
        }
    }
);

impl PickerDelegate {
    fn new(mtm: MainThreadMarker, callback: Box<dyn FnOnce(Option<Vec<PathBuf>>)>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(PickerIvars { callback: RefCell::new(Some(callback)) });
        unsafe { msg_send![super(this), init] }
    }

    fn finish(&self, result: Option<Vec<PathBuf>>) {
        let cb = self.ivars().callback.borrow_mut().take();
        if let Some(cb) = cb {
            cb(result);
        }
        // The picker is gone: let go of its delegate.
        let me = self as *const Self as *const NSObject;
        KEEP_ALIVE.with(|k| k.borrow_mut().retain(|d| !std::ptr::eq(&**d, me)));
    }
}

thread_local! {
    /// Delegates of the pickers on screen (a picker holds its delegate weakly).
    static KEEP_ALIVE: RefCell<Vec<Retained<NSObject>>> = const { RefCell::new(Vec::new()) };
}

/// Uniform type identifiers for the dialog's filters (`public.item` when there are none or an
/// extension is unknown, so nothing the app can read is greyed out).
fn content_types(dialog: &FileDialog) -> Vec<&'static str> {
    let mut ids: Vec<&'static str> = Vec::new();
    for ext in dialog.filters.iter().flat_map(|f| f.extensions.iter()) {
        let id = match ext.trim_start_matches('.').to_ascii_lowercase().as_str() {
            "png" => "public.png",
            "jpg" | "jpeg" => "public.jpeg",
            "gif" => "com.compuserve.gif",
            "tif" | "tiff" => "public.tiff",
            "heic" | "heif" => "public.heic",
            "webp" => "org.webmproject.webp",
            "bmp" => "com.microsoft.bmp",
            "svg" => "public.svg-image",
            "pdf" => "com.adobe.pdf",
            "txt" => "public.plain-text",
            "csv" => "public.comma-separated-values-text",
            "html" | "htm" => "public.html",
            "json" => "public.json",
            "mp4" | "m4v" => "public.mpeg-4",
            "mov" => "com.apple.quicktime-movie",
            "mp3" => "public.mp3",
            "wav" | "wave" => "com.microsoft.waveform-audio",
            "aif" | "aiff" | "aifc" => "public.aiff-audio",
            "m4a" => "com.apple.m4a-audio",
            "docx" => "org.openxmlformats.wordprocessingml.document",
            "xlsx" => "org.openxmlformats.spreadsheetml.sheet",
            "pptx" => "org.openxmlformats.presentationml.presentation",
            // Anything else (app formats, RAW files, fonts, `*`, …): don't restrict the picker.
            _ => {
                ids.clear();
                break;
            }
        };
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    if ids.is_empty() {
        ids.push("public.item");
    }
    ids
}

/// A presenter for a document picker; its callback gets the chosen files (`None`: cancelled).
fn picker(types: Vec<&'static str>, multiple: bool) -> Present<Option<Vec<PathBuf>>> {
    Box::new(move |mtm, done| {
        let Some(vc) = top_view_controller(mtm) else { return false };
        let Some(cls) = AnyClass::get(c"UIDocumentPickerViewController") else { return false };
        let types = NSArray::from_retained_slice(&types.iter().map(|s| NSString::from_str(s)).collect::<Vec<_>>());
        unsafe {
            let alloc: Allocated<AnyObject> = msg_send![cls, alloc];
            // UIDocumentPickerModeImport = 0: the picked files are copied into the sandbox.
            let picker: Option<Retained<AnyObject>> = msg_send![alloc, initWithDocumentTypes: &*types, inMode: 0usize];
            let Some(picker) = picker else { return false };
            let delegate = PickerDelegate::new(mtm, done);
            let _: () = msg_send![&*picker, setAllowsMultipleSelection: multiple];
            let _: () = msg_send![&*picker, setDelegate: &*delegate];
            KEEP_ALIVE.with(|k| k.borrow_mut().push(Retained::into_super(delegate)));
            present(&vc, &picker);
        }
        true
    })
}

fn save_path(dialog: &FileDialog) -> PathBuf {
    let docs = documents_dir();
    let dir = dialog
        .starting_directory
        .clone()
        .filter(|d| d.starts_with(&docs) && d.is_dir())
        .unwrap_or(docs);
    let mut name = dialog
        .file_name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| "Untitled".to_string());
    if std::path::Path::new(&name).extension().is_none() {
        if let Some(ext) = dialog.filters.first().and_then(|f| f.extensions.first()).filter(|e| e.as_str() != "*") {
            name = format!("{name}.{}", ext.trim_start_matches('.'));
        }
    }
    dir.join(name)
}

/// Sync pickers. On the UI thread the files arrive later, as dropped files (see the module docs).
fn pick_sync(dialog: FileDialog, multiple: bool) -> Option<Vec<PathBuf>> {
    let present = picker(content_types(&dialog), multiple);
    if MainThreadMarker::new().is_some() {
        show(present, None, Box::new(|picked: Option<Vec<PathBuf>>| craft_ios::deliver_files(picked.unwrap_or_default())));
        None
    } else {
        show_and_wait(present, None)
    }
}

fn folder(dialog: &FileDialog) -> PathBuf {
    let docs = documents_dir();
    dialog.starting_directory.clone().filter(|d| d.starts_with(&docs) && d.is_dir()).unwrap_or(docs)
}

impl FilePickerDialogImpl for FileDialog {
    fn pick_file(self) -> Option<PathBuf> {
        pick_sync(self, false).and_then(|v| v.into_iter().next())
    }
    fn pick_files(self) -> Option<Vec<PathBuf>> {
        pick_sync(self, true)
    }
}

impl FileSaveDialogImpl for FileDialog {
    fn save_file(self) -> Option<PathBuf> {
        Some(save_path(&self))
    }
}

impl FolderPickerDialogImpl for FileDialog {
    fn pick_folder(self) -> Option<PathBuf> {
        Some(folder(&self))
    }
    fn pick_folders(self) -> Option<Vec<PathBuf>> {
        Some(vec![folder(&self)])
    }
}

// ---------------------------------------------------------------------------------------------
// Alerts
// ---------------------------------------------------------------------------------------------

/// The buttons, as (label, UIAlertActionStyle, result); the last one is the cautious answer.
fn buttons(b: &MessageButtons) -> Vec<(String, isize, MessageDialogResult)> {
    // UIAlertActionStyle: 0 default, 1 cancel.
    match b {
        MessageButtons::Ok => vec![("OK".into(), 0, MessageDialogResult::Ok)],
        MessageButtons::OkCancel => vec![
            ("OK".into(), 0, MessageDialogResult::Ok),
            ("Cancel".into(), 1, MessageDialogResult::Cancel),
        ],
        MessageButtons::YesNo => vec![
            ("Yes".into(), 0, MessageDialogResult::Yes),
            ("No".into(), 1, MessageDialogResult::No),
        ],
        MessageButtons::YesNoCancel => vec![
            ("Yes".into(), 0, MessageDialogResult::Yes),
            ("No".into(), 0, MessageDialogResult::No),
            ("Cancel".into(), 1, MessageDialogResult::Cancel),
        ],
        MessageButtons::OkCustom(ok) => vec![(ok.clone(), 0, MessageDialogResult::Custom(ok.clone()))],
        MessageButtons::OkCancelCustom(ok, cancel) => vec![
            (ok.clone(), 0, MessageDialogResult::Custom(ok.clone())),
            (cancel.clone(), 1, MessageDialogResult::Custom(cancel.clone())),
        ],
        MessageButtons::YesNoCancelCustom(yes, no, cancel) => vec![
            (yes.clone(), 0, MessageDialogResult::Custom(yes.clone())),
            (no.clone(), 0, MessageDialogResult::Custom(no.clone())),
            (cancel.clone(), 1, MessageDialogResult::Custom(cancel.clone())),
        ],
    }
}

fn cautious_answer(b: &MessageButtons) -> MessageDialogResult {
    buttons(b).pop().map(|b| b.2).unwrap_or(MessageDialogResult::Cancel)
}

fn alert(dialog: &MessageDialog) -> Present<MessageDialogResult> {
    let title = match (dialog.title.is_empty(), dialog.level) {
        (false, _) => dialog.title.clone(),
        (true, MessageLevel::Info) => String::new(),
        (true, MessageLevel::Warning) => "Warning".into(),
        (true, MessageLevel::Error) => "Error".into(),
    };
    let description = dialog.description.clone();
    let buttons = buttons(&dialog.buttons);
    Box::new(move |mtm, done| {
        let Some(vc) = top_view_controller(mtm) else { return false };
        let Some(cls) = AnyClass::get(c"UIAlertController") else { return false };
        let Some(action_cls) = AnyClass::get(c"UIAlertAction") else { return false };
        let done: Rc<RefCell<Option<Box<dyn FnOnce(MessageDialogResult)>>>> = Rc::new(RefCell::new(Some(done)));
        unsafe {
            let title = NSString::from_str(&title);
            let message = NSString::from_str(&description);
            // UIAlertControllerStyleAlert = 1
            let alert: Option<Retained<AnyObject>> =
                msg_send![cls, alertControllerWithTitle: &*title, message: &*message, preferredStyle: 1isize];
            let Some(alert) = alert else { return false };
            for (label, style, result) in buttons {
                let done = done.clone();
                let handler = RcBlock::new(move |_action: *mut AnyObject| {
                    if let Some(cb) = done.borrow_mut().take() {
                        cb(result.clone());
                    }
                });
                let label = NSString::from_str(&label);
                let action: Option<Retained<AnyObject>> =
                    msg_send![action_cls, actionWithTitle: &*label, style: style, handler: &*handler];
                if let Some(action) = action {
                    let _: () = msg_send![&*alert, addAction: &*action];
                }
            }
            present(&vc, &alert);
        }
        true
    })
}

impl MessageDialogImpl for MessageDialog {
    fn show(self) -> MessageDialogResult {
        let cautious = cautious_answer(&self.buttons);
        if MainThreadMarker::new().is_none() {
            return show_and_wait(alert(&self), cautious);
        }
        // On the UI thread nothing can wait for the answer.
        if buttons(&self.buttons).len() == 1 {
            show(alert(&self), cautious.clone(), Box::new(|_| {}));
        } else {
            log::warn!("rfd (iOS): {:?} answered {cautious} without asking: the UI thread can't wait", self.description);
        }
        cautious
    }
}

// ---------------------------------------------------------------------------------------------
// Async dialogs
// ---------------------------------------------------------------------------------------------

struct Shared<T> {
    value: Option<T>,
    waker: Option<Waker>,
}

struct Pending<T>(Arc<Mutex<Shared<T>>>);

impl<T> Future for Pending<T> {
    type Output = T;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match s.value.take() {
            Some(v) => Poll::Ready(v),
            None => {
                s.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

/// Shows the dialog now (as the macOS backend does when the future is created) and resolves the
/// future when it is dismissed.
fn show_async<T: Send + 'static>(present: Present<T>, fallback: T) -> DialogFutureType<T> {
    let shared = Arc::new(Mutex::new(Shared { value: None, waker: None }));
    let s = shared.clone();
    show(
        present,
        fallback,
        Box::new(move |v| {
            let mut s = s.lock().unwrap_or_else(|e| e.into_inner());
            s.value = Some(v);
            if let Some(w) = s.waker.take() {
                w.wake();
            }
        }),
    );
    Box::pin(Pending(shared))
}

impl AsyncFilePickerDialogImpl for FileDialog {
    fn pick_file_async(self) -> DialogFutureType<Option<FileHandle>> {
        let fut = show_async(picker(content_types(&self), false), None);
        Box::pin(async move { fut.await.and_then(|v| v.into_iter().next()).map(FileHandle::wrap) })
    }
    fn pick_files_async(self) -> DialogFutureType<Option<Vec<FileHandle>>> {
        let fut = show_async(picker(content_types(&self), true), None);
        Box::pin(async move { fut.await.map(|v| v.into_iter().map(FileHandle::wrap).collect()) })
    }
}

impl AsyncFolderPickerDialogImpl for FileDialog {
    fn pick_folder_async(self) -> DialogFutureType<Option<FileHandle>> {
        Box::pin(std::future::ready(Some(FileHandle::wrap(folder(&self)))))
    }
    fn pick_folders_async(self) -> DialogFutureType<Option<Vec<FileHandle>>> {
        Box::pin(std::future::ready(Some(vec![FileHandle::wrap(folder(&self))])))
    }
}

impl AsyncFileSaveDialogImpl for FileDialog {
    fn save_file_async(self) -> DialogFutureType<Option<FileHandle>> {
        Box::pin(std::future::ready(Some(FileHandle::wrap(save_path(&self)))))
    }
}

impl AsyncMessageDialogImpl for MessageDialog {
    fn show_async(self) -> DialogFutureType<MessageDialogResult> {
        let cautious = cautious_answer(&self.buttons);
        show_async(alert(&self), cautious)
    }
}
