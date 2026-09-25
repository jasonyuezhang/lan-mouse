use super::{Capture, CaptureError, CaptureEvent, Position, error::MacosCaptureCreationError};
use async_trait::async_trait;
use bitflags::bitflags;
use core_foundation::{
    array::CFArray,
    base::{CFRelease, CFType, TCFType, kCFAllocatorDefault},
    date::CFTimeInterval,
    dictionary::CFDictionary,
    number::{CFBooleanRef, CFNumber, kCFBooleanTrue},
    runloop::{CFRunLoop, CFRunLoopSource, kCFRunLoopCommonModes},
    string::{CFString, CFStringCreateWithCString, CFStringRef, kCFStringEncodingUTF8},
};
use core_graphics::{
    base::{CGError, kCGErrorSuccess},
    display::{CGDisplay, CGPoint},
    event::{
        CGEvent, CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions,
        CGEventTapPlacement, CGEventTapProxy, CGEventType, CGMouseButton, CallbackResult,
        EventField,
    },
    event_source::{CGEventSource, CGEventSourceStateID},
    geometry::CGRect,
    window::{
        copy_window_info, kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly,
    },
};
use futures_core::Stream;
use input_event::{
    BTN_BACK, BTN_FORWARD, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT, Event, KeyboardEvent, PointerEvent,
    screen::{Edge, EdgeSegments, Rect},
};
use keycode::{KeyMap, KeyMapping};
use libc::c_void;
use once_cell::unsync::Lazy;
use std::{
    collections::HashSet,
    ffi::{CString, c_char},
    pin::Pin,
    sync::{Arc, OnceLock},
    task::{Context, Poll},
    thread::{self},
    time::{Duration, Instant},
};
use tokio::sync::{
    Mutex,
    mpsc::{self, Receiver, Sender},
    oneshot,
};

static FILE_DRAG_CLOCK: OnceLock<Instant> = OnceLock::new();
const LOCAL_FILE_DRAG_TAG: i64 = 0x4c4d4443414e434c; // LMDCANCL
static FILE_DRAG_READY_UNTIL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
fn file_drag_clock() -> u64 {
    FILE_DRAG_CLOCK
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis() as u64
}
pub fn set_file_drag_ready(ready: bool) {
    FILE_DRAG_READY_UNTIL.store(
        if ready { file_drag_clock() + 1500 } else { 0 },
        std::sync::atomic::Ordering::Relaxed,
    );
}
fn file_drag_ready() -> bool {
    let until = FILE_DRAG_READY_UNTIL.load(std::sync::atomic::Ordering::Relaxed);
    until != 0 && file_drag_clock() < until
}

/// End the source application's drag using the daemon's existing Accessibility
/// permission. Process-addressed events never enter the shared input stream.
pub fn cancel_source_file_drag(pid: i32) {
    if pid <= 0 || pid == std::process::id() as i32 {
        return;
    }
    let Ok(source) = CGEventSource::new(CGEventSourceStateID::Private) else {
        log::warn!("could not create source-drag cancellation event source");
        return;
    };
    let (Ok(down), Ok(up)) = (
        CGEvent::new_keyboard_event(source.clone(), 53, true),
        CGEvent::new_keyboard_event(source, 53, false),
    ) else {
        log::warn!("could not create source-drag cancellation events");
        return;
    };
    for event in [down, up] {
        event.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, LOCAL_FILE_DRAG_TAG);
        event.post_to_pid(pid);
    }
    log::info!("sent source file drag cancellation to process {pid}");
}

/// Refuse to click until the relay is the frontmost window at the click point.
fn relay_covers_point(pid: i32, window: i32, point: CGPoint) -> bool {
    unsafe extern "C" {
        fn CGWindowLevelForKey(key: i32) -> i32;
    }
    // kCGCursorWindowLevelKey. Quartz includes the pointer image in its list;
    // that image cannot intercept a click and must not hide the relay from us.
    let cursor_level = i64::from(unsafe { CGWindowLevelForKey(19) });
    let Some(list) = copy_window_info(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        0,
    ) else {
        return false;
    };
    // Quartz documents this array as dictionaries with CFString keys.
    let list: CFArray<CFDictionary<CFString, CFType>> =
        unsafe { CFArray::wrap_under_get_rule(list.as_concrete_TypeRef()) };
    for info in list.iter() {
        let number = |key: &str| {
            info.find(CFString::new(key))
                .and_then(|v| v.downcast::<CFNumber>())
                .and_then(|n| n.to_i64())
        };
        if number("kCGWindowLayer") == Some(cursor_level) {
            continue;
        }
        let bounds = info
            .find(CFString::new("kCGWindowBounds"))
            .and_then(|v| v.downcast::<CFDictionary>())
            .and_then(|d| CGRect::from_dict_representation(&d));
        if bounds.is_some_and(|r| r.contains(&point)) {
            let covers = number("kCGWindowOwnerPID") == Some(i64::from(pid))
                && number("kCGWindowNumber") == Some(i64::from(window));
            if !covers {
                log::debug!(
                    "native drag window {window} at {point:?} is covered by window {:?}, owner {:?}, layer {:?}",
                    number("kCGWindowNumber"),
                    number("kCGWindowOwnerPID"),
                    number("kCGWindowLayer")
                );
            }
            return covers;
        }
    }
    false
}

