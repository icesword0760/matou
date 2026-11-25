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

#[cfg(not(target_os = "macos"))]
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
}
