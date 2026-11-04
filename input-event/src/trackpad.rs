//! Native macOS trackpad events (gestures and phased scrolling) forwarded
//! verbatim between two Macs.
//!
//! macOS has no public API to synthesize pinch, rotate or Spaces/Mission
//! Control swipes, and scroll phases (momentum, rubber-banding, back/forward
//! swipes) do not survive translation to [`crate::PointerEvent::Axis`]. The
//! capturing Mac therefore copies the gesture/scroll fields of each CGEvent and
//! the receiving Mac recreates the event from them. Only peers that advertise
//! the `macos_trackpad` protocol capability receive these; scroll events fall
//! back to [`crate::PointerEvent::Axis`] for everyone else.

use crate::PointerEvent;
use std::fmt::{self, Display};

/// Largest observed frame (Dock swipe) uses 26 fields.
pub const MAX_TRACKPAD_FIELDS: usize = 32;

/// `kCGEventScrollWheel`.
pub const CG_SCROLL_WHEEL: u8 = 22;
/// `NSEventTypeGesture`: pinch, rotate, smart zoom, ... and frame terminators.
pub const CG_GESTURE: u8 = 29;
/// Dock-control gesture: Spaces, Mission Control, App Exposé, Launchpad.
pub const CG_DOCK_GESTURE: u8 = 30;

/// `kCGScrollWheelEventPointDeltaAxis1` / `Axis2`.
const SCROLL_POINT_DELTA_AXIS_1: u8 = 96;
const SCROLL_POINT_DELTA_AXIS_2: u8 = 97;