/// Establish WindowServer mouse tracking only after the relay covers the click.
/// Process-addressed events neither select a dispatch window nor hold the global
/// button state; a drag started with them ends at its first motion event.
pub fn begin_native_file_drag(pid: i32, window: i32, x: i32, y: i32) -> bool {
    if pid <= 0 || pid == std::process::id() as i32 || window <= 0 {
        return false;
    }
    let point = CGPoint::new(f64::from(x), f64::from(y));
    if !relay_covers_point(pid, window, point) {
        return false;
    }
    let Ok(source) = CGEventSource::new(CGEventSourceStateID::CombinedSessionState) else {
        log::warn!("could not create native file drag event source");
        return false;
    };
    let Ok(event) = CGEvent::new_mouse_event(
        source,
        CGEventType::LeftMouseDown,
        point,
        CGMouseButton::Left,
    ) else {
        log::warn!("could not create native file drag mouse-down");
        return false;
    };
    event.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, LOCAL_FILE_DRAG_TAG);
    event.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, 1);
    // A late UI activation must not resurrect a released drag. Retried startup
    // requests may only inject one press for this held-button lifetime.
    if !input_event::macos::claim_file_drag_button() {
        return false;
    }
    event.post(CGEventTapLocation::HID);
    log::info!("started native file drag over verified relay window {window}, process {pid}");
    true
}

/// A tap disabled by timeout this often within this window is not having a
/// one-off stall; stop re-enabling it and tear down instead
const TAP_TIMEOUTS_FATAL: usize = 3;
const TAP_TIMEOUT_WINDOW: Duration = Duration::from_secs(30);

/// Union of all displays. `xmin`/`ymin` is the first visible pixel,
/// `xmax`/`ymax` is one past the last.
#[derive(Debug, Default)]
struct Bounds {
    xmin: f64,
    xmax: f64,
    ymin: f64,
    ymax: f64,
}

impl Bounds {
    /// Does a pointer at `location` moving by `delta` leave the desktop on
    /// `position`'s side?
    ///
    /// Strict on all four sides: the pointer has to be pushed *past* the outer
    /// pixel, sitting on it is not enough. The event tap also sees the events
    /// our own emulation posts, and a peer's positioned `Enter` warps the
    /// cursor exactly onto `xmin`/`ymin` with a zero delta. With `<=` that
    /// warp tripped our own barrier, which told the peer to release, whose
    /// warp tripped its barrier, ... — a visible enter/leave ping-pong at the
    /// edge (and a hook storm) every time the cursor came back from a client
    /// under the peer's own mouse.
    fn crosses(&self, position: Position, location: (f64, f64), delta: (f64, f64)) -> bool {
        let (x, y) = (location.0 + delta.0, location.1 + delta.1);
        match position {
            Position::Left => x < self.xmin,
            Position::Right => x >= self.xmax,
            Position::Top => y < self.ymin,
            Position::Bottom => y >= self.ymax,
        }
    }
}

#[derive(Debug)]
struct InputCaptureState {
    /// active capture positions
    active_clients: Lazy<HashSet<Position>>,
    /// the currently entered capture position, if any
    current_pos: Option<Position>,
    /// position where the cursor was captured
    enter_position: Option<CGPoint>,
    /// bounds of the input capture area
    bounds: Bounds,
    /// current state of modifier keys
    modifier_state: XMods,
    /// recent `TapDisabledByTimeout` events, to tell a one-off stall from a
    /// tap macOS keeps killing
    tap_timeouts: Vec<Instant>,
}

#[derive(Debug)]
enum ProducerEvent {
    Release,
    Create(Position),
    Destroy(Position),
    Grab(Position),
    EventTapDisabled,
    DisplayReconfigured,
}

impl InputCaptureState {
    fn new() -> Result<Self, MacosCaptureCreationError> {
        let mut res = Self {
            active_clients: Lazy::new(HashSet::new),
            current_pos: None,
            enter_position: None,
            bounds: Bounds::default(),
            modifier_state: Default::default(),
            tap_timeouts: Vec::new(),
        };
        res.update_bounds()?;
        Ok(res)
    }

    fn crossed(&mut self, event: &CGEvent) -> Option<Position> {
        let location = event.location();
        let relative_x = event.get_double_value_field(EventField::MOUSE_EVENT_DELTA_X);
        let relative_y = event.get_double_value_field(EventField::MOUSE_EVENT_DELTA_Y);

        for &position in self.active_clients.iter() {
            if self
                .bounds
                .crosses(position, (location.x, location.y), (relative_x, relative_y))
            {
                log::debug!("Crossed barrier into position: {position:?}");
                return Some(position);
            }
        }
        None
    }

    /// Normalized position (0..=1) along the exposed desktop edge the cursor
    /// is about to cross, so the peer can place the cursor at the same height / offset.
    fn cross_axis(&self, event: &CGEvent, position: Position) -> Option<f32> {
        let location = event.location();
        let edge = match position {
            Position::Left => Edge::Left,
            Position::Right => Edge::Right,
            Position::Top => Edge::Top,
            Position::Bottom => Edge::Bottom,
        };
        let cross = match edge {
            Edge::Left | Edge::Right => location.y,
            Edge::Top | Edge::Bottom => location.x,
        }
        .round() as i32;
        let segments = EdgeSegments::from_rectangles(edge, display_rectangles());
        // the cursor may not sit exactly on the edge pixel yet; look the edge
        // coordinate up from the segment covering our cross-axis position.
        let edge_coordinate = segments
            .segments()
            .find(|s| cross >= s.cross_start && cross < s.cross_end)?
            .edge_coordinate;
        segments.normalize(edge_coordinate, cross)
    }

    // Get the max bounds of all displays
    fn update_bounds(&mut self) -> Result<(), MacosCaptureCreationError> {
        let active_ids =
            CGDisplay::active_displays().map_err(MacosCaptureCreationError::ActiveDisplays)?;
        active_ids.iter().for_each(|d| {
            let bounds = CGDisplay::new(*d).bounds();
            self.bounds.xmin = self.bounds.xmin.min(bounds.origin.x);
            self.bounds.xmax = self.bounds.xmax.max(bounds.origin.x + bounds.size.width);
            self.bounds.ymin = self.bounds.ymin.min(bounds.origin.y);
            self.bounds.ymax = self.bounds.ymax.max(bounds.origin.y + bounds.size.height);
        });

        log::debug!("Updated displays bounds: {0:?}", self.bounds);
        Ok(())
    }

