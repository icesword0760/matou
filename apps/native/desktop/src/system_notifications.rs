#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Authorization {
    NotDetermined,
    Denied,
    Authorized,
}

pub struct SystemRequest {
    pub identifier: String,
    pub title: String,
    pub subtitle: String,
    pub body: String,
    pub thread_id: Option<String>,
    pub event_id: String,
    pub play_sound: bool,
}

pub fn notification_identifier(session_id: Option<&str>, event_id: &str) -> String {
    session_id.map_or_else(|| format!("event:{event_id}"), str::to_owned)
}

#[cfg(any(target_os = "windows", test))]
pub const NOTIFICATION_URI_PREFIX: &str = "matou-notification://open/";
#[cfg(any(target_os = "windows", test))]
pub const MAX_NOTIFICATION_EVENT_ID_BYTES: usize = 512;

#[cfg(any(target_os = "windows", test))]
pub fn valid_notification_event_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_NOTIFICATION_EVENT_ID_BYTES
        && !id.chars().any(char::is_control)
}

pub use platform::*;

#[cfg(target_os = "macos")]
mod platform {
    // objc 0.2 macros check a dependency-internal feature in the caller.
    #![allow(unexpected_cfgs)]

    use std::{
        ffi::{CStr, c_void},
        ptr,
        sync::{
            Mutex, OnceLock,
            atomic::{AtomicBool, AtomicU8, Ordering},
        },
    };

    use block2::{Block, RcBlock};
    use cocoa::{
        appkit::NSApp,
        base::{BOOL, YES, id, nil},
    };
    use core_foundation::{base::TCFType, string::CFString};
    use objc::{
        Encode, Encoding, class,
        declare::ClassDecl,
        msg_send,
        runtime::{Class, Object, Sel},
        sel, sel_impl,
    };

    use super::{Authorization, SystemRequest};

    #[link(name = "UserNotifications", kind = "framework")]
    unsafe extern "C" {
        static UNNotificationDefaultActionIdentifier: id;
    }

    #[repr(C)]
    struct DispatchQueue {
        _opaque: [u8; 0],
    }

    unsafe extern "C" {
        static _dispatch_main_q: DispatchQueue;
        fn dispatch_async_f(
            queue: *const DispatchQueue,
            context: *mut c_void,
            work: extern "C" fn(*mut c_void),
        );
    }

    const SOUND: usize = 1 << 1;
    const ALERT: usize = 1 << 2;
    const LIST: usize = 1 << 3;
    const BANNER: usize = 1 << 4;

    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    static DELEGATE: OnceLock<usize> = OnceLock::new();
    static AUTHORIZATION: AtomicU8 = AtomicU8::new(0);
    static REQUESTING: AtomicBool = AtomicBool::new(false);
    static ACTIVATIONS: Mutex<Vec<String>> = Mutex::new(Vec::new());

    #[repr(transparent)]
    struct CompletionBlock(*mut c_void);

    // Objective-C represents a block parameter with the @? encoding.
    unsafe impl Encode for CompletionBlock {
        fn encode() -> Encoding {
            unsafe { Encoding::from_str("@?") }
        }
    }

    pub fn is_available() -> bool {
        *AVAILABLE.get_or_init(|| unsafe {
            let bundle: id = msg_send![class!(NSBundle), mainBundle];
            let identifier: id = msg_send![bundle, bundleIdentifier];
            let executable: id = msg_send![bundle, bundleURL];
            let extension: id = msg_send![executable, pathExtension];
            let app = CFString::new("app");
            let is_app: BOOL = msg_send![extension, isEqualToString: app.as_concrete_TypeRef()];
            !identifier.is_null() && is_app == YES
        })
    }

    unsafe fn center() -> id {
        unsafe { msg_send![class!(UNUserNotificationCenter), currentNotificationCenter] }
    }