/// Fields that carry gesture/scroll payload. Everything else (location,
/// timestamp, window and source-process ids, user data) belongs to the
/// capturing Mac and is regenerated on the receiver.
pub fn is_forwarded_field(cg_type: u8, field: u8) -> bool {
    match cg_type {
        // delta axes, continuous flag, fixed-point/point deltas, scroll phase
        // and count; >= 110 holds momentum phase and newer per-OS extras.
        CG_SCROLL_WHEEL => matches!(field, 11..=13 | 88 | 93..=100 | 110..=200) && field != 169,
        // 169 is a mach timestamp.
        CG_GESTURE | CG_DOCK_GESTURE => matches!(field, 110..=200) && field != 169,
        _ => false,
    }
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum TrackpadValue {
    Integer(i64),
    Double(f64),
}

/// A CGEvent type plus its non-zero forwarded fields.
#[derive(Debug, PartialEq, Clone, Copy)]
pub struct TrackpadEvent {
    cg_type: u8,
    len: u8,
    ids: [u8; MAX_TRACKPAD_FIELDS],
    /// i64 or f64 bit patterns, as selected by `double_mask`.
    bits: [u64; MAX_TRACKPAD_FIELDS],
    double_mask: u32,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct TooManyFields;

impl TrackpadEvent {
    pub fn new(cg_type: u8) -> Self {
        Self {
            cg_type,
            len: 0,
            ids: [0; MAX_TRACKPAD_FIELDS],
            bits: [0; MAX_TRACKPAD_FIELDS],
            double_mask: 0,
        }
    }

    pub fn cg_type(&self) -> u8 {
        self.cg_type
    }

    pub fn push(&mut self, field: u8, value: TrackpadValue) -> Result<(), TooManyFields> {
        let i = self.len as usize;
        if i == MAX_TRACKPAD_FIELDS {
            return Err(TooManyFields);
        }
        self.ids[i] = field;
        self.bits[i] = match value {
            TrackpadValue::Integer(v) => v as u64,
            TrackpadValue::Double(v) => {
                self.double_mask |= 1 << i;
                v.to_bits()
            }
        };
        self.len += 1;
        Ok(())
    }

    pub fn fields(&self) -> impl Iterator<Item = (u8, TrackpadValue)> + '_ {
        (0..self.len as usize).map(|i| {
            let value = if self.double_mask & (1 << i) != 0 {
                TrackpadValue::Double(f64::from_bits(self.bits[i]))
            } else {
                TrackpadValue::Integer(self.bits[i] as i64)
            };
            (self.ids[i], value)
        })
    }

    fn integer(&self, field: u8) -> i64 {
        self.fields()
            .find_map(|(id, v)| match (id == field, v) {
                (true, TrackpadValue::Integer(v)) => Some(v),
                (true, TrackpadValue::Double(v)) => Some(v as i64),
                _ => None,
            })
            .unwrap_or(0)
    }

    /// Pixel scroll for peers without the trackpad capability. Gestures have
    /// no portable equivalent and yield nothing.
    pub fn scroll_fallback(&self) -> impl Iterator<Item = PointerEvent> {
        let (v, h) = if self.cg_type == CG_SCROLL_WHEEL {
            (
                self.integer(SCROLL_POINT_DELTA_AXIS_1),
                self.integer(SCROLL_POINT_DELTA_AXIS_2),
            )
        } else {
            (0, 0)
        };
        [(0u8, v), (1u8, h)]
            .into_iter()
            .filter(|(_, d)| *d != 0)
            .map(|(axis, d)| PointerEvent::Axis {
                time: 0,
                axis,
                value: d as f64,
            })
    }
}

impl Display for TrackpadEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "trackpad(type {}, {} fields)", self.cg_type, self.len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_round_trip_with_their_kind() {
        let mut e = TrackpadEvent::new(CG_GESTURE);
        e.push(110, TrackpadValue::Integer(8)).unwrap();
        e.push(113, TrackpadValue::Double(0.024)).unwrap();
        e.push(132, TrackpadValue::Integer(-1)).unwrap();
        let fields: Vec<_> = e.fields().collect();
        assert_eq!(
            fields,
            [
                (110, TrackpadValue::Integer(8)),
                (113, TrackpadValue::Double(0.024)),
                (132, TrackpadValue::Integer(-1)),
            ]
        );
    }

    #[test]
    fn push_refuses_more_than_capacity() {
        let mut e = TrackpadEvent::new(CG_DOCK_GESTURE);
        for i in 0..MAX_TRACKPAD_FIELDS {
            e.push(110 + i as u8, TrackpadValue::Integer(1)).unwrap();
        }
        assert_eq!(e.push(200, TrackpadValue::Integer(1)), Err(TooManyFields));
    }

    #[test]
    fn scroll_falls_back_to_pixel_axes_and_gestures_to_nothing() {
        let mut scroll = TrackpadEvent::new(CG_SCROLL_WHEEL);
        scroll.push(96, TrackpadValue::Integer(-7)).unwrap();
        scroll.push(97, TrackpadValue::Integer(2)).unwrap();
        scroll.push(99, TrackpadValue::Integer(2)).unwrap();
        let axes: Vec<_> = scroll.scroll_fallback().collect();
        assert_eq!(
            axes,
            [
                PointerEvent::Axis {
                    time: 0,
                    axis: 0,
                    value: -7.0
                },
                PointerEvent::Axis {
                    time: 0,
                    axis: 1,
                    value: 2.0
                },
            ]
        );

        // momentum-end frame: phase only, no delta
        let mut end = TrackpadEvent::new(CG_SCROLL_WHEEL);
        end.push(123, TrackpadValue::Integer(3)).unwrap();
        assert_eq!(end.scroll_fallback().count(), 0);

        let mut pinch = TrackpadEvent::new(CG_GESTURE);
        pinch.push(96, TrackpadValue::Integer(5)).unwrap();
        assert_eq!(pinch.scroll_fallback().count(), 0);
    }

    #[test]
    fn only_payload_fields_are_forwarded() {
        for timestamp_or_window in [39, 40, 41, 42, 45, 50, 55, 101, 169] {
            assert!(!is_forwarded_field(CG_GESTURE, timestamp_or_window));
            assert!(!is_forwarded_field(CG_SCROLL_WHEEL, timestamp_or_window));
        }
        assert!(is_forwarded_field(CG_SCROLL_WHEEL, 99)); // scroll phase
        assert!(is_forwarded_field(CG_SCROLL_WHEEL, 123)); // momentum phase
        assert!(is_forwarded_field(CG_GESTURE, 113)); // magnification
        assert!(is_forwarded_field(CG_DOCK_GESTURE, 124)); // swipe progress
        assert!(!is_forwarded_field(5, 113)); // mouse move
    }
}
