//! Replay of Dock swipes (Spaces, Mission Control, App Exposé).
//!
//! Since macOS 27 the Dock ignores the CGEvent fields of a type-30 event and
//! reads the IOHIDEvent attached to it instead, so a field-for-field copy of
//! the captured event is not enough: the receiver builds a DockSwipe
//! IOHIDEvent from the forwarded fields and attaches it with SkyLight's
//! `SLEventSetIOHIDEvent`. Both APIs are private, so they are looked up at
//! runtime; if one disappears in a future macOS, swipes just stay inert.

use core_foundation_sys::base::CFRelease;
use core_graphics::event::CGEvent;
use foreign_types::ForeignType;
use input_event::{TrackpadEvent, trackpad::TrackpadValue};
use std::{
    ffi::{CStr, c_char, c_int, c_void},
    sync::OnceLock,
};

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

/// Attach the DockSwipe IOHIDEvent described by `trackpad` to `event`.
/// Returns false when the private APIs are unavailable.
pub(super) fn attach_hid_event(event: &CGEvent, trackpad: &TrackpadEvent) -> bool {
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
