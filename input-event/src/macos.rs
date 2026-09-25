//! macOS state shared by capture and emulation.
use std::{
    ffi::{CStr, c_char, c_void},
    sync::atomic::{AtomicU8, Ordering},
    time::Duration,
};

// 0 = released, 1 = held and awaiting the native relay, 2 = native press sent.
// The receiver's actual input stream owns this state, not the UI timer.
static FILE_DRAG_BUTTON: AtomicU8 = AtomicU8::new(0);
pub fn adopt_file_drag_button() {
    FILE_DRAG_BUTTON.store(1, Ordering::SeqCst);
}
pub fn release_file_drag_button() {
    FILE_DRAG_BUTTON.store(0, Ordering::SeqCst);
}
pub fn claim_file_drag_button() -> bool {
    FILE_DRAG_BUTTON
        .compare_exchange(1, 2, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}

const FALLBACK_REPEAT_DELAY: Duration = Duration::from_millis(250);
const FALLBACK_REPEAT_INTERVAL: Duration = Duration::from_millis(33);

/// The user's key repeat delay and interval (System Settings → Keyboard),
/// as AppKit reports them. Read once at startup; the `defaults` values are
/// in 1/60 s ticks and AppKit already applies the system defaults, so ask it
/// rather than re-deriving the numbers.
pub fn system_key_repeat() -> (Duration, Duration) {
    let delay = ns_event_interval(c"keyRepeatDelay");
    let interval = ns_event_interval(c"keyRepeatInterval");
    (
        delay.unwrap_or(FALLBACK_REPEAT_DELAY),
        interval.unwrap_or(FALLBACK_REPEAT_INTERVAL),
    )
}

/// `+[NSEvent <selector>]` returning an NSTimeInterval, `None` if unusable
fn ns_event_interval(selector: &CStr) -> Option<Duration> {
    // SAFETY: plain class-method call on a class that exists in every AppKit;
    // `objc_msgSend` is cast to the signature of a method returning a double,
    // which is the standard way to call it without the objc runtime crates
    let seconds = unsafe {
        let class = objc_getClass(c"NSEvent".as_ptr());
        if class.is_null() {
            return None;
        }
        let send: unsafe extern "C" fn(*mut c_void, *mut c_void) -> f64 =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        send(class, sel_registerName(selector.as_ptr()))
    };
    // 0.0 is what a broken read looks like; a real "off" is a huge number
    (seconds.is_finite() && seconds > 0.0 && seconds < 10.0)
        .then(|| Duration::from_secs_f64(seconds))
}

// AppKit only for `+[NSEvent keyRepeatDelay]` / `keyRepeatInterval`
#[link(name = "AppKit", kind = "framework")]
extern "C" {}

#[link(name = "objc")]
extern "C" {
    fn objc_getClass(name: *const c_char) -> *mut c_void;
    fn sel_registerName(name: *const c_char) -> *mut c_void;
    fn objc_msgSend();
}