    /// start the input capture by
    fn start_capture(&mut self, event: &CGEvent, position: Position) -> Result<(), CaptureError> {
        let mut location = event.location();
        let edge_offset = 1.0;
        // move cursor location to display bounds
        match position {
            Position::Left => location.x = self.bounds.xmin + edge_offset,
            Position::Right => location.x = self.bounds.xmax - edge_offset,
            Position::Top => location.y = self.bounds.ymin + edge_offset,
            Position::Bottom => location.y = self.bounds.ymax - edge_offset,
        };
        self.enter_position = Some(location);
        self.reset_cursor()
    }

    /// resets the cursor to the position, where the capture started
    fn reset_cursor(&mut self) -> Result<(), CaptureError> {
        let pos = self.enter_position.expect("capture active");
        log::trace!("Resetting cursor position to: {}, {}", pos.x, pos.y);
        CGDisplay::warp_mouse_cursor_position(pos).map_err(CaptureError::WarpCursor)
    }

    fn hide_cursor(&self) -> Result<(), CaptureError> {
        CGDisplay::hide_cursor(&CGDisplay::main()).map_err(CaptureError::CoreGraphics)
    }

    fn show_cursor(&self) -> Result<(), CaptureError> {
        CGDisplay::show_cursor(&CGDisplay::main()).map_err(CaptureError::CoreGraphics)
    }

    async fn handle_producer_event(
        &mut self,
        producer_event: ProducerEvent,
    ) -> Result<(), CaptureError> {
        log::debug!("handling event: {producer_event:?}");
        match producer_event {
            ProducerEvent::Release => {
                if self.current_pos.is_some() {
                    self.show_cursor()?;
                    self.current_pos = None;
                }
            }
            ProducerEvent::Grab(pos) => {
                if self.current_pos.is_none() {
                    self.hide_cursor()?;
                    self.current_pos = Some(pos);
                }
            }
            ProducerEvent::Create(p) => {
                self.active_clients.insert(p);
            }
            ProducerEvent::Destroy(p) => {
                if let Some(current) = self.current_pos {
                    if current == p {
                        self.show_cursor()?;
                        self.current_pos = None;
                    };
                }
                self.active_clients.remove(&p);
            }
            ProducerEvent::EventTapDisabled => {
                // Tap death can happen mid-capture (TCC Accessibility
                // revoked, tap-timeout, etc). Release state so we
                // don't leave the cursor hidden even if the outer
                // task only logs this error rather than propagating.
                if self.current_pos.is_some() {
                    self.show_cursor()?;
                    self.current_pos = None;
                }
                return Err(CaptureError::EventTapDisabled);
            }
            ProducerEvent::DisplayReconfigured => {
                // The macOS display configuration changed — a monitor
                // was plugged in/out, the resolution changed, the
                // arrangement was rearranged, etc. Re-fetch the
                // active-display bounds so barrier crossings and the
                // cursor-warp on capture-start use the current
                // geometry instead of whatever was true at process
                // start.
                if let Err(e) = self.update_bounds() {
                    log::warn!("failed to refresh display bounds: {e}");
                } else {
                    log::info!("display reconfigured: {:?}", self.bounds);
                }
            }
        };
        Ok(())
    }
}

