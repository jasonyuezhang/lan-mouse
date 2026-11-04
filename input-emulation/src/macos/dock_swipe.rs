//! Replay of Dock swipes (Spaces, Mission Control, App Exposé).
//!
//! Since macOS 27 the Dock ignores the CGEvent fields of a type-30 event and
//! reads the IOHIDEvent attached to it instead, so a field-for-field copy of
//! the captured event is not enough: the receiver builds a DockSwipe
//! IOHIDEvent from the forwarded fields and attaches it with SkyLight's
//! `SLEventSetIOHIDEvent`. Both APIs are private, so they are looked up at
//! runtime; if one disappears in a future macOS, swipes just stay inert.

use super::LAN_MOUSE_EVENT_TAG;
use core_foundation_sys::base::CFRelease;
use core_graphics::{
    event::{CGEvent, CGEventTapLocation, EventField},
    event_source::CGEventSource,
};
use foreign_types::ForeignType;
use input_event::{
    TrackpadEvent,
    trackpad::{CG_DOCK_GESTURE, TrackpadValue},
};
use std::{
    cell::Cell,
    ffi::{CStr, c_char, c_int, c_void},
    rc::Rc,
    sync::OnceLock,
    time::Duration,
};
use tokio::task::JoinHandle;

type HidEvent = *mut c_void;

// IOHIDEventTypes.h / IOHIDEventFieldDefs.h (IOHIDFamily, open source)
const HID_TYPE_VELOCITY: u32 = 9;
const HID_TYPE_DOCK_SWIPE: u32 = 23;
const fn hid_field(event_type: u32, index: u32) -> u32 {
    (event_type << 16) | index
}
const DOCK_SWIPE_MOTION: u32 = hid_field(HID_TYPE_DOCK_SWIPE, 1);
const DOCK_SWIPE_PROGRESS: u32 = hid_field(HID_TYPE_DOCK_SWIPE, 2);
const DOCK_SWIPE_FLAVOR: u32 = hid_field(HID_TYPE_DOCK_SWIPE, 5);
const VELOCITY_X: u32 = hid_field(HID_TYPE_VELOCITY, 0);
const VELOCITY_Y: u32 = hid_field(HID_TYPE_VELOCITY, 1);
const PHASE_SHIFT: u32 = 24;
const PHASE_BEGAN: i64 = 1;
const PHASE_CHANGED: i64 = 2;
const PHASE_ENDED: i64 = 4;
const PHASE_CANCELLED: i64 = 8;
/// `kIOHIDGestureFlavorDockPrimary`, what a captured swipe carries in field 138.
const FLAVOR_DOCK_PRIMARY: i64 = 3;

// CGEvent fields of a captured type-30 event, as observed on macOS 26/27.
const CG_MOTION: u8 = 123;
const CG_PROGRESS: u8 = 124;
const CG_EXIT_SPEED_X: u8 = 129;
const CG_EXIT_SPEED_Y: u8 = 130;
const CG_PHASE: u8 = 132;
const CG_PHASE_2: u8 = 134;
const CG_INVERTED_FROM_DEVICE: u8 = 136;
const CG_FLAVOR: u8 = 138;

type CreateFn = unsafe extern "C" fn(*const c_void, u32, u64, u32) -> HidEvent;
type SetIntegerFn = unsafe extern "C" fn(HidEvent, u32, isize);
type SetFloatFn = unsafe extern "C" fn(HidEvent, u32, f64);
type AppendFn = unsafe extern "C" fn(HidEvent, HidEvent, u32);
type AttachFn = unsafe extern "C" fn(*mut c_void, HidEvent);

struct Api {
    create: CreateFn,
    set_integer: SetIntegerFn,
    set_float: SetFloatFn,
    append: AppendFn,
    attach: AttachFn,
}

