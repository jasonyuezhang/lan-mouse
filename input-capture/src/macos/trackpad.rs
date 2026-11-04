//! Capture of native trackpad gestures (pinch, rotate, smart zoom, Spaces /
//! Mission Control swipes) for replay on another Mac.
//!
//! `core_graphics::CGEventTap` hands its callback a `CGEventType` enum that
//! has no variants for gesture types 29 / 30, so receiving one there would be
//! an invalid enum value. Gestures therefore get their own tap with a raw
//! `u32` event type, installed next to the main tap on the same run loop
//! (so ordering against scroll events is preserved).

use super::{CaptureEvent, InputCaptureState, Position};
use core_foundation::{
    base::TCFType,
    mach_port::{CFMachPort, CFMachPortRef},
    runloop::{CFRunLoop, kCFRunLoopCommonModes},
};
use core_graphics::{event::CGEvent, sys::CGEventRef};
use foreign_types::ForeignType;
use input_event::{
    Event,
    trackpad::{CG_DOCK_GESTURE, CG_GESTURE, TrackpadEvent, TrackpadValue, is_forwarded_field},
};
use libc::c_void;
use std::{mem::ManuallyDrop, ptr, sync::Arc};
use tokio::sync::{Mutex, mpsc::Sender};

/// Copy the forwarded payload fields of `ev` (see
/// [`input_event::trackpad::is_forwarded_field`]).
pub(super) fn trackpad_event(cg_type: u8, ev: &CGEvent) -> TrackpadEvent {
    let mut out = TrackpadEvent::new(cg_type);
    for field in 0..=u8::MAX {
        if !is_forwarded_field(cg_type, field) {
            continue;
        }
        let i = ev.get_integer_value_field(field.into());
        let d = ev.get_double_value_field(field.into());
        if i == 0 && d == 0.0 {
            continue;
        }
        // Each getter converts from the field's stored kind; a fractional
        // (or NaN) double that disagrees with the integer view is a double.
        let value = if d != 0.0 && d != i as f64 {
            TrackpadValue::Double(d)
        } else {
            TrackpadValue::Integer(i)
        };
        if out.push(field, value).is_err() {
            log::debug!("trackpad event type {cg_type}: dropping fields from {field}");
            break;
        }
    }
    out
}

const TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
const TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;

type RawTapCallback = unsafe extern "C" fn(
    proxy: *const c_void,
    event_type: u32,
    event: CGEventRef,
    user_info: *mut c_void,
) -> CGEventRef;

unsafe extern "C" {
    // Same symbol core_graphics binds, but with a raw `u32` event type.
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: RawTapCallback,
        user_info: *mut c_void,
    ) -> CFMachPortRef;
}

struct Context {
    state: Arc<Mutex<InputCaptureState>>,
    event_tx: Sender<(Position, CaptureEvent)>,
    port: std::sync::OnceLock<usize>,
}

/// Owns the gesture tap; drop it on the tap thread after its run loop exits.
pub(super) struct GestureTap {
    // Declared first so the port is released before the context it points at.
    _port: CFMachPort,
    context: *mut Context,
}

impl Drop for GestureTap {
    fn drop(&mut self) {
        // SAFETY: created by Box::into_raw in `install`; the run loop that
        // could invoke the callback has exited.
        drop(unsafe { Box::from_raw(self.context) });
    }
}

impl GestureTap {
    /// Install on the current thread's run loop. Failure only disables
    /// gesture forwarding; pointer and keyboard capture are unaffected.
    pub(super) fn install(
        state: Arc<Mutex<InputCaptureState>>,
        event_tx: Sender<(Position, CaptureEvent)>,
    ) -> Option<Self> {
        const HID_EVENT_TAP: u32 = 0;
        const HEAD_INSERT: u32 = 0;
        const DEFAULT_OPTIONS: u32 = 0;
        let mask = (1u64 << CG_GESTURE) | (1u64 << CG_DOCK_GESTURE);
        let context = Box::into_raw(Box::new(Context {
            state,
            event_tx,
            port: Default::default(),
        }));
        // SAFETY: `context` outlives the tap (see Drop).
        let port = unsafe {
            CGEventTapCreate(
                HID_EVENT_TAP,
                HEAD_INSERT,
                DEFAULT_OPTIONS,
                mask,
                gesture_callback,
                context as *mut c_void,
            )
        };
        if port.is_null() {
            drop(unsafe { Box::from_raw(context) });
            log::warn!("could not create trackpad gesture tap; gestures stay local");
            return None;
        }
        let port = unsafe { CFMachPort::wrap_under_create_rule(port) };
        let _ = unsafe { &*context }
            .port
            .set(port.as_concrete_TypeRef() as usize);
        let Ok(source) = port.create_runloop_source(0) else {
            drop(unsafe { Box::from_raw(context) });
            log::warn!("could not attach trackpad gesture tap; gestures stay local");
            return None;
        };
        unsafe { CFRunLoop::get_current().add_source(&source, kCFRunLoopCommonModes) };
        log::debug!("trackpad gesture tap installed");
        Some(Self {
            _port: port,
            context,
        })
    }
}

unsafe extern "C" fn gesture_callback(
    _proxy: *const c_void,
    event_type: u32,
    event: CGEventRef,
    user_info: *mut c_void,
) -> CGEventRef {
    // SAFETY: `user_info` is the live Context installed with this tap.
    let context = unsafe { &*(user_info as *const Context) };
    match event_type {
        TAP_DISABLED_BY_TIMEOUT => {
            // The main tap owns capture recovery; just keep gestures flowing.
            if let Some(&port) = context.port.get() {
                unsafe { super::CGEventTapEnable(port as *mut c_void, true) };
            }
            return event;
        }
        TAP_DISABLED_BY_USER_INPUT => return event,
        _ => {}
    }
    let Some(pos) = context.state.blocking_lock().current_pos else {
        return event;
    };
    // SAFETY: borrowed for the duration of the callback; the caller owns it.
    let ev = ManuallyDrop::new(unsafe { CGEvent::from_ptr(event) });
    let trackpad = trackpad_event(event_type as u8, &ev);
    // Never block the OS input path (see the main tap).
    if let Err(e) = context
        .event_tx
        .try_send((pos, CaptureEvent::Input(Event::Trackpad(trackpad))))
    {
        log::debug!("dropping captured gesture: {e}");
    }
    // While capturing, the gesture belongs to the other Mac.
    ptr::null_mut()
}