fn get_events(
    ev_type: &CGEventType,
    ev: &CGEvent,
    result: &mut Vec<CaptureEvent>,
    modifier_state: &mut XMods,
) -> Result<(), CaptureError> {
    fn map_pointer_event(ev: &CGEvent) -> PointerEvent {
        PointerEvent::Motion {
            time: 0,
            dx: ev.get_double_value_field(EventField::MOUSE_EVENT_DELTA_X),
            dy: ev.get_double_value_field(EventField::MOUSE_EVENT_DELTA_Y),
        }
    }

    fn map_key(ev: &CGEvent) -> Result<u32, CaptureError> {
        let code = ev.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
        match KeyMap::from_key_mapping(KeyMapping::Mac(code as u16)) {
            Ok(k) => Ok(k.evdev as u32),
            Err(()) => Err(CaptureError::KeyMapError(code)),
        }
    }

    match ev_type {
        CGEventType::KeyDown => {
            if ev.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT) != 0 {
                return Ok(());
            }
            let k = map_key(ev)?;
            result.push(CaptureEvent::Input(Event::Keyboard(KeyboardEvent::Key {
                time: 0,
                key: k,
                state: 1,
            })));
        }
        CGEventType::KeyUp => {
            let k = map_key(ev)?;
            result.push(CaptureEvent::Input(Event::Keyboard(KeyboardEvent::Key {
                time: 0,
                key: k,
                state: 0,
            })));
        }
        CGEventType::FlagsChanged => {
            let mut depressed = XMods::empty();
            let mut mods_locked = XMods::empty();
            let cg_flags = ev.get_flags();

            if cg_flags.contains(CGEventFlags::CGEventFlagShift) {
                depressed |= XMods::ShiftMask;
            }
            if cg_flags.contains(CGEventFlags::CGEventFlagControl) {
                depressed |= XMods::ControlMask;
            }
            if cg_flags.contains(CGEventFlags::CGEventFlagAlternate) {
                depressed |= XMods::Mod1Mask;
            }
            if cg_flags.contains(CGEventFlags::CGEventFlagCommand) {
                depressed |= XMods::Mod4Mask;
            }
            if cg_flags.contains(CGEventFlags::CGEventFlagAlphaShift) {
                depressed |= XMods::LockMask;
                mods_locked |= XMods::LockMask;
            }

            // check if pressed or released
            let state = if depressed > *modifier_state { 1 } else { 0 };
            *modifier_state = depressed;

            if let Ok(key) = map_key(ev) {
                let key_event = CaptureEvent::Input(Event::Keyboard(KeyboardEvent::Key {
                    time: 0,
                    key,
                    state,
                }));
                result.push(key_event);
            }

            let modifier_event = KeyboardEvent::Modifiers {
                depressed: depressed.bits(),
                latched: 0,
                locked: mods_locked.bits(),
                group: 0,
            };

            result.push(CaptureEvent::Input(Event::Keyboard(modifier_event)));
        }
        CGEventType::MouseMoved => {
            result.push(CaptureEvent::Input(Event::Pointer(map_pointer_event(ev))))
        }
        CGEventType::LeftMouseDragged => {
            result.push(CaptureEvent::Input(Event::Pointer(map_pointer_event(ev))))
        }
        CGEventType::RightMouseDragged => {
            result.push(CaptureEvent::Input(Event::Pointer(map_pointer_event(ev))))
        }
        CGEventType::OtherMouseDragged => {
            result.push(CaptureEvent::Input(Event::Pointer(map_pointer_event(ev))))
        }
        CGEventType::LeftMouseDown => {
            result.push(CaptureEvent::Input(Event::Pointer(PointerEvent::Button {
                time: 0,
                button: BTN_LEFT,
                state: 1,
            })))
        }
        CGEventType::LeftMouseUp => {
            result.push(CaptureEvent::Input(Event::Pointer(PointerEvent::Button {
                time: 0,
                button: BTN_LEFT,
                state: 0,
            })))
        }
        CGEventType::RightMouseDown => {
            result.push(CaptureEvent::Input(Event::Pointer(PointerEvent::Button {
                time: 0,
                button: BTN_RIGHT,
                state: 1,
            })))
        }
        CGEventType::RightMouseUp => {
            result.push(CaptureEvent::Input(Event::Pointer(PointerEvent::Button {
                time: 0,
                button: BTN_RIGHT,
                state: 0,
            })))
        }
        CGEventType::OtherMouseDown => {
            let btn_num = ev.get_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER);
            let button = match btn_num {
                3 => BTN_BACK,
                4 => BTN_FORWARD,
                _ => BTN_MIDDLE,
            };
            result.push(CaptureEvent::Input(Event::Pointer(PointerEvent::Button {
                time: 0,
                button,
                state: 1,
            })))
        }
        CGEventType::OtherMouseUp => {
            let btn_num = ev.get_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER);
            let button = match btn_num {
                3 => BTN_BACK,
                4 => BTN_FORWARD,
                _ => BTN_MIDDLE,
            };
            result.push(CaptureEvent::Input(Event::Pointer(PointerEvent::Button {
                time: 0,
                button,
                state: 0,
            })))
        }
        CGEventType::ScrollWheel => {
            if ev.get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_IS_CONTINUOUS) != 0 {
                let v =
                    ev.get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_POINT_DELTA_AXIS_1);
                let h =
                    ev.get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_POINT_DELTA_AXIS_2);
                if v != 0 {
                    result.push(CaptureEvent::Input(Event::Pointer(PointerEvent::Axis {
                        time: 0,
                        axis: 0, // Vertical
                        value: v as f64,
                    })));
                }
                if h != 0 {
                    result.push(CaptureEvent::Input(Event::Pointer(PointerEvent::Axis {
                        time: 0,
                        axis: 1, // Horizontal
                        value: h as f64,
                    })));
                }
            } else {
                // line based scrolling
                const LINES_PER_STEP: i32 = 3;
                const V120_STEPS_PER_LINE: i32 = 120 / LINES_PER_STEP;
                let v = ev.get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_1);
                let h = ev.get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_2);
                if v != 0 {
                    result.push(CaptureEvent::Input(Event::Pointer(
                        PointerEvent::AxisDiscrete120 {
                            axis: 0, // Vertical
                            value: V120_STEPS_PER_LINE * v as i32,
                        },
                    )));
                }
                if h != 0 {
                    result.push(CaptureEvent::Input(Event::Pointer(
                        PointerEvent::AxisDiscrete120 {
                            axis: 1, // Horizontal
                            value: V120_STEPS_PER_LINE * h as i32,
                        },
                    )));
                }
            }
        }
        _ => (),
    }
    Ok(())
}

