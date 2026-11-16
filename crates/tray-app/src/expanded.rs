//! macOS 27's expanded interface session: the status item tells the app when to open and
//! close a window hung from it, and keeps the item drawn pressed while it is open.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObjectProtocol};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::NSStatusItem;
use objc2_foundation::NSObject;
use tao::event_loop::EventLoopProxy;

use crate::backend::UserEvent;

// objc2-app-kit 0.3 has no bindings for this macOS 27 API, so it is reached by message.

pub struct DelegateIvars {
    proxy: EventLoopProxy<UserEvent>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TurnrayExpandedInterfaceDelegate"]
    #[ivars = DelegateIvars]
    pub struct Delegate;

    impl Delegate {
        #[unsafe(method(statusItem:didBeginExpandedInterfaceSession:))]
        fn did_begin(&self, _item: &NSStatusItem, _session: &AnyObject) {
            let _ = self.ivars().proxy.send_event(UserEvent::ExpandedBegan);
        }

        #[unsafe(method(statusItemDidEndExpandedInterfaceSession:animated:))]
        fn did_end(&self, _item: &NSStatusItem, _animated: Bool) {
            let _ = self.ivars().proxy.send_event(UserEvent::ExpandedEnded);
        }
    }

    unsafe impl NSObjectProtocol for Delegate {}
);

impl Delegate {
    pub fn new(proxy: EventLoopProxy<UserEvent>, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DelegateIvars { proxy });
        unsafe { msg_send![super(this), init] }
    }
}

/// Makes `delegate` open and close `item`'s window. False before macOS 27, which has no
/// such session. The item holds its delegate weakly, so the caller keeps it.
pub fn adopt(item: &NSStatusItem, delegate: &Delegate) -> bool {
    let supported: bool =
        unsafe { msg_send![item, respondsToSelector: sel!(setExpandedInterfaceDelegate:)] };
    if supported {
        unsafe {
            let _: () = msg_send![item, setExpandedInterfaceDelegate: delegate];
        }
    }
    supported
}

/// Ends the open session, if any; the delegate then hears that it ended.
pub fn cancel(item: &NSStatusItem) {
    unsafe {
        let session: Option<Retained<AnyObject>> = msg_send![item, expandedInterfaceSession];
        if let Some(session) = session {
            let _: () = msg_send![&*session, cancel];
        }
    }
}
