//! Asynchronous local-path selection only. No storage, backend or activation work.
//! AppKit entry points are main-thread-only; callers poll while a panel is busy.
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DialogId(pub u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogKind {
    Open,
    Save,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogError {
    Busy,
    Closed,
    Unsupported,
    WrongThread,
    NoParent,
    QuitUnavailable,
    InvalidName,
    InvalidUrl,
    Aborted,
    IdExhausted,
}
impl std::fmt::Display for DialogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Busy => "A workspace file chooser is already pending",
            Self::Closed => "Workspace file chooser is closed",
            Self::Unsupported => "Native file selection is unavailable on this platform; enter a local workspace path instead",
            Self::WrongThread => "Native file selection must run on the main UI thread",
            Self::NoParent => "No application window is available for file selection; enter a local workspace path instead",
            Self::QuitUnavailable => "Quit handling could not be installed for file selection; enter a local workspace path instead",
            Self::InvalidName => "Suggested filename must contain 1–128 bytes without separators or control characters",
            Self::InvalidUrl => "The selected location is not a supported absolute local file path",
            Self::Aborted => "The operating system could not finish the file chooser",
            Self::IdExhausted => "Workspace file chooser IDs exhausted",
        })
    }
}
impl std::error::Error for DialogError {}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DialogCompletion {
    pub id: DialogId,
    pub kind: DialogKind,
    /// None means canceled; a selected path still requires validation and explicit file I/O.
    pub result: Result<Option<PathBuf>, DialogError>,
}
#[derive(Default)]
struct Mailbox {
    #[cfg(any(target_os = "macos", test))]
    next: u64,
    pending: Option<(DialogId, DialogKind)>,
    completion: Option<DialogCompletion>,
    exit_requested: bool,
    closed: bool,
}
impl Mailbox {
    #[cfg(any(target_os = "macos", test))]
    fn begin(&mut self, kind: DialogKind) -> Result<DialogId, DialogError> {
        if self.closed {
            return Err(DialogError::Closed);
        }
        if self.pending.is_some() {
            return Err(DialogError::Busy);
        }
        self.next = self.next.checked_add(1).ok_or(DialogError::IdExhausted)?;
        let id = DialogId(self.next);
        self.pending = Some((id, kind));
        Ok(id)
    }
    #[cfg(any(target_os = "macos", test))]
    fn begin_with_parent<T>(
        &mut self,
        kind: DialogKind,
        parent: Option<T>,
    ) -> Result<(DialogId, T), DialogError> {
        if self.closed {
            return Err(DialogError::Closed);
        }
        if self.pending.is_some() {
            return Err(DialogError::Busy);
        }
        // Resolve the native owner before reserving an ID or setting Busy.
        let parent = parent.ok_or(DialogError::NoParent)?;
        self.begin(kind).map(|id| (id, parent))
    }
    #[cfg(any(target_os = "macos", test))]
    fn complete(&mut self, id: DialogId, result: Result<Option<PathBuf>, DialogError>) {
        if self.closed || self.completion.is_some() {
            return;
        }
        if let Some((pending, kind)) = self.pending {
            if pending == id {
                self.completion = Some(DialogCompletion { id, kind, result });
            }
        }
    }
    fn poll(&mut self) -> Option<DialogCompletion> {
        let result = self.completion.take()?;
        self.pending = None;
        Some(result)
    }
    fn cancel(&mut self) -> bool {
        if let Some((id, kind)) = self.pending {
            // Explicit UI cancellation also discards a selection completed just
            // before polling. The late native callback must not restore it.
            self.completion = Some(DialogCompletion {
                id,
                kind,
                result: Ok(None),
            });
            true
        } else {
            false
        }
    }
    fn close(&mut self) {
        self.closed = true;
        self.pending = None;
        self.completion = None;
        self.exit_requested = false;
    }
    #[cfg(any(target_os = "macos", test))]
    fn request_exit(&mut self, id: DialogId) -> bool {
        if self.closed || self.pending.is_none_or(|(pending, _)| pending != id) {
            return false;
        }
        self.cancel();
        self.exit_requested = true;
        true
    }
}

#[cfg(any(target_os = "macos", test))]
fn quit_shortcut(command: bool, control: bool, option: bool, shift: bool, text: &str) -> bool {
    command && !control && !option && !shift && text.eq_ignore_ascii_case("q")
}