unsafe extern "C" {
    fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| {
        const RTLD_LAZY: c_int = 1;
        let open = |path: &CStr| unsafe { dlopen(path.as_ptr(), RTLD_LAZY) };
        let iokit = open(c"/System/Library/Frameworks/IOKit.framework/IOKit");
        let skylight = open(c"/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight");
        let sym = |lib: *mut c_void, name: &CStr| {
            let p = if lib.is_null() {
                std::ptr::null_mut()
            } else {
                unsafe { dlsym(lib, name.as_ptr()) }
            };
            (!p.is_null()).then_some(p)
        };
        // SAFETY: signatures as declared in IOKitUser's IOHIDEventPrivate.h;
        // SLEventSetIOHIDEvent(CGEventRef, IOHIDEventRef) copies the event in.
        let api = unsafe {
            Api {
                create: std::mem::transmute::<*mut c_void, CreateFn>(sym(
                    iokit,
                    c"IOHIDEventCreate",
                )?),
                set_integer: std::mem::transmute::<*mut c_void, SetIntegerFn>(sym(
                    iokit,
                    c"IOHIDEventSetIntegerValue",
                )?),
                set_float: std::mem::transmute::<*mut c_void, SetFloatFn>(sym(
                    iokit,
                    c"IOHIDEventSetFloatValue",
                )?),
                append: std::mem::transmute::<*mut c_void, AppendFn>(sym(
                    iokit,
                    c"IOHIDEventAppendEvent",
                )?),
                attach: std::mem::transmute::<*mut c_void, AttachFn>(sym(
                    skylight,
                    c"SLEventSetIOHIDEvent",
                )?),
            }
        };
        Some(api)
    })
    .as_ref()
}

fn value(trackpad: &TrackpadEvent, field: u8) -> Option<TrackpadValue> {
    trackpad
        .fields()
        .find_map(|(id, v)| (id == field).then_some(v))
}

fn integer(trackpad: &TrackpadEvent, field: u8) -> i64 {
    match value(trackpad, field) {
        Some(TrackpadValue::Integer(v)) => v,
        Some(TrackpadValue::Double(v)) => v as i64,
        None => 0,
    }
}

fn double(trackpad: &TrackpadEvent, field: u8) -> f64 {
    match value(trackpad, field) {
        Some(TrackpadValue::Integer(v)) => v as f64,
        Some(TrackpadValue::Double(v)) => v,
        None => 0.0,
    }
}

/// A swipe whose frames stop arriving is ended after this long. Frames come
/// every ~10 ms while fingers move; a swipe left open (its end frame lost to a
/// full capture queue, UDP, or capture switching mid-gesture) leaves the Dock
/// mid-transition and the desktop unresponsive until the Dock restarts.
const STALL_TIMEOUT: Duration = Duration::from_millis(1000);

/// Replays Dock swipes and guarantees each one ends.
#[derive(Default)]
pub(super) struct DockSwipeReplay {
    /// last frame of a swipe that has begun but not ended
    open: Rc<Cell<Option<TrackpadEvent>>>,
    watchdog: Option<JoinHandle<()>>,
}

impl DockSwipeReplay {
    pub(super) fn post(&mut self, source: &CGEventSource, trackpad: &TrackpadEvent) {
        if let Some(task) = self.watchdog.take() {
            task.abort();
        }
        post_now(source, trackpad);
        let phase = integer(trackpad, CG_PHASE);
        // two lines per swipe: enough to tell a stuck Dock from a busy Mac
        match phase {
            PHASE_BEGAN => log::info!("dock swipe began (motion {})", integer(trackpad, CG_MOTION)),
            PHASE_CHANGED => {}
            _ => log::info!("dock swipe ended (phase {phase})"),
        }
        if !matches!(phase, PHASE_BEGAN | PHASE_CHANGED) {
            self.open.set(None);
            return;
        }
        self.open.set(Some(*trackpad));
        let (open, source) = (self.open.clone(), source.clone());
        self.watchdog = Some(tokio::task::spawn_local(async move {
            tokio::time::sleep(STALL_TIMEOUT).await;
            if let Some(last) = open.take() {
                log::warn!("dock swipe stalled without an end frame; ending it");
                post_now(&source, &ended(&last));
            }
        }));
    }

    /// End a swipe still in progress, e.g. when the peer goes away.
    pub(super) fn finish(&mut self, source: &CGEventSource) {
        if let Some(task) = self.watchdog.take() {
            task.abort();
        }
        if let Some(last) = self.open.take() {
            post_now(source, &ended(&last));
        }
    }
}

