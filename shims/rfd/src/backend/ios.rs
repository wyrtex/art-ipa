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
//! The desktop API is synchronous. On the main thread the dialog is presented and a nested run
//! loop spins until it is dismissed (winit queues the events that arrive meanwhile). Off the main
//! thread the dialog is dispatched to the main queue and the caller blocks on a channel.

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
// Run loop / dispatch plumbing
// ---------------------------------------------------------------------------------------------

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFRunLoopDefaultMode: *const c_void;
    fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, return_after_source_handled: u8) -> i32;
}

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

/// Presents a dialog with `present` (which calls its callback exactly once when the dialog is
/// done, or never if it could not be shown — `present` then returns `false`) and waits for it.
fn run_modal<T, P>(present: P, fallback: T) -> T
where
    T: Send + 'static,
    P: FnOnce(MainThreadMarker, Box<dyn FnOnce(T)>) -> bool + Send + 'static,
{
    if let Some(mtm) = MainThreadMarker::new() {
        let slot: Rc<RefCell<Option<T>>> = Rc::default();
        let done = slot.clone();
        if !present(mtm, Box::new(move |v| *done.borrow_mut() = Some(v))) {
            return fallback;
        }
        loop {
            if let Some(v) = slot.borrow_mut().take() {
                return v;
            }
            // SAFETY: plain CoreFoundation call on the main thread's run loop.
            unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.05, 1) };
        }
    } else {
        let (tx, rx) = mpsc::channel::<Option<T>>();
        run_on_main(Box::new(move || {
            // SAFETY: dispatched to the main queue.
            let mtm = unsafe { MainThreadMarker::new_unchecked() };
            let tx2 = tx.clone();
            if !present(mtm, Box::new(move |v| { let _ = tx2.send(Some(v)); })) {
                let _ = tx.send(None);
            }
        }));
        rx.recv().ok().flatten().unwrap_or(fallback)
    }
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
                Some(p) => vc = Some(p),
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
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let docs = home.join("Documents");
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
        if let Some(cb) = self.ivars().callback.borrow_mut().take() {
            cb(result);
        }
    }
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
            "heic" => "public.heic",
            "webp" => "org.webmproject.webp",
            "bmp" => "com.microsoft.bmp",
            "svg" => "public.svg-image",
            "pdf" => "com.adobe.pdf",
            "txt" => "public.plain-text",
            "csv" => "public.comma-separated-values-text",
            "html" | "htm" => "public.html",
            "json" => "public.json",
            "mp4" => "public.mpeg-4",
            "mov" => "com.apple.quicktime-movie",
            "mp3" => "public.mp3",
            "wav" => "com.microsoft.waveform-audio",
            "aif" | "aiff" => "public.aiff-audio",
            "m4a" => "com.apple.m4a-audio",
            "docx" => "org.openxmlformats.wordprocessingml.document",
            "xlsx" => "org.openxmlformats.spreadsheetml.sheet",
            "pptx" => "org.openxmlformats.presentationml.presentation",
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

fn pick(dialog: FileDialog, multiple: bool) -> Option<Vec<PathBuf>> {
    let types = content_types(&dialog);
    run_modal(
        move |mtm, done| {
            let Some(vc) = top_view_controller(mtm) else { return false };
            let Some(cls) = AnyClass::get(c"UIDocumentPickerViewController") else { return false };
            let types = NSArray::from_retained_slice(
                &types.iter().map(|s| NSString::from_str(s)).collect::<Vec<_>>(),
            );
            unsafe {
                let alloc: Allocated<AnyObject> = msg_send![cls, alloc];
                // UIDocumentPickerModeImport = 0: the picked files are copied into the sandbox.
                let picker: Option<Retained<AnyObject>> =
                    msg_send![alloc, initWithDocumentTypes: &*types, inMode: 0usize];
                let Some(picker) = picker else { return false };
                let delegate = PickerDelegate::new(mtm, Box::new(move |r| done(r)));
                let _: () = msg_send![&*picker, setAllowsMultipleSelection: multiple];
                let _: () = msg_send![&*picker, setDelegate: &*delegate];
                // The picker holds its delegate weakly; tie the delegate's lifetime to the picker.
                KEEP_ALIVE.with(|k| k.borrow_mut().push(Retained::into_super(delegate)));
                present(&vc, &picker);
            }
            true
        },
        None,
    )
}