fn create_event_tap<'a>(
    client_state: Arc<Mutex<InputCaptureState>>,
    notify_tx: Sender<ProducerEvent>,
    event_tx: Sender<(Position, CaptureEvent)>,
) -> Result<CGEventTap<'a>, MacosCaptureCreationError> {
    // Shared slot for the tap's mach port pointer. Stored as `usize`
    // because raw pointers aren't `Send`, but the integer
    // representation is — and CGEventTapEnable is documented as
    // thread-safe. Set immediately after CGEventTap::new returns;
    // read by the callback to recover from a TapDisabledByTimeout.
    let tap_mach_port: Arc<OnceLock<usize>> = Arc::new(OnceLock::new());
    let tap_mach_port_cb = Arc::clone(&tap_mach_port);

    let cg_events_of_interest: Vec<CGEventType> = vec![
        CGEventType::LeftMouseDown,
        CGEventType::LeftMouseUp,
        CGEventType::RightMouseDown,
        CGEventType::RightMouseUp,
        CGEventType::OtherMouseDown,
        CGEventType::OtherMouseUp,
        CGEventType::MouseMoved,
        CGEventType::LeftMouseDragged,
        CGEventType::RightMouseDragged,
        CGEventType::OtherMouseDragged,
        CGEventType::ScrollWheel,
        CGEventType::KeyDown,
        CGEventType::KeyUp,
        CGEventType::FlagsChanged,
    ];

    let event_tap_callback = move |_proxy: CGEventTapProxy,
                                   event_type: CGEventType,
                                   cg_ev: &CGEvent| {
        log::trace!("Got event from tap: {event_type:?}");
        // Window-addressed drag events stay local, including while capturing.
        if cg_ev.get_integer_value_field(EventField::EVENT_SOURCE_USER_DATA) == LOCAL_FILE_DRAG_TAG
        {
            return CallbackResult::Keep;
        }
        let mut state = client_state.blocking_lock();
        let mut capture_position = None;
        let mut res_events = vec![];

        if matches!(event_type, CGEventType::TapDisabledByTimeout) {
            // The kernel disables the tap when our callback runs
            // longer than ~1s on a single event — heavy load, this
            // process briefly suspended, or the daemon's thread
            // stalled while the callback waited on it. Apple's
            // documented recovery is to re-enable the tap.
            //
            // But never while capturing: in that state every local
            // event is dropped and the cursor is pinned to the edge,
            // so resurrecting the tap with capture state intact keeps
            // the desk frozen for as long as the trouble lasts (a
            // revoked Accessibility permission made macOS kill the
            // tap 12 times in 90 s while the user could do nothing).
            // Give the desk back first; the peer's watchdog releases
            // our keys. And if the tap keeps dying, stop pretending
            // it is a one-off: tear down so the service can report it.
            let now = Instant::now();
            state
                .tap_timeouts
                .retain(|t| now.duration_since(*t) < TAP_TIMEOUT_WINDOW);
            state.tap_timeouts.push(now);
            if state.current_pos.is_some() {
                log::warn!("CGEventTap disabled by timeout while capturing — releasing capture");
                let _ = CGDisplay::show_cursor(&CGDisplay::main());
                state.current_pos = None;
                let _ = notify_tx.try_send(ProducerEvent::Release);
            }
            if state.tap_timeouts.len() >= TAP_TIMEOUTS_FATAL {
                log::error!(
                    "CGEventTap disabled by timeout {} times within {TAP_TIMEOUT_WINDOW:?} — giving up on it",
                    state.tap_timeouts.len()
                );
                let _ = notify_tx.try_send(ProducerEvent::EventTapDisabled);
                return CallbackResult::Keep;
            }
            if let Some(&port) = tap_mach_port_cb.get() {
                log::warn!("CGEventTap disabled by timeout — re-enabling");
                unsafe {
                    CGEventTapEnable(port as *mut c_void, true);
                }
            } else {
                log::error!(
                    "CGEventTap disabled by timeout, but mach port not yet stored — cannot re-enable"
                );
            }
            return CallbackResult::Keep;
        }

        if matches!(event_type, CGEventType::TapDisabledByUserInput) {
            // Deliberate kill — secure-input mode (e.g. password
            // field), TCC Accessibility revoked mid-session, or
            // the user disabling event-monitoring. We can't
            // recover from this; drop captured state synchronously
            // and return Keep on this event. Otherwise the
            // `current_pos.is_some()` branch below would drop this
            // event (and any racing callback still in flight) back
            // into `CallbackResult::Drop`, silently eating the
            // user's clicks and keypresses while the tap winds
            // down. Clear state + show the cursor here, then
            // notify the producer loop so the service can tear
            // down cleanly.
            log::error!("CGEventTap disabled by user input, releasing capture state");
            if state.current_pos.is_some() {
                let _ = CGDisplay::show_cursor(&CGDisplay::main());
                state.current_pos = None;
            }
            notify_tx
                .try_send(ProducerEvent::EventTapDisabled)
                .unwrap_or_else(|e| {
                    log::error!("Failed to send notification: {e}");
                });
            return CallbackResult::Keep;
        }

        // Are we in a client?
        if let Some(current_pos) = state.current_pos {
            capture_position = Some(current_pos);
            get_events(
                &event_type,
                cg_ev,
                &mut res_events,
                &mut state.modifier_state,
            )
            .unwrap_or_else(|e| {
                log::error!("Failed to get events: {e}");
            });

            // Keep (hidden) cursor at the edge of the screen
            if matches!(
                event_type,
                CGEventType::MouseMoved
                    | CGEventType::LeftMouseDragged
                    | CGEventType::RightMouseDragged
                    | CGEventType::OtherMouseDragged
            ) {
                state.reset_cursor().unwrap_or_else(|e| log::warn!("{e}"));
            }
        } else if matches!(event_type, CGEventType::MouseMoved)
            || (matches!(event_type, CGEventType::LeftMouseDragged) && file_drag_ready())
        {
            // Did we cross a barrier?
            if let Some(new_pos) = state.crossed(cg_ev) {
                capture_position = Some(new_pos);
                let cross_axis = state.cross_axis(cg_ev, new_pos);
                state
                    .start_capture(cg_ev, new_pos)
                    .unwrap_or_else(|e| log::warn!("{e}"));
                res_events.push(CaptureEvent::Begin { cross_axis });
                // a closed channel (instance being dropped) must not
                // panic-abort the daemon from inside the tap callback
                if let Err(e) = notify_tx.try_send(ProducerEvent::Grab(new_pos)) {
                    log::error!("failed to notify producer of capture start: {e}");
                    let _ = CGDisplay::show_cursor(&CGDisplay::main());
                    state.current_pos = None;
                    return CallbackResult::Keep;
                }
            }
        }

        if let Some(pos) = capture_position {
            // This callback runs inside the OS input path: if it blocks,
            // macOS stalls all input, then kills the tap. When the
            // daemon's thread is not keeping up, losing an event is far
            // better than freezing the desk — so never block here.
            res_events.iter().for_each(|e| {
                if let Err(err) = event_tx.try_send((pos, *e)) {
                    // closed when the instance is dropped, full when stalled
                    log::debug!("dropping captured event: {err}");
                }
            });
            // Returning Drop should stop the event from being processed
            // but core fundation still returns the event
            cg_ev.set_type(CGEventType::Null);
            CallbackResult::Drop
        } else {
            CallbackResult::Keep
        }
    };

    let tap = CGEventTap::new(
        // The integrated MMF helper processes Session events. Capturing at HID
        // guarantees raw input reaches exactly one Mac's mouse engine.
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::Default,
        cg_events_of_interest,
        event_tap_callback,
    )
    .map_err(|_| MacosCaptureCreationError::EventTapCreation)?;

    // Hand the mach port pointer to the callback so it can re-enable
    // the tap on TapDisabledByTimeout. The pointer is valid for the
    // lifetime of `tap` (which lives on the event-tap thread until
    // the run loop exits).
    let port_ptr = tap.mach_port().as_concrete_TypeRef() as usize;
    let _ = tap_mach_port.set(port_ptr);

    let tap_source: CFRunLoopSource = tap
        .mach_port()
        .create_runloop_source(0)
        .expect("Failed creating loop source");

    unsafe {
        CFRunLoop::get_current().add_source(&tap_source, kCFRunLoopCommonModes);
    }

    Ok(tap)
}

