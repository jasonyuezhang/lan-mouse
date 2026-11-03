//! Native cursor regression check. Moves the pointer without clicking or typing.
//! Pause sharing first; run a signed binary with Accessibility/Input Monitoring.
#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(tokio::task::LocalSet::new().run_until(native::run()));
}

#[cfg(target_os = "macos")]
mod native {
    use core_graphics::{
        display::CGDisplay,
        event::{CGEvent, CGEventTapLocation, CGEventType, CGMouseButton, EventField},
        event_source::{CGEventSource, CGEventSourceStateID},
    };
    use futures::StreamExt;
    use input_capture::{CaptureEvent, InputCapture, Position};
    use input_emulation::{InputEmulation, WarpPosition};
    use input_event::screen::{Edge, EdgeSegments, Rect};
    use std::time::Duration;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGCursorIsVisible() -> u32;
    }

    pub async fn run() {
        let mut capture = InputCapture::new(Some(input_capture::Backend::MacOs))
            .await
            .unwrap();
        let mut emulation = InputEmulation::new(Some(input_emulation::Backend::MacOs))
            .await
            .unwrap();
        let original =
            CGEvent::new(CGEventSource::new(CGEventSourceStateID::CombinedSessionState).unwrap())
                .unwrap()
                .location();
        capture.create(1, Position::Left).await.unwrap();
        capture.create(2, Position::Left).await.unwrap();
        capture.create(3, Position::Right).await.unwrap();
        capture.create(4, Position::Right).await.unwrap();
        emulation.create(1).await;
        let result = check(&mut capture, &mut emulation).await;
        // Always give the pointer back, including on an assertion failure.
        let released = capture.terminate().await;
        emulation.terminate().await;
        let _ = CGDisplay::warp_mouse_cursor_position(original);
        released.unwrap();
        result.unwrap();
    }

    async fn check(
        capture: &mut InputCapture,
        emulation: &mut InputEmulation,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
            .map_err(|_| "event source")?;
        let rectangles = CGDisplay::active_displays()
            .map_err(|_| "displays")?
            .into_iter()
            .map(|id| {
                let b = CGDisplay::new(id).bounds();
                Rect {
                    x: b.origin.x as i32,
                    y: b.origin.y as i32,
                    width: b.size.width as i32,
                    height: b.size.height as i32,
                }
            })
            .collect::<Vec<_>>();
        for cycle in 0..12 {
            let (edge, position, inward) = if cycle % 2 == 0 {
                (Edge::Left, WarpPosition::Left, 20.0)
            } else {
                (Edge::Right, WarpPosition::Right, -20.0)
            };
            let edge = EdgeSegments::from_rectangles(edge, rectangles.iter().copied());
            let outgoing = if cycle % 2 == 0 { 0.85 } else { 0.15 };
            let returning = 1.0 - outgoing;
            tokio::time::sleep(Duration::from_millis(350)).await;
            emulation.warp_cursor(1, position, outgoing).await?;
            emulation
                .consume(
                    input_event::Event::Pointer(input_event::PointerEvent::Motion {
                        time: 0,
                        dx: inward,
                        dy: 0.0,
                    }),
                    1,
                )
                .await?;
            tokio::time::sleep(Duration::from_millis(100)).await;
            if unsafe { CGCursorIsVisible() } == 0 {
                return Err(
                    "cursor already hidden before capture; stop other sharing instances first"
                        .into(),
                );
            }
            emulation.warp_cursor(1, position, outgoing).await?;
            tokio::time::sleep(Duration::from_millis(30)).await;
            let location = CGEvent::new(source.clone())
                .map_err(|_| "location")?
                .location();
            let event = CGEvent::new_mouse_event(
                source.clone(),
                CGEventType::MouseMoved,
                location,
                CGMouseButton::Left,
            )
            .map_err(|_| "mouse event")?;
            event.set_integer_value_field(
                EventField::MOUSE_EVENT_DELTA_X,
                if inward > 0.0 { -3 } else { 3 },
            );
            event.set_integer_value_field(EventField::MOUSE_EVENT_DELTA_Y, 0);
            event.post(CGEventTapLocation::HID);
            let begin = tokio::time::timeout(Duration::from_secs(2), capture.next())
                .await?
                .ok_or("capture ended")??;
            if !matches!(begin.1, CaptureEvent::Begin { .. }) {
                return Err("expected a new capture".into());
            }
            if unsafe { CGCursorIsVisible() } != 0 {
                return Err("capture did not hide cursor before Begin".into());
            }
            capture.release().await?;
            // No sleep: this is the order used by an incoming positioned handoff.
            emulation.warp_cursor(1, position, returning).await?;
            let actual = CGEvent::new(source.clone())
                .map_err(|_| "location")?
                .location();
            let (x, y) = edge.denormalize(returning).ok_or("empty edge")?;
            if (actual.x - f64::from(x)).abs() > 1.0 || (actual.y - f64::from(y)).abs() > 1.0 {
                return Err(format!("cycle {cycle}: expected ({x}, {y}), got {actual:?}").into());
            }
            emulation
                .consume(
                    input_event::Event::Pointer(input_event::PointerEvent::Motion {
                        time: 0,
                        dx: if inward > 0.0 { -3.0 } else { 3.0 },
                        dy: 0.0,
                    }),
                    1,
                )
                .await?;
            tokio::time::sleep(Duration::from_millis(30)).await;
            // WindowServer may publish visibility on the next display tick.
            emulation
                .consume(
                    input_event::Event::Pointer(input_event::PointerEvent::Motion {
                        time: 0,
                        dx: inward,
                        dy: 0.0,
                    }),
                    1,
                )
                .await?;
            for _ in 0..20 {
                if unsafe { CGCursorIsVisible() } != 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            if unsafe { CGCursorIsVisible() } == 0 {
                return Err("released cursor is still hidden".into());
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
            // A duplicate Begin queued for the second client must not survive release.
            if tokio::time::timeout(Duration::from_millis(30), capture.next())
                .await
                .is_ok()
            {
                return Err("stale capture event survived the return handoff".into());
            }
            println!("cycle {cycle}: visible at the new return position ({x}, {y})");
        }
        Ok(())
    }
}