thread_local! {
    static KEEP_ALIVE: RefCell<Vec<Retained<NSObject>>> = const { RefCell::new(Vec::new()) };
}

fn save_path(dialog: &FileDialog) -> PathBuf {
    let dir = dialog
        .starting_directory
        .clone()
        .filter(|d| d.starts_with(documents_dir()))
        .unwrap_or_else(documents_dir);
    let mut name = dialog.file_name.clone().unwrap_or_else(|| "Untitled".to_string());
    if std::path::Path::new(&name).extension().is_none() {
        if let Some(ext) = dialog.filters.first().and_then(|f| f.extensions.first()) {
            name = format!("{name}.{}", ext.trim_start_matches('.'));
        }
    }
    dir.join(name)
}

impl FilePickerDialogImpl for FileDialog {
    fn pick_file(self) -> Option<PathBuf> {
        pick(self, false).and_then(|v| v.into_iter().next())
    }
    fn pick_files(self) -> Option<Vec<PathBuf>> {
        pick(self, true)
    }
}

impl FileSaveDialogImpl for FileDialog {
    fn save_file(self) -> Option<PathBuf> {
        Some(save_path(&self))
    }
}

impl FolderPickerDialogImpl for FileDialog {
    fn pick_folder(self) -> Option<PathBuf> {
        Some(documents_dir())
    }
    fn pick_folders(self) -> Option<Vec<PathBuf>> {
        Some(vec![documents_dir()])
    }
}

// ---------------------------------------------------------------------------------------------
// Alerts
// ---------------------------------------------------------------------------------------------

fn buttons(b: &MessageButtons) -> Vec<(String, isize, MessageDialogResult)> {
    // UIAlertActionStyle: 0 default, 1 cancel.
    match b {
        MessageButtons::Ok => vec![("OK".into(), 0, MessageDialogResult::Ok)],
        MessageButtons::OkCancel => vec![
            ("Cancel".into(), 1, MessageDialogResult::Cancel),
            ("OK".into(), 0, MessageDialogResult::Ok),
        ],
        MessageButtons::YesNo => vec![
            ("No".into(), 0, MessageDialogResult::No),
            ("Yes".into(), 0, MessageDialogResult::Yes),
        ],
        MessageButtons::YesNoCancel => vec![
            ("Cancel".into(), 1, MessageDialogResult::Cancel),
            ("No".into(), 0, MessageDialogResult::No),
            ("Yes".into(), 0, MessageDialogResult::Yes),
        ],
        MessageButtons::OkCustom(ok) => vec![(ok.clone(), 0, MessageDialogResult::Custom(ok.clone()))],
        MessageButtons::OkCancelCustom(ok, cancel) => vec![
            (cancel.clone(), 1, MessageDialogResult::Custom(cancel.clone())),
            (ok.clone(), 0, MessageDialogResult::Custom(ok.clone())),
        ],
        MessageButtons::YesNoCancelCustom(yes, no, cancel) => vec![
            (cancel.clone(), 1, MessageDialogResult::Custom(cancel.clone())),
            (no.clone(), 0, MessageDialogResult::Custom(no.clone())),
            (yes.clone(), 0, MessageDialogResult::Custom(yes.clone())),
        ],
    }
}