/// `last` turned into an end frame without exit velocity, so the Dock settles
/// on whichever side the swipe's progress is closer to.
fn ended(last: &TrackpadEvent) -> TrackpadEvent {
    let mut end = TrackpadEvent::new(CG_DOCK_GESTURE);
    for (field, value) in last.fields() {
        if matches!(
            field,
            CG_PHASE | CG_PHASE_2 | CG_EXIT_SPEED_X | CG_EXIT_SPEED_Y
        ) {
            continue;
        }
        // cannot overflow: same capacity, at most as many fields as `last`
        let _ = end.push(field, value);
    }
    let _ = end.push(CG_PHASE, TrackpadValue::Integer(PHASE_ENDED));
    let _ = end.push(CG_PHASE_2, TrackpadValue::Integer(PHASE_ENDED));
    end
}

fn post_now(source: &CGEventSource, trackpad: &TrackpadEvent) {
    /// private field that sets types CGEventType cannot represent
    const EVENT_TYPE_FIELD: u32 = 55;
    let Ok(event) = CGEvent::new(source.clone()) else {
        log::warn!("could not create {trackpad}");
        return;
    };
    event.set_integer_value_field(EVENT_TYPE_FIELD, CG_DOCK_GESTURE.into());
    for (field, value) in trackpad.fields() {
        match value {
            TrackpadValue::Integer(v) => event.set_integer_value_field(field.into(), v),
            TrackpadValue::Double(v) => event.set_double_value_field(field.into(), v),
        }
    }
    if !attach_hid_event(&event, trackpad) {
        log::debug!("dock swipe replay unavailable on this macOS");
    }
    // Posted after the HID taps: the Dock picks it up from here (as with Mac
    // Mouse Fix), and this Mac's own capture tap never sees it.
    event.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, LAN_MOUSE_EVENT_TAG);
    event.post(CGEventTapLocation::Session);
}

/// Attach the DockSwipe IOHIDEvent described by `trackpad` to `event`.
/// Returns false when the private APIs are unavailable.
fn attach_hid_event(event: &CGEvent, trackpad: &TrackpadEvent) -> bool {
    let Some(api) = api() else {
        return false;
    };
    let phase = integer(trackpad, CG_PHASE);
    // The captured progress already has natural scrolling applied ("inverted
    // from device"); the Dock applies it again to an attached IOHIDEvent, so
    // undo it here or swipes go the wrong way and spreading for Show Desktop
    // opens Launchpad / Apps instead (same as Mac Mouse Fix's macOS 27 path).
    let sign = if integer(trackpad, CG_INVERTED_FROM_DEVICE) != 0 {
        -1.0
    } else {
        1.0
    };
    let flavor = match integer(trackpad, CG_FLAVOR) {
        0 => FLAVOR_DOCK_PRIMARY,
        f => f,
    };
    // SAFETY: every IOHIDEvent created here is released after use; `attach`
    // copies the event into the CGEvent instead of retaining it.
    unsafe {
        let hid = (api.create)(
            std::ptr::null(),
            HID_TYPE_DOCK_SWIPE,
            0,
            (phase as u32) << PHASE_SHIFT,
        );
        if hid.is_null() {
            return false;
        }
        (api.set_integer)(
            hid,
            DOCK_SWIPE_MOTION,
            integer(trackpad, CG_MOTION) as isize,
        );
        (api.set_integer)(hid, DOCK_SWIPE_FLAVOR, flavor as isize);
        (api.set_float)(
            hid,
            DOCK_SWIPE_PROGRESS,
            sign * double(trackpad, CG_PROGRESS),
        );
        // the exit velocity decides whether the swipe completes or springs back
        if matches!(phase, PHASE_ENDED | PHASE_CANCELLED) {
            let velocity = (api.create)(std::ptr::null(), HID_TYPE_VELOCITY, 0, 0);
            if !velocity.is_null() {
                (api.set_float)(
                    velocity,
                    VELOCITY_X,
                    sign * double(trackpad, CG_EXIT_SPEED_X),
                );
                (api.set_float)(
                    velocity,
                    VELOCITY_Y,
                    sign * double(trackpad, CG_EXIT_SPEED_Y),
                );
                (api.append)(hid, velocity, 0);
                CFRelease(velocity);
            }
        }
        (api.attach)(event.as_ptr() as *mut c_void, hid);
        CFRelease(hid);
    }
    true
}