fn event_tap_thread(
    client_state: Arc<Mutex<InputCaptureState>>,
    event_tx: Sender<(Position, CaptureEvent)>,
    notify_tx: Sender<ProducerEvent>,
    ready: std::sync::mpsc::Sender<Result<CFRunLoop, MacosCaptureCreationError>>,
    exit: oneshot::Sender<()>,
) {
    // Clone now: create_event_tap consumes notify_tx into its closure.
    let display_notify_tx = notify_tx.clone();

    let _tap = match create_event_tap(client_state, notify_tx, event_tx) {
        Err(e) => {
            ready.send(Err(e)).expect("channel closed");
            return;
        }
        Ok(tap) => {
            let run_loop = CFRunLoop::get_current();
            ready.send(Ok(run_loop)).expect("channel closed");
            tap
        }
    };

    // Register a Quartz display-reconfiguration callback so the
    // capture state's bounds get refreshed when the user plugs in a
    // monitor, changes resolution, or rearranges displays. The
    // callback runs on this thread's CFRunLoop. Box-leak the sender
    // so the C side has a stable user_info pointer; reclaim it after
    // the run loop exits.
    let display_user_info = Box::into_raw(Box::new(display_notify_tx)) as *mut c_void;
    unsafe {
        CGDisplayRegisterReconfigurationCallback(
            display_reconfiguration_callback,
            display_user_info,
        );
    }

    log::debug!("running CFRunLoop...");
    CFRunLoop::run_current();
    log::debug!("event tap thread exiting!...");

    unsafe {
        CGDisplayRemoveReconfigurationCallback(display_reconfiguration_callback, display_user_info);
        // Reclaim the leaked sender Box so we don't leak a tokio
        // channel sender on every capture create/destroy cycle.
        drop(Box::from_raw(
            display_user_info as *mut Sender<ProducerEvent>,
        ));
    }

    let _ = exit.send(());
}

/// Quartz display-reconfiguration callback. Fires twice per change:
/// once with `kCGDisplayBeginConfigurationFlag` set (BEFORE the
/// change is applied — the bounds are still stale at this point),
/// then again afterwards with the actual change flags (Add, Remove,
/// Mode, DesktopShapeChanged, etc.). Skip the begin phase; on the
/// real notification, kick the producer task to refresh bounds.
extern "C" fn display_reconfiguration_callback(_display: u32, flags: u32, user_info: *mut c_void) {
    const K_CG_DISPLAY_BEGIN_CONFIGURATION_FLAG: u32 = 1 << 0;
    if flags & K_CG_DISPLAY_BEGIN_CONFIGURATION_FLAG != 0 {
        return;
    }
    if user_info.is_null() {
        return;
    }
    // SAFETY: user_info is a Box::into_raw of Sender<ProducerEvent>
    // owned by `event_tap_thread`. It's valid for the lifetime of
    // that thread; the registration is removed before the box is
    // freed. The callback only fires while the run loop is running
    // on that thread, so we know the box is live here.
    let sender = unsafe { &*(user_info as *const Sender<ProducerEvent>) };
    if let Err(e) = sender.blocking_send(ProducerEvent::DisplayReconfigured) {
        log::warn!("failed to notify display reconfiguration: {e}");
    }
}

pub struct MacOSInputCapture {
    event_rx: Receiver<(Position, CaptureEvent)>,
    notify_tx: Sender<ProducerEvent>,
    run_loop: CFRunLoop,
    key_repeat: super::key_repeat::KeyRepeat,
    capture_state: Arc<Mutex<InputCaptureState>>,
}

