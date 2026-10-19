//! The update toast: a system notification, so an update typeThai found on its own is visible without opening the tray.
//! A click is the tray item's action (`bridge::REQ_UPDATE_CLICK`): restart into the staged update, or open the release
//! page. The callers post one notification per release (`update::take_toast`).
//!
//! Nothing here is load-bearing. A copy that cannot post notifications — a build without a bundle, a permission the
//! user refused, a Windows build whose tile is not registered — still has the tray item, the bubble's chip and
//! Settings → Updates.
#[cfg(target_os = "macos")]
pub use macos::post;
#[cfg(windows)]
pub use windows::post;
/// The notifications already sitting in Notification Center (macOS; empty elsewhere), so `--demo-toast` can show what
/// the system actually took rather than what was sent.
#[cfg(target_os = "macos")]
pub use macos::delivered;
#[cfg(not(target_os = "macos"))]
pub fn delivered() -> Vec<String> { Vec::new() }
/// What this copy is allowed to post (macOS answers for real; elsewhere the answer is that nothing is asked for).
#[cfg(target_os = "macos")]
pub use macos::authorization;
#[cfg(not(target_os = "macos"))]
pub fn authorization() -> &'static str { "no permission needed" }

#[cfg(target_os = "macos")]
mod macos {
    use std::cell::RefCell;
    use std::sync::Mutex;
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::{Bool, ProtocolObject};
    use objc2::{define_class, msg_send, AllocAnyThread};
    use objc2_foundation::{NSBundle, NSError, NSObject, NSObjectProtocol, NSOperationQueue, NSString};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNMutableNotificationContent, UNNotification, UNNotificationPresentationOptions,
        UNNotificationRequest, UNNotificationResponse, UNNotificationSound, UNUserNotificationCenter,
        UNUserNotificationCenterDelegate,
    };

    define_class!(
        // SAFETY: an NSObject subclass with no ivars. The two optional `UNUserNotificationCenterDelegate` methods are
        // the ones the framework calls; both only read or answer their arguments.
        #[unsafe(super(NSObject))]
        #[name = "typeThaiNotifications"]
        #[ivars = ()]
        struct Delegate;

        unsafe impl NSObjectProtocol for Delegate {}

        unsafe impl UNUserNotificationCenterDelegate for Delegate {
            /// A toast shows while typeThai itself is in front (Settings open on the Updates page) instead of being
            /// silently dropped: a banner, and a row left in Notification Center.
            #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
            fn will_present(&self, _center: &UNUserNotificationCenter, _notification: &UNNotification, done: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>) {
                done.call((UNNotificationPresentationOptions::Banner | UNNotificationPresentationOptions::List,));
            }
            /// The click: the same request the tray item raises, applied by the UI loop.
            #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
            fn did_receive(&self, _center: &UNUserNotificationCenter, _response: &UNNotificationResponse, done: &block2::DynBlock<dyn Fn()>) {
                crate::bridge::REQ_UPDATE_CLICK.store(true, std::sync::atomic::Ordering::SeqCst);
                done.call(());
            }
        }
    );

    impl Delegate {
        fn new() -> Retained<Self> {
            let this = Self::alloc().set_ivars(());
            unsafe { msg_send![super(this), init] }
        }
    }

    thread_local! {
        /// `setDelegate:` is weak, so the delegate has to be kept alive here. It is set up from the UI loop, on the
        /// main thread, and the app's life is the object's.
        static DELEGATE: RefCell<Option<Retained<Delegate>>> = const { RefCell::new(None) };
    }
    /// Whether the user let typeThai post notifications: `None` until the first one is asked for.
    static GRANTED: Mutex<Option<bool>> = Mutex::new(None);

    /// The notification center, when this process may talk to it at all. A build without a bundle (`cargo run`, a
    /// bare executable) has no identifier, and `currentNotificationCenter` traps without one.
    fn center() -> Option<Retained<UNUserNotificationCenter>> {
        NSBundle::mainBundle().bundleIdentifier()?;
        let center = UNUserNotificationCenter::currentNotificationCenter();
        DELEGATE.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_none() {
                let delegate = Delegate::new();
                center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
                *slot = Some(delegate);
            }
        });
        Some(center)
    }

    /// Post `title`/`body`, asking for permission the first time there is something to say, so the prompt arrives with
    /// its reason rather than at launch. False when this copy cannot post notifications at all: a build without a
    /// bundle, or a permission the user refused.
    pub fn post(title: &str, body: &str) -> bool {
        let Some(notifications) = center() else { return false };
        match *GRANTED.lock().unwrap() {
            Some(true) => { send(&notifications, title, body); true }
            Some(false) => false,
            None => {
                let (title, body) = (title.to_string(), body.to_string());
                let done = RcBlock::new(move |granted: Bool, _error: *mut NSError| {
                    *GRANTED.lock().unwrap() = Some(granted.as_bool());
                    if !granted.as_bool() { return }
                    // the answer arrives on a queue of the framework's choosing; post on the main one
                    let (title, body) = (title.clone(), body.clone());
                    let post = RcBlock::new(move || if let Some(notifications) = center() { send(&notifications, &title, &body) });
                    unsafe { NSOperationQueue::mainQueue().addOperationWithBlock(&post) };
                });
                notifications.requestAuthorizationWithOptions_completionHandler(
                    UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
                    &done,
                );
                true
            }
        }
    }

    fn send(center: &UNUserNotificationCenter, title: &str, body: &str) {
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(title));
        content.setBody(&NSString::from_str(body));
        content.setSound(Some(&UNNotificationSound::defaultSound()));
        // the title names the release, so a later toast for the same one replaces it instead of stacking
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(&NSString::from_str(title), &content, None);
        let failed = RcBlock::new(|error: *mut NSError| {
            if let Some(error) = unsafe { error.as_ref() } { eprintln!("typeThai: update notification not posted: {error}") }
        });
        center.addNotificationRequest_withCompletionHandler(&request, Some(&failed));
    }

    /// What macOS currently allows this app to post (`Authorized`, `Denied`, `NotDetermined`, …), for `--demo-toast` to
    /// say why a notification did not appear.
    pub fn authorization() -> &'static str {
        use objc2_user_notifications::UNAuthorizationStatus;
        let Some(notifications) = center() else { return "no bundle" };
        let (tx, rx) = std::sync::mpsc::channel();
        let done = RcBlock::new(move |settings: std::ptr::NonNull<objc2_user_notifications::UNNotificationSettings>| {
            let status = unsafe { settings.as_ref() }.authorizationStatus();
            let _ = tx.send(match status {
                UNAuthorizationStatus::NotDetermined => "not asked yet",
                UNAuthorizationStatus::Denied => "refused in System Settings > Notifications",
                UNAuthorizationStatus::Authorized => "allowed",
                UNAuthorizationStatus::Provisional => "provisional",
                _ => "unknown",
            });
        });
        notifications.getNotificationSettingsWithCompletionHandler(&done);
        rx.recv_timeout(std::time::Duration::from_secs(3)).unwrap_or("no answer")
    }

    /// The titles of the notifications this app has in Notification Center, for `--demo-toast` to check its own work
    /// (and for the caller to see what the system actually took, rather than what was sent).
    pub fn delivered() -> Vec<String> {
        use std::sync::mpsc;
        let Some(notifications) = center() else { return vec![] };
        let (tx, rx) = mpsc::channel();
        let done = RcBlock::new(move |list: std::ptr::NonNull<objc2_foundation::NSArray<UNNotification>>| {
            let list = unsafe { list.as_ref() };
            let titles = list.iter().map(|n| n.request().content().title().to_string()).collect::<Vec<_>>();
            let _ = tx.send(titles);
        });
        notifications.getDeliveredNotificationsWithCompletionHandler(&done);
        rx.recv_timeout(std::time::Duration::from_secs(3)).unwrap_or_default()
    }
}