/// Each chooser owns its registrations; callbacks hold only a weak reference.
#[cfg(any(target_os = "macos", test))]
struct Registrations<M, Q> {
    monitor: Option<M>,
    menu: Option<Q>,
}
#[cfg(any(target_os = "macos", test))]
impl<M, Q> Default for Registrations<M, Q> {
    fn default() -> Self {
        Self {
            monitor: None,
            menu: None,
        }
    }
}
#[cfg(any(target_os = "macos", test))]
impl<M, Q> Registrations<M, Q> {
    fn clear(&mut self) {
        drop(self.monitor.take());
        drop(self.menu.take());
    }
}

/// One chooser owned by the main UI thread. Rc also prevents moving it to a worker.
/// Keep alive until callback/poll or explicit cancel; Drop closes any remaining panel.
#[derive(Default)]
pub struct Dialog {
    mailbox: Rc<RefCell<Mailbox>>,
    #[cfg(target_os = "macos")]
    panel: Option<native::Panel>,
}
impl Dialog {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn is_busy(&self) -> bool {
        self.mailbox.borrow().pending.is_some()
    }
    pub fn open(&mut self) -> Result<DialogId, DialogError> {
        self.begin(DialogKind::Open, None)
    }
    pub fn save(&mut self, suggested_name: &str) -> Result<DialogId, DialogError> {
        if suggested_name.is_empty()
            || suggested_name.len() > 128
            || suggested_name
                .chars()
                .any(|c| c.is_control() || c == '/' || c == '\\')
            || suggested_name == "."
            || suggested_name == ".."
        {
            return Err(DialogError::InvalidName);
        }
        self.begin(DialogKind::Save, Some(suggested_name))
    }
    fn begin(&mut self, kind: DialogKind, name: Option<&str>) -> Result<DialogId, DialogError> {
        #[cfg(target_os = "macos")]
        {
            let main = objc2_foundation::MainThreadMarker::new().ok_or(DialogError::WrongThread)?;
            // Creating a save/open panel can change keyWindow. Capture the
            // actual application window first, and never fall back to app-modal.
            let parent = native::parent_window(main);
            let (id, parent) = self.mailbox.borrow_mut().begin_with_parent(kind, parent)?;
            match native::Panel::show(main, &parent, kind, name, id, Rc::downgrade(&self.mailbox)) {
                Ok(panel) => {
                    self.panel = Some(panel);
                    Ok(id)
                }
                Err(error) => {
                    let mut mailbox = self.mailbox.borrow_mut();
                    mailbox.cancel();
                    mailbox.poll();
                    Err(error)
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (kind, name);
            if self.mailbox.borrow().closed {
                Err(DialogError::Closed)
            } else {
                Err(DialogError::Unsupported)
            }
        }
    }
    /// Nonblocking callback polling. Request periodic repaints while is_busy().
    pub fn poll(&mut self) -> Option<DialogCompletion> {
        let completion = self.mailbox.borrow_mut().poll()?;
        #[cfg(target_os = "macos")]
        if let Some(panel) = self.panel.take() {
            panel.dismiss();
        }
        Some(completion)
    }
    /// One bounded request for normal window close; never exits the process itself.
    pub fn take_exit_request(&mut self) -> bool {
        std::mem::take(&mut self.mailbox.borrow_mut().exit_requested)
    }
    /// Immediately closes a pending panel, producing a correlated canceled result.
    /// A callback arriving after cancellation cannot replace that result or affect a newer panel.
    pub fn cancel(&mut self) -> bool {
        let pending = self.mailbox.borrow().pending;
        if let Some((_id, _)) = pending {
            self.mailbox.borrow_mut().cancel();
            #[cfg(target_os = "macos")]
            if let Some(panel) = self.panel.take() {
                panel.cancel();
            }
            true
        } else {
            false
        }
    }
    pub fn close(&mut self) {
        // Invalidate before messaging AppKit: cancellation can synchronously invoke its callback.
        self.mailbox.borrow_mut().close();
        #[cfg(target_os = "macos")]
        if let Some(panel) = self.panel.take() {
            panel.cancel();
        }
    }
}
impl Drop for Dialog {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use block2::RcBlock;
    use objc2::rc::{Retained, Weak};
    use objc2::runtime::{AnyObject, Sel};
    use objc2::{declare_class, msg_send_id, mutability, sel, ClassType, DeclaredClass};
    use objc2_app_kit::{
        NSApplication, NSEvent, NSEventMask, NSEventModifierFlags, NSEventType, NSMenu, NSMenuItem,
        NSModalResponse, NSModalResponseCancel, NSModalResponseOK, NSOpenPanel, NSSavePanel,
        NSWindow,
    };
    use objc2_foundation::{MainThreadMarker, NSObject, NSObjectProtocol, NSString, NSURL};
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    use std::ptr::NonNull;
    use std::rc::Weak as RcWeak;

    struct EventMonitor(Retained<AnyObject>);
    impl Drop for EventMonitor {
        fn drop(&mut self) {
            // SAFETY: token came from addLocalMonitor on the main thread;
            // its single owning registration is removed exactly once.
            unsafe {
                NSEvent::removeMonitor(&self.0);
            }
        }
    }
    struct MenuGuard {
        item: Retained<NSMenuItem>,
        original_target: Option<Retained<AnyObject>>,
        original_action: Option<Sel>,
        original_enabled: bool,
    }
    fn diagnostics_enabled() -> bool {
        std::env::var("STUDIO_WORKSPACE_DIALOG_DIAGNOSTICS").is_ok_and(|value| value == "1")
    }
    fn log_menu_state(stage: &str, application: &NSApplication, item: &NSMenuItem) {
        if !diagnostics_enabled() {
            return;
        }
        // SAFETY: called only during main-thread installation. Log fixed predicates,
        // class/selector identities and flags, never titles, paths or event text.
        unsafe {
            let key = item.keyEquivalent();
            let user_key = item.userKeyEquivalent();
            let target = item.target();
            let action = item.action();
            eprintln!(
                "workspace-dialog-menu stage={} q_equivalent={} user_q_equivalent={} modifier_bits={} enabled={} hidden={} hidden_ancestor={} target_class={} action={} app_active={} key_window={} main_window={} modal_window={}",
                stage,
                key.to_string().eq_ignore_ascii_case("q"),
                user_key.to_string().eq_ignore_ascii_case("q"),
                item.keyEquivalentModifierMask().0,
                item.isEnabled(),
                item.isHidden(),
                item.isHiddenOrHasHiddenAncestor(),
                target.as_ref().map_or("none", |target| target.class().name()),
                action.as_ref().map_or("none", |action| action.name()),
                application.isActive(),
                application.keyWindow().is_some(),
                application.mainWindow().is_some(),
                application.modalWindow().is_some(),
            );
        }
    }
    impl MenuGuard {
        fn install(main: MainThreadMarker, target: &QuitTarget) -> Result<Self, DialogError> {
            let application = NSApplication::sharedApplication(main);
            // SAFETY: main-thread menu inspection and an exact target/action scope;
            // the winit delegate and all other menu items remain untouched.
            unsafe {
                let menu = application.mainMenu().ok_or(DialogError::QuitUnavailable)?;
                let item = find_quit(&menu, &mut 128, 0).ok_or(DialogError::QuitUnavailable)?;
                log_menu_state("before-install", &application, &item);
                let guard = Self {
                    original_target: item.target(),
                    original_action: item.action(),
                    original_enabled: item.isEnabled(),
                    item,
                };
                guard.item.setTarget(Some(target));
                // Preserve the standard system action identity. Remote file panels
                // can route a Quit key equivalent by its standard selector, while
                // direct menu clicks can dispatch arbitrary custom selectors.
                guard.item.setAction(Some(sel!(terminate:)));
                guard.item.setEnabled(true);
                log_menu_state("after-install", &application, &guard.item);
                Ok(guard)
            }
        }
    }
    impl Drop for MenuGuard {
        fn drop(&mut self) {
            // SAFETY: main-thread scoped menu owner restores exact prior state.
            unsafe {
                self.item.setTarget(self.original_target.as_deref());
                self.item.setAction(self.original_action);
                self.item.setEnabled(self.original_enabled);
            }
        }
    }
    fn find_quit(menu: &NSMenu, budget: &mut usize, depth: u8) -> Option<Retained<NSMenuItem>> {
        if depth > 4 {
            return None;
        }
        // SAFETY: retained main-thread menu; traversal has a shared finite budget.
        unsafe {
            for index in 0..menu.numberOfItems().clamp(0, 128) {
                if *budget == 0 {
                    break;
                }
                *budget -= 1;
                let Some(item) = menu.itemAtIndex(index) else {
                    continue;
                };
                if item.action() == Some(sel!(terminate:)) {
                    return Some(item);
                }
                if let Some(child) = item.submenu() {
                    if let Some(found) = find_quit(&child, budget, depth + 1) {
                        return Some(found);
                    }
                }
            }
        }
        None
    }
    type HookState = Registrations<EventMonitor, MenuGuard>;
    struct ExitRequest {
        id: DialogId,
        mailbox: RcWeak<RefCell<Mailbox>>,
        panel: Weak<NSSavePanel>,
        hooks: RcWeak<RefCell<HookState>>,
    }
    impl ExitRequest {
        fn new(
            id: DialogId,
            mailbox: &RcWeak<RefCell<Mailbox>>,
            panel: &NSSavePanel,
            hooks: &Rc<RefCell<HookState>>,
        ) -> Self {
            Self {
                id,
                mailbox: mailbox.clone(),
                panel: Weak::new(panel),
                hooks: Rc::downgrade(hooks),
            }
        }
        fn fire(&self) -> bool {
            let Some(mailbox) = self.mailbox.upgrade() else {
                return false;
            };
            // Release the borrow before cancel: AppKit can invoke completion synchronously.
            let accepted = mailbox.borrow_mut().request_exit(self.id);
            if !accepted {
                return false;
            }
            clear_hooks(&self.hooks);
            if let Some(panel) = self.panel.load() {
                // SAFETY: main-thread action/monitor, weak owner upgraded for this call.
                unsafe {
                    panel.cancel(None);
                }
                panel.orderOut(None);
            }
            true
        }
    }
    declare_class!(
        struct QuitTarget;
        // SAFETY: NSObject has no subclassing obligations; all access is UI-thread-only.
        unsafe impl ClassType for QuitTarget {
            type Super=NSObject;
            type Mutability=mutability::MainThreadOnly;
            const NAME:&'static str="ROSStudioWorkspaceQuitTarget";
        }
        impl DeclaredClass for QuitTarget {type Ivars=ExitRequest;}
        unsafe impl NSObjectProtocol for QuitTarget {}
        // SAFETY: selector uses the standard one-object action signature.
        unsafe impl QuitTarget {
            // AppKit requires modal menu targets to opt in, including keyboard
            // equivalents dispatched before the local NSEvent monitor. This
            // target exists only for its current chooser and requests normal
            // app cleanup; it never terminates the process directly.
            // https://developer.apple.com/documentation/appkit/nsmenuitem/target
            #[method(worksWhenModal)]
            fn works_when_modal(&self) -> bool { true }
            #[method(terminate:)]
            fn request_quit(&self,_sender:Option<&AnyObject>){self.ivars().fire();}
        }
    );
    impl QuitTarget {
        fn new(main: MainThreadMarker, request: ExitRequest) -> Retained<Self> {
            let this = main.alloc().set_ivars(request);
            // SAFETY: fully initialized ivars and ordinary NSObject initialization.
            unsafe { msg_send_id![super(this), init] }
        }
    }
    fn clear_hooks(hooks: &RcWeak<RefCell<HookState>>) {
        if let Some(hooks) = hooks.upgrade() {
            hooks.borrow_mut().clear();
        }
    }
    struct QuitHooks {
        state: Rc<RefCell<HookState>>,
        // Retain the original block/target while removal can occur inside their call.
        _handler: RcBlock<dyn Fn(NonNull<NSEvent>) -> *mut NSEvent>,
        _target: Retained<QuitTarget>,
    }
    impl QuitHooks {
        fn install(
            main: MainThreadMarker,
            id: DialogId,
            mailbox: &RcWeak<RefCell<Mailbox>>,
            panel: &NSSavePanel,
        ) -> Result<Self, DialogError> {
            let state = Rc::new(RefCell::new(HookState::default()));
            let target = QuitTarget::new(main, ExitRequest::new(id, mailbox, panel, &state));
            state.borrow_mut().menu = Some(MenuGuard::install(main, &target)?);
            let request = ExitRequest::new(id, mailbox, panel, &state);
            // Opt-in development evidence only: never record text, paths, or arbitrary key codes.
            // The cap also bounds stderr work while a remote AppKit chooser owns keyboard focus.
            let diagnostics = diagnostics_enabled();
            let diagnostic_count = std::cell::Cell::new(0_u8);
            if diagnostics {
                eprintln!("workspace-dialog-key installed cap=32");
            }
            let handler = RcBlock::new(move |event: NonNull<NSEvent>| {
                // SAFETY: NSEvent's local monitor supplies a valid event on the main thread.
                let event_ref = unsafe { event.as_ref() };
                let flags = unsafe { event_ref.modifierFlags() };
                let kind = unsafe { event_ref.r#type() };
                let key_down = kind == NSEventType::KeyDown;
                let key_up = kind == NSEventType::KeyUp;
                // AppKit only permits charactersIgnoringModifiers for key events.
                let text = (key_down || key_up)
                    .then(|| unsafe { event_ref.charactersIgnoringModifiers() })
                    .flatten();
                let quit = text
                    .as_ref()
                    .filter(|text| text.len() <= 4)
                    .is_some_and(|text| {
                        quit_shortcut(
                            flags.contains(NSEventModifierFlags::NSEventModifierFlagCommand),
                            flags.contains(NSEventModifierFlags::NSEventModifierFlagControl),
                            flags.contains(NSEventModifierFlags::NSEventModifierFlagOption),
                            flags.contains(NSEventModifierFlags::NSEventModifierFlagShift),
                            &text.to_string(),
                        )
                    });
                if diagnostics && diagnostic_count.get() < 32 {
                    diagnostic_count.set(diagnostic_count.get() + 1);
                    eprintln!(
                        "workspace-dialog-key down={} up={} flags={} command={} control={} option={} shift={} q_keycode={} quit={}",
                        key_down,
                        key_up,
                        kind == NSEventType::FlagsChanged,
                        flags.contains(NSEventModifierFlags::NSEventModifierFlagCommand),
                        flags.contains(NSEventModifierFlags::NSEventModifierFlagControl),
                        flags.contains(NSEventModifierFlags::NSEventModifierFlagOption),
                        flags.contains(NSEventModifierFlags::NSEventModifierFlagShift),
                        unsafe { event_ref.keyCode() } == 12,
                        quit,
                    );
                }
                if key_down && quit && request.fire() {
                    std::ptr::null_mut()
                } else {
                    event.as_ptr()
                }
            });
            // SAFETY: owned block is copied by AppKit; captures are weak UI-thread owners.
            let token = unsafe {
                NSEvent::addLocalMonitorForEventsMatchingMask_handler(
                    if diagnostics {
                        NSEventMask::KeyDown | NSEventMask::KeyUp | NSEventMask::FlagsChanged
                    } else {
                        NSEventMask::KeyDown
                    },
                    &handler,
                )
            }
            .ok_or(DialogError::QuitUnavailable)?;
            state.borrow_mut().monitor = Some(EventMonitor(token));
            Ok(Self {
                state,
                _handler: handler,
                _target: target,
            })
        }
        fn clear(&self) {
            self.state.borrow_mut().clear();
        }
    }
    impl Drop for QuitHooks {
        fn drop(&mut self) {
            self.clear();
        }
    }
    pub(super) struct Panel {
        panel: Retained<NSSavePanel>,
        hooks: QuitHooks,
    }
    pub(super) fn parent_window(main: MainThreadMarker) -> Option<Retained<NSWindow>> {
        let application = NSApplication::sharedApplication(main);
        // SAFETY: MainThreadMarker proves AppKit is being queried on its UI thread;
        // the retained result outlives panel creation and the begin-sheet call.
        unsafe {
            application
                .mainWindow()
                .filter(|window| !window.isSheet())
                .or_else(|| application.keyWindow().filter(|window| !window.isSheet()))
        }
    }
    impl Panel {
        pub(super) fn show(
            main: MainThreadMarker,
            parent: &NSWindow,
            kind: DialogKind,
            name: Option<&str>,
            id: DialogId,
            mailbox: RcWeak<RefCell<Mailbox>>,
        ) -> Result<Self, DialogError> {
            // SAFETY: caller obtained a live MainThreadMarker on this thread. The retained
            // panel remains in Dialog until completion/cancel, and only this thread can own Dialog.
            let panel = unsafe {
                match kind {
                    DialogKind::Open => {
                        let panel = NSOpenPanel::openPanel(main);
                        panel.setCanChooseFiles(true);
                        panel.setCanChooseDirectories(false);
                        panel.setAllowsMultipleSelection(false);
                        Retained::into_super(panel)
                    }
                    DialogKind::Save => {
                        let panel = NSSavePanel::savePanel(main);
                        panel.setCanCreateDirectories(true);
                        panel.setNameFieldStringValue(&NSString::from_str(
                            name.unwrap_or("workspace.json"),
                        ));
                        panel
                    }
                }
            };
            let hooks = QuitHooks::install(main, id, &mailbox, &panel)?;
            let weak_hooks = Rc::downgrade(&hooks.state);
            let weak = Weak::new(&*panel);
            let callback = RcBlock::new(move |response: NSModalResponse| {
                clear_hooks(&weak_hooks);
                // AppKit invokes this completion on its main UI thread. Keep borrows short;
                // the block retains neither panel nor Dialog, avoiding a reference cycle.
                let Some(mailbox) = mailbox.upgrade() else {
                    return;
                };
                let relevant = {
                    let state = mailbox.borrow();
                    !state.closed
                        && state.pending.is_some_and(|(pending, _)| pending == id)
                        && state.completion.is_none()
                };
                if !relevant {
                    return;
                }
                let result = if response == NSModalResponseCancel {
                    Ok(None)
                } else if response == NSModalResponseOK {
                    weak.load()
                        .ok_or(DialogError::InvalidUrl)
                        .and_then(|panel| {
                            // SAFETY: retained main-thread panel, same callback thread.
                            unsafe { panel.URL() }
                                .ok_or(DialogError::InvalidUrl)
                                .and_then(|url| path(&url))
                                .map(Some)
                        })
                } else {
                    Err(DialogError::Aborted)
                };
                mailbox.borrow_mut().complete(id, result);
            });
            // SAFETY: retained main-thread parent was acquired before chooser creation.
            // AppKit copies the block and owns the asynchronous sheet association.
            // Use a document sheet; native Quit routing still requires an actual-window check.
            unsafe {
                panel.beginSheetModalForWindow_completionHandler(parent, &callback);
                if diagnostics_enabled() {
                    let attached = parent.attachedSheet();
                    let panel_window: &NSWindow = &panel;
                    eprintln!(
                        "workspace-dialog-sheet panel_key={} panel_visible={} parent_key={} parent_attached_sheet={} attached_is_panel={}",
                        panel.isKeyWindow(),
                        panel.isVisible(),
                        parent.isKeyWindow(),
                        attached.is_some(),
                        attached.as_deref().is_some_and(|window| std::ptr::eq(window, panel_window)),
                    );
                }
            }
            Ok(Self { panel, hooks })
        }
        pub(super) fn dismiss(self) {
            self.hooks.clear();
            self.panel.orderOut(None);
        }
        pub(super) fn cancel(self) {
            self.hooks.clear();
            // SAFETY: Dialog is !Send/!Sync and panel was created on the main thread.
            unsafe {
                self.panel.cancel(None);
            }
            self.panel.orderOut(None);
        }
    }
    fn path(url: &NSURL) -> Result<PathBuf, DialogError> {
        // SAFETY: NSURL retained for this call. A bounded buffer avoids lossy NSString
        // conversion and preserves native bytes (including Unicode). The URL owns no file I/O.
        unsafe {
            if !url.isFileURL()
                || url.isFileReferenceURL()
                || url.user().is_some()
                || url.password().is_some()
            {
                return Err(DialogError::InvalidUrl);
            }
            if url
                .host()
                .is_some_and(|host| !host.is_empty() && host.to_string() != "localhost")
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(DialogError::InvalidUrl);
            }
            let mut bytes = [0u8; 4097];
            let pointer = NonNull::new(bytes.as_mut_ptr().cast()).ok_or(DialogError::InvalidUrl)?;
            if !url.getFileSystemRepresentation_maxLength(pointer, bytes.len()) {
                return Err(DialogError::InvalidUrl);
            }
            let end = bytes
                .iter()
                .position(|byte| *byte == 0)
                .ok_or(DialogError::InvalidUrl)?;
            // Foundation can preserve an encoded NUL as a literal "%00" in its
            // filesystem representation. Reject that ambiguous URL; a real
            // filename containing "%00" is encoded "%2500" and remains valid.
            let absolute = url.absoluteString().ok_or(DialogError::InvalidUrl)?;
            if absolute.len() > 16384 || absolute.to_string().contains("%00") {
                return Err(DialogError::InvalidUrl);
            }
            let path = PathBuf::from(OsString::from_vec(bytes[..end].to_vec()));
            if end == 0 || !path.is_absolute() {
                return Err(DialogError::InvalidUrl);
            }
            Ok(path)
        }
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn scoped_quit_target_exposes_modal_keyboard_routing_contract() {
            // Runtime registration inspection needs no panel or main-thread
            // instance; native keyboard delivery remains a separate UI check.
            let class = QuitTarget::class();
            assert!(class.instance_method(sel!(worksWhenModal)).is_some());
            assert!(class.instance_method(sel!(terminate:)).is_some());
        }
        fn url(text: &str) -> Retained<NSURL> {
            // Foundation URL construction only: no panel, storage or network access.
            unsafe { NSURL::URLWithString(&NSString::from_str(text)).unwrap() }
        }
        #[test]
        fn local_unicode_paths_round_trip_without_lossy_string_conversion() {
            assert_eq!(
                path(&url("file:///tmp/caf%C3%A9%20%F0%9F%A4%96.json")).unwrap(),
                PathBuf::from("/tmp/cafe\u{301} 🤖.json")
            );
            assert_eq!(
                path(&url("file:///tmp/percent%2520name.json")).unwrap(),
                PathBuf::from("/tmp/percent%20name.json")
            );
        }
        #[test]
        fn nonlocal_nonfile_fragment_and_oversized_urls_are_rejected() {
            for text in [
                "https://example.invalid/workspace.json",
                "file://remote.invalid/tmp/workspace.json",
                "file:///tmp/workspace.json?query=1",
                "file:///tmp/workspace.json#fragment",
                "file:///tmp/good%00hidden.json",
                "relative.json",
            ] {
                assert_eq!(path(&url(text)), Err(DialogError::InvalidUrl), "{text}");
            }
            assert_eq!(
                path(&url(&format!("file:///tmp/{}", "x".repeat(5000)))),
                Err(DialogError::InvalidUrl)
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_duplicate_and_canceled_callbacks_cannot_replace_the_current_result() {
        let mut state = Mailbox::default();
        let first = state.begin(DialogKind::Open).unwrap();
        assert_eq!(state.begin(DialogKind::Save), Err(DialogError::Busy));
        state.complete(first, Ok(None));
        state.complete(first, Ok(Some(PathBuf::from("/unexpected"))));
        assert_eq!(state.poll().unwrap().result, Ok(None));
        let second = state.begin(DialogKind::Save).unwrap();
        assert!(second.0 > first.0);
        state.complete(first, Err(DialogError::Aborted));
        assert!(state.poll().is_none());
        state.complete(second, Ok(Some(PathBuf::from("/tmp/café 🤖.json"))));
        let completion = state.poll().unwrap();
        assert_eq!(completion.id, second);
        assert_eq!(completion.kind, DialogKind::Save);
        assert_eq!(
            completion.result,
            Ok(Some(PathBuf::from("/tmp/café 🤖.json")))
        );
    }
    #[test]
    fn closing_invalidates_pending_and_late_callbacks() {
        let mut state = Mailbox::default();
        let id = state.begin(DialogKind::Open).unwrap();
        state.close();
        state.complete(id, Ok(Some(PathBuf::from("/stale"))));
        assert!(state.poll().is_none());
        assert_eq!(state.begin(DialogKind::Open), Err(DialogError::Closed));
    }
    #[test]
    fn malformed_save_names_do_not_open_native_panels() {
        let mut dialog = Dialog::new();
        for name in [
            "",
            ".",
            "..",
            "../bad",
            "bad/name",
            "bad\\name",
            "bad\nname",
        ] {
            assert_eq!(dialog.save(name), Err(DialogError::InvalidName));
            assert!(!dialog.is_busy());
        }
        assert_eq!(dialog.save(&"x".repeat(129)), Err(DialogError::InvalidName));
    }
    #[test]
    fn exhaustion_never_reuses_an_identity() {
        let mut state = Mailbox {
            next: u64::MAX,
            ..Default::default()
        };
        assert_eq!(state.begin(DialogKind::Open), Err(DialogError::IdExhausted));
        assert!(state.pending.is_none());
    }
    #[test]
    fn missing_parent_does_not_reserve_or_wedge_the_next_chooser() {
        let mut state = Mailbox::default();
        assert_eq!(
            state.begin_with_parent::<()>(DialogKind::Save, None),
            Err(DialogError::NoParent)
        );
        assert!(state.pending.is_none());
        assert!(state.completion.is_none());
        assert_eq!(state.next, 0);
        let (id, parent) = state
            .begin_with_parent(DialogKind::Save, Some("main window"))
            .unwrap();
        assert_eq!(id, DialogId(1));
        assert_eq!(parent, "main window");
        assert_eq!(
            state.begin_with_parent::<()>(DialogKind::Open, None),
            Err(DialogError::Busy)
        );
        assert!(state.cancel());
        state.poll().unwrap();
        let (new_id, _) = state.begin_with_parent(DialogKind::Open, Some(())).unwrap();
        state.complete(id, Ok(Some(PathBuf::from("/late-save"))));
        assert!(state.poll().is_none());
        state.complete(new_id, Ok(None));
        assert_eq!(state.poll().unwrap().id, new_id);
        state.close();
        assert_eq!(
            state.begin_with_parent::<()>(DialogKind::Open, None),
            Err(DialogError::Closed)
        );
    }
    #[test]
    fn quit_request_cancels_only_current_selection_and_is_consumed_once() {
        let mut dialog = Dialog::new();
        let id = dialog.mailbox.borrow_mut().begin(DialogKind::Save).unwrap();
        assert!(!dialog.mailbox.borrow_mut().request_exit(DialogId(id.0 + 1)));
        assert!(!dialog.take_exit_request());
        dialog
            .mailbox
            .borrow_mut()
            .complete(id, Ok(Some(PathBuf::from("/selection-before-quit"))));
        assert!(dialog.mailbox.borrow_mut().request_exit(id));
        assert!(dialog.take_exit_request());
        assert!(!dialog.take_exit_request());
        assert_eq!(dialog.poll().unwrap().result, Ok(None));
        let next = dialog.mailbox.borrow_mut().begin(DialogKind::Open).unwrap();
        assert!(!dialog.mailbox.borrow_mut().request_exit(id));
        assert!(!dialog.take_exit_request());
        dialog.mailbox.borrow_mut().complete(next, Ok(None));
        dialog.poll().unwrap();
        dialog.close();
        assert!(!dialog.mailbox.borrow_mut().request_exit(next));
    }
    #[test]
    fn quit_key_requires_command_q_without_alternate_command_shortcuts() {
        assert!(quit_shortcut(true, false, false, false, "q"));
        assert!(quit_shortcut(true, false, false, false, "Q"));
        for (command, control, option, shift, text) in [
            (false, false, false, false, "q"),
            (true, true, false, false, "q"),
            (true, false, true, false, "q"),
            (true, false, false, true, "q"),
            (true, false, false, false, "w"),
            (true, false, false, false, "qq"),
            (true, false, false, false, ""),
            (true, false, false, false, "ｑ"),
        ] {
            assert!(!quit_shortcut(command, control, option, shift, text));
        }
    }
    #[test]
    fn chooser_registrations_remove_restore_once_and_stale_scopes_cannot_touch_new_owners() {
        struct Tracked(&'static str, Rc<RefCell<Vec<&'static str>>>);
        impl Drop for Tracked {
            fn drop(&mut self) {
                self.1.borrow_mut().push(self.0);
            }
        }
        let events = Rc::new(RefCell::new(Vec::new()));
        let old = Rc::new(RefCell::new(Registrations {
            monitor: Some(Tracked("remove old monitor", events.clone())),
            menu: Some(Tracked("restore old menu", events.clone())),
        }));
        let stale = Rc::downgrade(&old);
        old.borrow_mut().clear();
        old.borrow_mut().clear();
        drop(old);
        assert_eq!(*events.borrow(), ["remove old monitor", "restore old menu"]);
        let new = Rc::new(RefCell::new(Registrations {
            monitor: Some(Tracked("remove new monitor", events.clone())),
            menu: Some(Tracked("restore new menu", events.clone())),
        }));
        assert!(stale.upgrade().is_none());
        assert_eq!(events.borrow().len(), 2);
        drop(new);
        assert_eq!(
            *events.borrow(),
            [
                "remove old monitor",
                "restore old menu",
                "remove new monitor",
                "restore new menu"
            ]
        );
    }
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    #[test]
    fn explicit_cancel_discards_a_selection_completed_before_poll() {
        let mut state = Mailbox::default();
        let id = state.begin(DialogKind::Open).unwrap();
        state.complete(id, Ok(Some(PathBuf::from("/selected-before-cancel"))));
        assert!(state.cancel());
        state.complete(id, Ok(Some(PathBuf::from("/late-after-cancel"))));
        assert_eq!(state.poll().unwrap().result, Ok(None));
        assert!(!state.cancel());
    }
}