    pub fn install() {
        if !is_available() {
            return;
        }
        DELEGATE.get_or_init(|| unsafe {
            let delegate_class = Class::get("MatouNotificationDelegate").unwrap_or_else(|| {
                let mut class = ClassDecl::new("MatouNotificationDelegate", class!(NSObject))
                    .expect("declare notification delegate");
                class.add_method(
                    sel!(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:),
                    did_receive as extern "C" fn(&Object, Sel, id, id, CompletionBlock),
                );
                class.add_method(
                    sel!(userNotificationCenter:willPresentNotification:withCompletionHandler:),
                    will_present as extern "C" fn(&Object, Sel, id, id, CompletionBlock),
                );
                class.register()
            });
            // +new returns an owned object. Keep it alive for the process because
            // UNUserNotificationCenter holds a weak reference to its delegate.
            let delegate: id = msg_send![delegate_class, new];
            let _: () = msg_send![center(), setDelegate: delegate];
            delegate as usize
        });
    }

    extern "C" fn activate_app(_: *mut c_void) {
        unsafe {
            let _: () = msg_send![NSApp(), activateIgnoringOtherApps: YES];
        }
    }

    extern "C" fn did_receive(
        _: &Object,
        _: Sel,
        _: id,
        response: id,
        completion: CompletionBlock,
    ) {
        unsafe {
            let action: id = msg_send![response, actionIdentifier];
            let is_click: BOOL =
                msg_send![action, isEqualToString: UNNotificationDefaultActionIdentifier];
            if is_click == YES {
                let notification: id = msg_send![response, notification];
                let request: id = msg_send![notification, request];
                let identifier: id = msg_send![request, identifier];
                let bytes: *const std::ffi::c_char = msg_send![identifier, UTF8String];
                if !bytes.is_null()
                    && let Ok(mut pending) = ACTIVATIONS.lock()
                {
                    pending.push(CStr::from_ptr(bytes).to_string_lossy().into_owned());
                }
                dispatch_async_f(&raw const _dispatch_main_q, ptr::null_mut(), activate_app);
            }
            let block = &*(completion.0 as *const Block<dyn Fn()>);
            block.call(());
        }
    }

    extern "C" fn will_present(
        _: &Object,
        _: Sel,
        _: id,
        notification: id,
        completion: CompletionBlock,
    ) {
        unsafe {
            let request: id = msg_send![notification, request];
            let content: id = msg_send![request, content];
            let sound: id = msg_send![content, sound];
            let block = &*(completion.0 as *const Block<dyn Fn(usize)>);
            block.call((BANNER | LIST | if sound.is_null() { 0 } else { SOUND },));
        }
    }

    pub fn authorization() -> Authorization {
        match AUTHORIZATION.load(Ordering::Relaxed) {
            0 => Authorization::NotDetermined,
            2 | 3 => Authorization::Authorized,
            _ => Authorization::Denied,
        }
    }

    fn store_status(status: isize) {
        AUTHORIZATION.store(status as u8, Ordering::Relaxed);
    }

    fn get_settings(on_status: impl Fn(isize) + Send + Sync + 'static) {
        let handler = RcBlock::new(move |settings: *mut c_void| unsafe {
            let status: isize = msg_send![settings as id, authorizationStatus];
            on_status(status);
        });
        unsafe {
            let _: () = msg_send![
                center(),
                getNotificationSettingsWithCompletionHandler: RcBlock::as_ptr(&handler).cast::<c_void>()
            ];
        }
    }

    pub fn request_authorization_once() {
        if !is_available() || REQUESTING.swap(true, Ordering::AcqRel) {
            return;
        }
        get_settings(|status| {
            store_status(status);
            if status != 0 {
                REQUESTING.store(false, Ordering::Release);
                return;
            }

            // BOOL is a one-byte 0/1 value on both supported Mac architectures.
            let handler = RcBlock::new(|granted: u8, error: *mut c_void| {
                if error.is_null() {
                    store_status(if granted != 0 { 2 } else { 1 });
                } else {
                    eprintln!(
                        "Request macOS notification permission: authorization request failed"
                    );
                }
                REQUESTING.store(false, Ordering::Release);
                refresh_authorization();
            });
            unsafe {
                let _: () = msg_send![
                    center(),
                    requestAuthorizationWithOptions: ALERT | SOUND
                    completionHandler: RcBlock::as_ptr(&handler).cast::<c_void>()
                ];
            }
        });
    }