#[cfg(windows)]
mod windows {
    use std::sync::Mutex;
    use windows::core::HSTRING;
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::Foundation::TypedEventHandler;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
    use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;

    /// The tile typeThai's toasts belong to: the installer gives the Start-menu shortcut the same id (see
    /// `packaging/typethai.iss`), which is what names the notification "typeThai" instead of the exe path.
    const AUMID: &str = "io.github.phasu21.typethai";

    /// The toast that is still on screen: its `Activated` event only reaches this process while the object it was
    /// registered on is alive.
    static LIVE: Mutex<Option<ToastNotification>> = Mutex::new(None);
    /// WinRT wants a COM apartment on the calling thread, and gpui may not have made one. A thread that already has one
    /// ignores this: STA on STA answers `S_FALSE`, and an MTA thread answers `RPC_E_CHANGED_MODE`, either of which
    /// leaves WinRT usable.
    static COM: std::sync::LazyLock<()> = std::sync::LazyLock::new(|| {
        use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_SINGLETHREADED};
        let _ = unsafe { RoInitialize(RO_INIT_SINGLETHREADED) };
    });

    /// False when the toast could not be handed to the shell (`show` says why, once, in the log).
    pub fn post(title: &str, body: &str) -> bool {
        std::sync::LazyLock::force(&COM);
        match show(title, body) {
            Ok(()) => true,
            Err(e) => { eprintln!("typeThai: update notification not shown: {e}"); false }
        }
    }

    fn show(title: &str, body: &str) -> windows::core::Result<()> {
        let _ = unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(AUMID)) };
        let xml = XmlDocument::new()?;
        xml.LoadXml(&HSTRING::from(format!(
            "<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>",
            escape(title), escape(body)
        )))?;
        let toast = ToastNotification::CreateToastNotification(&xml)?;
        toast.Activated(&TypedEventHandler::new(|_, _| {
            crate::bridge::REQ_UPDATE_CLICK.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }))?;
        ToastNotificationManager::CreateToastNotifier()?.Show(&toast)?;
        *LIVE.lock().unwrap() = Some(toast);
        Ok(())
    }

    /// The toast body is XML, and the release names in it are ours — but a stray `&` would cost the whole notification.
    fn escape(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    }
}