impl MacOSInputCapture {
    pub async fn new() -> Result<Self, MacosCaptureCreationError> {
        request_macos_capture_permissions()?;

        let state = Arc::new(Mutex::new(InputCaptureState::new()?));
        let (event_tx, event_rx) = mpsc::channel(32);
        let (notify_tx, mut notify_rx) = mpsc::channel(32);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (tap_exit_tx, mut tap_exit_rx) = oneshot::channel();

        unsafe {
            configure_cf_settings()?;
        }

        log::info!("Enabling CGEvent tap");
        let event_tap_thread_state = state.clone();
        let event_tap_notify = notify_tx.clone();
        thread::spawn(move || {
            event_tap_thread(
                event_tap_thread_state,
                event_tx,
                event_tap_notify,
                ready_tx,
                tap_exit_tx,
            )
        });

        // wait for event tap creation result
        let run_loop = ready_rx.recv().expect("channel closed")?;

        let capture_state = state.clone();
        let _tap_task: tokio::task::JoinHandle<()> = tokio::task::spawn_local(async move {
            loop {
                tokio::select! {
                    producer_event = notify_rx.recv() => {
                        let Some(producer_event) = producer_event else {
                            break;
                        };
                        let mut state = state.lock().await;
                        state.handle_producer_event(producer_event).await.unwrap_or_else(|e| {
                            log::error!("Failed to handle producer event: {e}");
                        })
                    }
                    _ = &mut tap_exit_rx => break,
                }
            }
            // show cursor
            let _ = CGDisplay::show_cursor(&CGDisplay::main());
        });

        Ok(Self {
            event_rx,
            notify_tx,
            run_loop,
            capture_state,
            key_repeat: {
                let (delay, interval) = input_event::macos::system_key_repeat();
                super::key_repeat::KeyRepeat::new(delay, interval)
            },
        })
    }
}

/// Bounds of every active display in global (CGEvent) coordinates.
fn display_rectangles() -> Vec<Rect> {
    CGDisplay::active_displays()
        .unwrap_or_default()
        .into_iter()
        .map(|id| {
            let b = CGDisplay::new(id).bounds();
            Rect {
                x: b.origin.x.round() as i32,
                y: b.origin.y.round() as i32,
                width: b.size.width.round() as i32,
                height: b.size.height.round() as i32,
            }
        })
        .collect()
}

fn request_macos_capture_permissions() -> Result<(), MacosCaptureCreationError> {
    // Call both request functions unconditionally so macOS surfaces both
    // TCC prompts on the very first launch. TCC always returns `false` the
    // first time a permission is requested (the grant only becomes visible
    // on the next process launch), so returning early on the first failure
    // would skip the second prompt and force the user through an extra
    // relaunch just to see it.
    let accessibility = request_accessibility_permission();
    let input_monitoring = request_input_monitoring_permission();

    if !accessibility {
        return Err(MacosCaptureCreationError::AccessibilityPermission);
    }
    if !input_monitoring {
        return Err(MacosCaptureCreationError::InputMonitoringPermission);
    }
    Ok(())
}

fn request_accessibility_permission() -> bool {
    // Silent check. The GUI owns the one-time user-visible prompt at
    // startup (see lan_mouse_gtk::macos_privacy) so retries triggered by
    // clicking the "Reenable" button don't pop a fresh Accessibility
    // alert every time.
    unsafe { AXIsProcessTrusted() }
}

fn request_input_monitoring_permission() -> bool {
    // Silent check, same reasoning as above.
    unsafe { CGPreflightListenEventAccess() }
}

impl Drop for MacOSInputCapture {
    fn drop(&mut self) {
        self.run_loop.stop();
    }
}

#[async_trait]
impl Capture for MacOSInputCapture {
    async fn create(&mut self, pos: Position) -> Result<(), CaptureError> {
        let notify_tx = self.notify_tx.clone();
        tokio::task::spawn_local(async move {
            log::debug!("creating capture, {pos}");
            let _ = notify_tx.send(ProducerEvent::Create(pos)).await;
            log::debug!("done !");
        });
        Ok(())
    }

    async fn destroy(&mut self, pos: Position) -> Result<(), CaptureError> {
        self.key_repeat.clear();
        let notify_tx = self.notify_tx.clone();
        tokio::task::spawn_local(async move {
            log::debug!("destroying capture {pos}");
            let _ = notify_tx.send(ProducerEvent::Destroy(pos)).await;
            log::debug!("done !");
        });
        Ok(())
    }

    async fn release(&mut self) -> Result<(), CaptureError> {
        self.key_repeat.clear();
        let notify_tx = self.notify_tx.clone();
        tokio::task::spawn_local(async move {
            log::debug!("notifying Release");
            let _ = notify_tx.send(ProducerEvent::Release).await;
        });
        Ok(())
    }

    async fn terminate(&mut self) -> Result<(), CaptureError> {
        self.key_repeat.clear();
        Ok(())
    }
}

impl Stream for MacOSInputCapture {
    type Item = Result<(Position, CaptureEvent), CaptureError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Drain physical events before considering repeats, especially key-up.
        match self.event_rx.poll_recv(cx) {
            Poll::Ready(None) => {
                self.key_repeat.clear();
                Poll::Ready(None)
            }
            Poll::Ready(Some((pos, event))) => {
                self.key_repeat.observe(pos, event);
                Poll::Ready(Some(Ok((pos, event))))
            }
            Poll::Pending => match self.key_repeat.poll(cx) {
                Poll::Ready((pos, event)) => {
                    // A tap timeout or permission change may release capture
                    // without delivering a key-up. Never repeat after that.
                    let active = self
                        .capture_state
                        .try_lock()
                        .ok()
                        .map(|state| state.current_pos == Some(pos));
                    match active {
                        Some(true) => Poll::Ready(Some(Ok((pos, event)))),
                        Some(false) => {
                            self.key_repeat.clear();
                            Poll::Pending
                        }
                        None => {
                            // Skip this tick if the tap is updating its state.
                            // Register the reset timer before yielding.
                            let _ = self.key_repeat.poll(cx);
                            Poll::Pending
                        }
                    }
                }
                Poll::Pending => Poll::Pending,
            },
        }
    }
}

type CGSConnectionID = u32;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn CGSSetConnectionProperty(
        cid: CGSConnectionID,
        targetCID: CGSConnectionID,
        key: CFStringRef,
        value: CFBooleanRef,
    ) -> CGError;
    fn _CGSDefaultConnection() -> CGSConnectionID;
}