    pub fn refresh_authorization() {
        if is_available() && !REQUESTING.load(Ordering::Acquire) {
            get_settings(|status| {
                if !REQUESTING.load(Ordering::Acquire) {
                    store_status(status);
                }
            });
        }
    }

    pub fn app_is_active() -> bool {
        unsafe {
            let active: BOOL = msg_send![NSApp(), isActive];
            active == YES
        }
    }

    pub fn post(request: SystemRequest) -> Option<async_channel::Receiver<bool>> {
        if !is_available() || authorization() != Authorization::Authorized || app_is_active() {
            return None;
        }
        let (send, result) = async_channel::bounded(1);
        let completion = RcBlock::new(move |error: *mut c_void| {
            if !error.is_null() {
                eprintln!("Post macOS notification: notification request failed");
                refresh_authorization();
            }
            let _ = send.try_send(error.is_null());
        });
        unsafe {
            let identifier = CFString::new(&request.identifier);
            let title = CFString::new(&request.title);
            let subtitle = CFString::new(&request.subtitle);
            let body = CFString::new(&request.body);
            let event_id = CFString::new(&request.event_id);
            let event_key = CFString::new("eventId");
            let session_key = CFString::new("sessionId");

            let content: id = msg_send![class!(UNMutableNotificationContent), new];
            let _: () = msg_send![content, setTitle: title.as_concrete_TypeRef()];
            let _: () = msg_send![content, setSubtitle: subtitle.as_concrete_TypeRef()];
            let _: () = msg_send![content, setBody: body.as_concrete_TypeRef()];

            let user_info: id = msg_send![class!(NSMutableDictionary), dictionary];
            let _: () = msg_send![user_info, setObject: event_id.as_concrete_TypeRef() forKey: event_key.as_concrete_TypeRef()];
            if !request.identifier.starts_with("event:") && request.identifier != "probe" {
                let _: () = msg_send![user_info, setObject: identifier.as_concrete_TypeRef() forKey: session_key.as_concrete_TypeRef()];
            }
            let _: () = msg_send![content, setUserInfo: user_info];
            if let Some(thread_id) = &request.thread_id {
                let thread_id = CFString::new(thread_id);
                let _: () =
                    msg_send![content, setThreadIdentifier: thread_id.as_concrete_TypeRef()];
            }
            if request.play_sound {
                let sound: id = msg_send![class!(UNNotificationSound), defaultSound];
                let _: () = msg_send![content, setSound: sound];
            }
            let notification: id = msg_send![
                class!(UNNotificationRequest),
                requestWithIdentifier: identifier.as_concrete_TypeRef() content: content trigger: nil
            ];
            let _: () = msg_send![content, release];
            let _: () = msg_send![
                center(),
                addNotificationRequest: notification
                withCompletionHandler: RcBlock::as_ptr(&completion).cast::<c_void>()
            ];
        }
        Some(result)
    }

    pub fn take_pending_activations() -> Vec<String> {
        ACTIVATIONS
            .lock()
            .map(|mut pending| std::mem::take(&mut *pending))
            .unwrap_or_default()
    }

    pub fn open_notification_settings() {
        if !is_available() {
            return;
        }
        unsafe {
            let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
            for path in [
                "x-apple.systempreferences:com.apple.Notifications-Settings.extension",
                "x-apple.systempreferences:com.apple.preference.notifications",
            ] {
                let path = CFString::new(path);
                let url: id = msg_send![class!(NSURL), URLWithString: path.as_concrete_TypeRef()];
                let opened: BOOL = msg_send![workspace, openURL: url];
                if opened == YES {
                    break;
                }
            }
        }
    }
}

