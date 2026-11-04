//! Posts a zero-magnification pinch and a zero-delta phased scroll through the
//! macOS emulation backend, so a session-level event tap can confirm macOS
//! accepts recreated gesture events (type 29 with subtype 8 and phase intact).
//! Nothing zooms or scrolls.
//!
//!   cargo run -p input-emulation --example macos_trackpad_replay_check
//!
//! Needs Accessibility permission for the terminal. Watch with any listen-only
//! session tap on gesture types 29 / 30 and scroll type 22. Pause sharing first:
//! while this Mac is controlling the other one, the daemon's HID tap captures
//! (and forwards) synthetic scroll events before a session tap can see them.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(tokio::task::LocalSet::new().run_until(run()));
}

#[cfg(target_os = "macos")]
async fn run() {
    use input_emulation::{Backend, InputEmulation};
    use input_event::{
        Event, TrackpadEvent,
        trackpad::{CG_GESTURE, CG_SCROLL_WHEEL, TrackpadValue::*},
    };
    use std::time::Duration;

    let mut emulation = InputEmulation::new(Some(Backend::MacOs))
        .await
        .expect("macOS emulation (Accessibility permission?)");
    emulation.create(0).await;

    // IOHIDEventPhaseBits: began 1, changed 2, ended 4.
    for phase in [1, 2, 2, 4] {
        let mut pinch = TrackpadEvent::new(CG_GESTURE);
        pinch.push(110, Integer(8)).unwrap(); // kIOHIDEventTypeZoom
        pinch.push(132, Integer(phase)).unwrap();
        pinch.push(113, Double(0.0)).unwrap();
        // every gesture frame is followed by a bare type-29 terminator
        let terminator = TrackpadEvent::new(CG_GESTURE);
        for e in [pinch, terminator] {
            emulation.consume(Event::Trackpad(e), 0).await.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(16)).await;
    }
    println!("posted pinch: 4 frames (subtype 8, phases 1,2,2,4)");

    // kCGScrollWheelEventScrollPhase: began 1, changed 2, ended 4.
    for phase in [1, 2, 4] {
        let mut scroll = TrackpadEvent::new(CG_SCROLL_WHEEL);
        scroll.push(88, Integer(1)).unwrap(); // continuous
        scroll.push(99, Integer(phase)).unwrap();
        emulation.consume(Event::Trackpad(scroll), 0).await.unwrap();
        tokio::time::sleep(Duration::from_millis(16)).await;
    }
    println!("posted scroll: 3 frames (phases 1,2,4)");
    emulation.terminate().await;
}