extern "C" {
    fn CGEventSourceSetLocalEventsSuppressionInterval(
        event_source: CGEventSource,
        seconds: CFTimeInterval,
    );
    fn CGPreflightListenEventAccess() -> bool;
    /// Re-enable an event tap that was disabled by a
    /// `kCGEventTapDisabledByTimeout` event. The Apple-documented
    /// recovery path: see Quartz Event Services Reference. The `tap`
    /// argument is a `CFMachPortRef`; we pass the raw pointer so we
    /// can store it as `usize` for cross-thread sharing.
    fn CGEventTapEnable(tap: *mut c_void, enable: bool);

    /// Register a callback invoked when the display configuration
    /// changes (monitor add/remove, resolution change, mirror,
    /// rearrange, etc). See Quartz Display Services Reference.
    fn CGDisplayRegisterReconfigurationCallback(
        callback: extern "C" fn(u32, u32, *mut c_void),
        user_info: *mut c_void,
    ) -> CGError;
    fn CGDisplayRemoveReconfigurationCallback(
        callback: extern "C" fn(u32, u32, *mut c_void),
        user_info: *mut c_void,
    ) -> CGError;
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

unsafe fn configure_cf_settings() -> Result<(), MacosCaptureCreationError> {
    // When we warp the cursor using CGWarpMouseCursorPosition local events are suppressed for a short time
    // this leeds to the cursor not flowing when crossing back from a clinet, set this to to 0 stops the warp
    // from working, set a low value by trial and error, 0.05s seems good. 0.25s is the default
    let event_source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
        .map_err(|_| MacosCaptureCreationError::EventSourceCreation)?;
    CGEventSourceSetLocalEventsSuppressionInterval(event_source, 0.05);
    // FIXME Memory Leak

    // This is a private settings that allows the cursor to be hidden while in the background.
    // It is used by Barrier and other apps.
    let key = CString::new("SetsCursorInBackground").unwrap();
    let cf_key = CFStringCreateWithCString(
        kCFAllocatorDefault,
        key.as_ptr() as *const c_char,
        kCFStringEncodingUTF8,
    );
    if CGSSetConnectionProperty(
        _CGSDefaultConnection(),
        _CGSDefaultConnection(),
        cf_key,
        kCFBooleanTrue,
    ) != kCGErrorSuccess
    {
        return Err(MacosCaptureCreationError::CGCursorProperty);
    }
    CFRelease(cf_key as *const c_void);
    Ok(())
}

// From X11/X.h
bitflags! {
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
    struct XMods: u32 {
        const ShiftMask = (1<<0);
        const LockMask = (1<<1);
        const ControlMask = (1<<2);
        const Mod1Mask = (1<<3);
        const Mod2Mask = (1<<4);
        const Mod3Mask = (1<<5);
        const Mod4Mask = (1<<6);
        const Mod5Mask = (1<<7);
    }
}

#[cfg(test)]
mod tests {
    use super::{Bounds, Position};

    // 1920x1080 main display with a 1440x900 one to its right; x: 0..3360
    fn desktop() -> Bounds {
        Bounds {
            xmin: 0.0,
            xmax: 3360.0,
            ymin: 0.0,
            ymax: 1080.0,
        }
    }

    #[test]
    fn a_peer_warping_us_onto_the_edge_pixel_is_not_a_crossing() {
        let b = desktop();
        // emulation.warp_cursor(): absolute MouseMoved onto the edge, no delta
        assert!(!b.crosses(Position::Left, (0.0, 500.0), (0.0, 0.0)));
        assert!(!b.crosses(Position::Top, (800.0, 0.0), (0.0, 0.0)));
        // and neither is the peer then moving us inwards or along the edge
        assert!(!b.crosses(Position::Left, (0.0, 500.0), (3.0, 0.0)));
        assert!(!b.crosses(Position::Left, (0.0, 500.0), (0.0, -7.0)));
    }

    #[test]
    fn pushing_past_the_edge_pixel_crosses() {
        let b = desktop();
        // macOS pins the location on the outer pixel; the delta keeps going
        assert!(b.crosses(Position::Left, (0.0, 500.0), (-1.0, 0.0)));
        assert!(b.crosses(Position::Top, (800.0, 0.0), (0.0, -1.0)));
        assert!(b.crosses(Position::Right, (3359.0, 500.0), (1.0, 0.0)));
        assert!(b.crosses(Position::Bottom, (800.0, 1079.0), (0.0, 1.0)));
    }

    #[test]
    fn near_and_far_edges_behave_the_same() {
        let b = desktop();
        // sitting on the outer pixel with no delta: no crossing on any side
        assert!(!b.crosses(Position::Right, (3359.0, 500.0), (0.0, 0.0)));
        assert!(!b.crosses(Position::Bottom, (800.0, 1079.0), (0.0, 0.0)));
        // well inside, large movement that stays inside
        assert!(!b.crosses(Position::Left, (100.0, 500.0), (-99.0, 0.0)));
        assert!(!b.crosses(Position::Right, (3000.0, 500.0), (359.0, 0.0)));
    }

    #[test]
    fn only_the_configured_side_counts() {
        let b = desktop();
        assert!(!b.crosses(Position::Right, (0.0, 500.0), (-5.0, 0.0)));
        assert!(!b.crosses(Position::Left, (3359.0, 500.0), (5.0, 0.0)));
    }
}

#[cfg(test)]
mod file_drag_tests {
    use super::*;
    #[test]
    fn file_drag_permission_is_a_short_lived_lease() {
        set_file_drag_ready(true);
        assert!(file_drag_ready());
        FILE_DRAG_READY_UNTIL.store(file_drag_clock(), std::sync::atomic::Ordering::Relaxed);
        assert!(!file_drag_ready());
        set_file_drag_ready(false);
        assert!(!file_drag_ready());
    }
}