#[cfg(any(target_os = "windows", test))]
mod windows_toast {
    use super::{NOTIFICATION_URI_PREFIX, SystemRequest, valid_notification_event_id};

    pub fn activation_uri(event_id: &str) -> Option<String> {
        if !valid_notification_event_id(event_id) {
            return None;
        }
        let bytes = event_id.as_bytes();
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut uri = String::with_capacity(NOTIFICATION_URI_PREFIX.len() + bytes.len() * 2);
        uri.push_str(NOTIFICATION_URI_PREFIX);
        for byte in bytes {
            uri.push(char::from(HEX[usize::from(byte >> 4)]));
            uri.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        Some(uri)
    }

    pub fn tag(identifier: &str) -> String {
        // FNV-1a gives the same 16-character tag across process restarts.
        let hash = identifier
            .bytes()
            .fold(0xcbf29ce484222325_u64, |hash, byte| {
                (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
            });
        format!("{hash:016x}")
    }

    pub fn xml(request: &SystemRequest) -> Option<String> {
        let uri = activation_uri(&request.event_id)?;
        let mut xml = format!(
            r#"<toast activationType="protocol" launch="{uri}"><visual><binding template="ToastGeneric">"#
        );
        append_text(&mut xml, &request.title);
        if !request.subtitle.is_empty() {
            append_text(&mut xml, &request.subtitle);
        }
        append_text(&mut xml, &request.body);
        xml.push_str("</binding></visual>");
        if !request.play_sound {
            xml.push_str(r#"<audio silent="true"/>"#);
        }
        xml.push_str("</toast>");
        Some(xml)
    }

    fn append_text(xml: &mut String, text: &str) {
        xml.push_str("<text>");
        for character in text.chars() {
            match character {
                '&' => xml.push_str("&amp;"),
                '<' => xml.push_str("&lt;"),
                '>' => xml.push_str("&gt;"),
                '"' => xml.push_str("&quot;"),
                '\'' => xml.push_str("&apos;"),
                '\t'
                | '\n'
                | '\r'
                | '\u{20}'..='\u{d7ff}'
                | '\u{e000}'..='\u{fffd}'
                | '\u{10000}'..='\u{10ffff}' => xml.push(character),
                _ => {}
            }
        }
        xml.push_str("</text>");
    }

    pub fn registered_executable(command: &str) -> Option<&str> {
        // The per-user installer registers exactly "<absolute exe path>" "%1".
        let path = command.strip_prefix('"')?.strip_suffix(r#"" "%1""#)?;
        (!path.contains(['"', '\0'])
            && std::path::Path::new(path)
                .extension()?
                .eq_ignore_ascii_case("exe"))
        .then_some(path)
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use std::{
        env, fs, process,
        sync::{
            OnceLock,
            atomic::{AtomicU8, Ordering},
            mpsc::{self, Receiver, SyncSender},
        },
        thread,
    };

    use windows::{
        Data::Xml::Dom::XmlDocument,
        UI::Notifications::{NotificationSetting, ToastNotification, ToastNotificationManager},
        Win32::{
            Foundation::{ERROR_NOT_FOUND, ERROR_SUCCESS},
            System::{
                Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW},
                WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
            },
            UI::{
                Shell::{SetCurrentProcessExplicitAppUserModelID, ShellExecuteW},
                WindowsAndMessaging::{
                    GetForegroundWindow, GetWindowThreadProcessId, SW_SHOWNORMAL,
                },
            },
        },
        core::{HRESULT, HSTRING, PCWSTR, Result, w},
    };

    use super::{Authorization, SystemRequest, windows_toast};

    const APP_USER_MODEL_ID: &str = "com.matou.desktop";
    const PROTOCOL_COMMAND_KEY: PCWSTR =
        w!(r"Software\Classes\matou-notification\shell\open\command");
    static WORKER: OnceLock<Option<SyncSender<Work>>> = OnceLock::new();
    static AUTHORIZATION: AtomicU8 = AtomicU8::new(0);

    enum Work {
        RefreshAuthorization,
        Post(SystemRequest, async_channel::Sender<bool>),
    }

    pub fn install() {
        WORKER.get_or_init(start_worker);
    }

    fn start_worker() -> Option<SyncSender<Work>> {
        let command = read_protocol_command()?;
        let installed = std::path::Path::new(windows_toast::registered_executable(&command)?);
        if !installed.is_absolute()
            || fs::canonicalize(installed).ok()?
                != fs::canonicalize(env::current_exe().ok()?).ok()?
        {
            return None;
        }
        let app_id = HSTRING::from(APP_USER_MODEL_ID);
        if let Err(error) =
            unsafe { SetCurrentProcessExplicitAppUserModelID(PCWSTR(app_id.as_ptr())) }
        {
            eprintln!("Set Windows notification identity: {error}");
            return None;
        }
        // Keep WinRT and its system calls off GPUI's UI thread. Serial sends
        // preserve event order when several notifications replace the same tag.
        let (sender, receiver) = mpsc::sync_channel(32);
        if let Err(error) = thread::Builder::new()
            .name("windows-notifications".into())
            .spawn(move || run_worker(receiver))
        {
            eprintln!("Start Windows notification worker: {error}");
            return None;
        }
        Some(sender)
    }

    fn read_protocol_command() -> Option<String> {
        // Windows command lines are limited to 32,767 UTF-16 code units.
        let mut buffer = vec![0u16; 32_767];
        let mut bytes = (buffer.len() * size_of::<u16>()) as u32;
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                PROTOCOL_COMMAND_KEY,
                PCWSTR::null(),
                RRF_RT_REG_SZ,
                None,
                Some(buffer.as_mut_ptr().cast()),
                Some(&mut bytes),
            )
        };
        if status != ERROR_SUCCESS {
            return None;
        }
        let units = bytes as usize / size_of::<u16>();
        let command = buffer.get(..units)?.strip_suffix(&[0])?;
        String::from_utf16(command).ok()
    }

    fn run_worker(receiver: Receiver<Work>) {
        if let Err(error) = unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
            eprintln!("Initialize Windows notifications: {error}");
            return;
        }
        refresh_setting();
        while let Ok(work) = receiver.recv() {
            match work {
                Work::RefreshAuthorization => refresh_setting(),
                Work::Post(request, sender) => {
                    let shown = match show(request) {
                        Ok(shown) => shown,
                        Err(error) => {
                            eprintln!("Post Windows notification: {error}");
                            false
                        }
                    };
                    let _ = sender.try_send(shown);
                }
            }
        }
        unsafe { RoUninitialize() };
    }

    fn notifier() -> Result<windows::UI::Notifications::ToastNotifier> {
        ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_USER_MODEL_ID))
    }

    fn authorization_for_setting(setting: Result<NotificationSetting>) -> Result<Authorization> {
        match setting {
            Ok(NotificationSetting::Enabled) => Ok(Authorization::Authorized),
            Ok(_) => Ok(Authorization::Denied),
            // On a fresh installation Windows may create the sender's settings
            // on its first actual notification. Let Show enforce system policy.
            Err(error) if error.code() == HRESULT::from_win32(ERROR_NOT_FOUND.0) => {
                Ok(Authorization::NotDetermined)
            }
            Err(error) => Err(error),
        }
    }

    fn read_setting(notifier: &windows::UI::Notifications::ToastNotifier) -> Result<Authorization> {
        let setting = authorization_for_setting(notifier.Setting());
        let value = match &setting {
            Ok(Authorization::Authorized) => 1,
            Ok(Authorization::Denied) => 2,
            _ => 0,
        };
        AUTHORIZATION.store(value, Ordering::Relaxed);
        setting
    }

    fn refresh_setting() {
        if let Err(error) = notifier().and_then(|notifier| read_setting(&notifier)) {
            AUTHORIZATION.store(0, Ordering::Relaxed);
            eprintln!("Read Windows notification setting: {error}");
        }
    }

    fn show(request: SystemRequest) -> Result<bool> {
        let notifier = notifier()?;
        let setting = read_setting(&notifier)?;
        if setting == Authorization::Denied {
            return Ok(false);
        }
        let Some(xml) = windows_toast::xml(&request) else {
            return Ok(false);
        };
        let document = XmlDocument::new()?;
        document.LoadXml(&HSTRING::from(xml))?;
        let toast = ToastNotification::CreateToastNotification(&document)?;
        toast.SetTag(&HSTRING::from(windows_toast::tag(&request.identifier)))?;
        // Use the app's default notification group so a session replaces its
        // previous toast even when the session moves to another scene.
        notifier.Show(&toast)?;
        if setting == Authorization::NotDetermined
            && let Err(error) = read_setting(&notifier)
        {
            eprintln!("Read Windows notification setting after delivery: {error}");
        }
        Ok(true)
    }

    fn worker() -> Option<&'static SyncSender<Work>> {
        WORKER.get()?.as_ref()
    }

    pub fn is_available() -> bool {
        worker().is_some()
    }

    pub fn authorization() -> Authorization {
        match AUTHORIZATION.load(Ordering::Relaxed) {
            1 => Authorization::Authorized,
            2 => Authorization::Denied,
            _ => Authorization::NotDetermined,
        }
    }

    pub fn request_authorization_once() {
        refresh_authorization();
    }

    pub fn refresh_authorization() {
        if let Some(worker) = worker() {
            let _ = worker.try_send(Work::RefreshAuthorization);
        }
    }

    pub fn app_is_active() -> bool {
        let mut foreground_pid = 0;
        unsafe {
            let window = GetForegroundWindow();
            if !window.0.is_null() {
                GetWindowThreadProcessId(window, Some(&mut foreground_pid));
            }
        }
        foreground_pid == process::id()
    }

    pub fn post(request: SystemRequest) -> Option<async_channel::Receiver<bool>> {
        let worker = worker()?;
        if app_is_active() {
            return None;
        }
        let (send, result) = async_channel::bounded(1);
        worker.try_send(Work::Post(request, send)).ok()?;
        Some(result)
    }

    pub fn take_pending_activations() -> Vec<String> {
        // The protocol launcher hands activation to the desktop's single instance.
        Vec::new()
    }

    pub fn open_notification_settings() {
        let result = unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                w!("ms-settings:notifications"),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        if result.0 as isize <= 32 {
            eprintln!(
                "Open Windows notification settings: error {}",
                result.0 as isize
            );
        }
    }

    #[cfg(test)]
    #[test]
    fn windows_notification_sender_creation_and_disabled_setting_are_distinct() {
        use windows::core::Error;

        assert_eq!(
            authorization_for_setting(Err(Error::from_hresult(HRESULT::from_win32(
                ERROR_NOT_FOUND.0,
            ))))
            .unwrap(),
            Authorization::NotDetermined,
        );
        assert_eq!(
            authorization_for_setting(Ok(NotificationSetting::Enabled)).unwrap(),
            Authorization::Authorized,
        );
        assert_eq!(
            authorization_for_setting(Ok(NotificationSetting::DisabledForApplication)).unwrap(),
            Authorization::Denied,
        );
        let access_denied = HRESULT::from_win32(5);
        assert_eq!(
            authorization_for_setting(Err(Error::from_hresult(access_denied)))
                .unwrap_err()
                .code(),
            access_denied,
        );
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod platform {
    use super::{Authorization, SystemRequest};

    pub fn install() {}
    pub fn is_available() -> bool {
        false
    }
    pub fn authorization() -> Authorization {
        Authorization::Denied
    }
    pub fn request_authorization_once() {}
    pub fn refresh_authorization() {}
    pub fn app_is_active() -> bool {
        true
    }
    pub fn post(_: SystemRequest) -> Option<async_channel::Receiver<bool>> {
        None
    }
    pub fn take_pending_activations() -> Vec<String> {
        Vec::new()
    }
    pub fn open_notification_settings() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_identifier_prefers_session() {
        assert_eq!(notification_identifier(Some("s1"), "e1"), "s1");
        assert_eq!(notification_identifier(None, "e1"), "event:e1");
    }

    #[test]
    fn windows_toast_uri_encodes_utf8_and_limits_payload_size() {
        assert_eq!(
            windows_toast::activation_uri("会话 /\"").as_deref(),
            Some("matou-notification://open/e4bc9ae8af9d202f22")
        );
        assert_eq!(windows_toast::activation_uri(""), None);
        assert_eq!(windows_toast::activation_uri("invalid\u{0}control"), None);
        let limit = MAX_NOTIFICATION_EVENT_ID_BYTES;
        assert_eq!(
            windows_toast::activation_uri(&"x".repeat(limit))
                .unwrap()
                .len(),
            NOTIFICATION_URI_PREFIX.len() + limit * 2,
        );
        assert_eq!(windows_toast::activation_uri(&"x".repeat(limit + 1)), None);
    }

    #[test]
    fn windows_toast_plain_text_is_escaped_and_only_uses_legal_xml_characters() {
        let mut request = SystemRequest {
            identifier: "会话".into(),
            title: "<action>&\"'".into(),
            subtitle: "新消息".into(),
            body: "普通\u{0}内容\u{1b}\u{fffe} 😀]]>\n".into(),
            thread_id: None,
            event_id: "e1".into(),
            play_sound: false,
        };
        let xml = windows_toast::xml(&request).unwrap();
        assert_eq!(
            xml,
            concat!(
                r#"<toast activationType="protocol" launch="matou-notification://open/6531">"#,
                r#"<visual><binding template="ToastGeneric">"#,
                "<text>&lt;action&gt;&amp;&quot;&apos;</text><text>新消息</text>",
                "<text>普通内容 😀]]&gt;\n</text></binding></visual>",
                r#"<audio silent="true"/></toast>"#,
            ),
        );
        request.play_sound = true;
        request.subtitle.clear();
        let xml = windows_toast::xml(&request).unwrap();
        assert!(!xml.contains("<audio"));
        assert_eq!(xml.matches("<text>").count(), 2);
    }

    #[test]
    fn windows_toast_tag_replaces_by_session_and_fits_winrt() {
        let first = windows_toast::tag(&notification_identifier(Some("s1"), "e1"));
        let second = windows_toast::tag(&notification_identifier(Some("s1"), "e2"));
        assert_eq!(first, second);
        assert_eq!(first, "08d8ff07b578d149");
        assert_eq!(first.len(), 16);
        assert_ne!(
            first,
            windows_toast::tag(&notification_identifier(Some("s2"), "e1"))
        );
        assert_ne!(
            windows_toast::tag(&notification_identifier(None, "e1")),
            windows_toast::tag(&notification_identifier(None, "e2")),
        );
    }

    #[test]
    fn windows_installer_protocol_preserves_spaces_and_chinese_in_executable_path() {
        let path = r"C:\Users\测试 帐号\AppData\Local\Programs\Matou\码头.exe";
        assert_eq!(
            windows_toast::registered_executable(&format!(r#""{path}" "%1""#)),
            Some(path),
        );
        assert_eq!(
            windows_toast::registered_executable(&format!("{path} %1")),
            None
        );
        assert_eq!(
            windows_toast::registered_executable(&format!(r#""{path}" "%1" --flag"#)),
            None,
        );
    }
}