fn alert(dialog: MessageDialog) -> MessageDialogResult {
    let title = match (dialog.title.is_empty(), dialog.level) {
        (false, _) => dialog.title.clone(),
        (true, MessageLevel::Info) => String::new(),
        (true, MessageLevel::Warning) => "Warning".into(),
        (true, MessageLevel::Error) => "Error".into(),
    };
    let description = dialog.description.clone();
    let buttons = buttons(&dialog.buttons);
    run_modal(
        move |mtm, done| {
            let Some(vc) = top_view_controller(mtm) else { return false };
            let Some(cls) = AnyClass::get(c"UIAlertController") else { return false };
            let Some(action_cls) = AnyClass::get(c"UIAlertAction") else { return false };
            let done: Rc<RefCell<Option<Box<dyn FnOnce(MessageDialogResult)>>>> =
                Rc::new(RefCell::new(Some(done)));
            unsafe {
                let title = NSString::from_str(&title);
                let message = NSString::from_str(&description);
                // UIAlertControllerStyleAlert = 1
                let alert: Option<Retained<AnyObject>> = msg_send![
                    cls,
                    alertControllerWithTitle: &*title,
                    message: &*message,
                    preferredStyle: 1isize
                ];
                let Some(alert) = alert else { return false };
                for (label, style, result) in buttons {
                    let done = done.clone();
                    let handler = RcBlock::new(move |_action: *mut AnyObject| {
                        if let Some(cb) = done.borrow_mut().take() {
                            cb(result.clone());
                        }
                    });
                    let label = NSString::from_str(&label);
                    let action: Option<Retained<AnyObject>> = msg_send![
                        action_cls,
                        actionWithTitle: &*label,
                        style: style,
                        handler: &*handler
                    ];
                    if let Some(action) = action {
                        let _: () = msg_send![&*alert, addAction: &*action];
                    }
                }
                present(&vc, &alert);
            }
            true
        },
        MessageDialogResult::Cancel,
    )
}

impl MessageDialogImpl for MessageDialog {
    fn show(self) -> MessageDialogResult {
        alert(self)
    }
}

// ---------------------------------------------------------------------------------------------
// Async wrappers
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

/// Runs the synchronous dialog: right away on the main thread (an executor blocking the main
/// thread could otherwise never see the dialog), or on a helper thread otherwise.
fn spawn_dialog<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> DialogFutureType<T> {
    if MainThreadMarker::new().is_some() {
        let v = f();
        return Box::pin(std::future::ready(v));
    }
    let shared = Arc::new(Mutex::new(Shared { value: None, waker: None }));
    let s = shared.clone();
    std::thread::spawn(move || {
        let v = f();
        let mut s = s.lock().unwrap_or_else(|e| e.into_inner());
        s.value = Some(v);
        if let Some(w) = s.waker.take() {
            w.wake();
        }
    });
    Box::pin(Pending(shared))
}

impl AsyncFilePickerDialogImpl for FileDialog {
    fn pick_file_async(self) -> DialogFutureType<Option<FileHandle>> {
        spawn_dialog(move || self.pick_file().map(FileHandle::wrap))
    }
    fn pick_files_async(self) -> DialogFutureType<Option<Vec<FileHandle>>> {
        spawn_dialog(move || self.pick_files().map(|v| v.into_iter().map(FileHandle::wrap).collect()))
    }
}

impl AsyncFolderPickerDialogImpl for FileDialog {
    fn pick_folder_async(self) -> DialogFutureType<Option<FileHandle>> {
        Box::pin(std::future::ready(self.pick_folder().map(FileHandle::wrap)))
    }
    fn pick_folders_async(self) -> DialogFutureType<Option<Vec<FileHandle>>> {
        Box::pin(std::future::ready(
            self.pick_folders().map(|v| v.into_iter().map(FileHandle::wrap).collect()),
        ))
    }
}

impl AsyncFileSaveDialogImpl for FileDialog {
    fn save_file_async(self) -> DialogFutureType<Option<FileHandle>> {
        Box::pin(std::future::ready(self.save_file().map(FileHandle::wrap)))
    }
}

impl AsyncMessageDialogImpl for MessageDialog {
    fn show_async(self) -> DialogFutureType<MessageDialogResult> {
        spawn_dialog(move || self.show())
    }
}
