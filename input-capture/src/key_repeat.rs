//! Generate repeats where physical key releases are observed, before network delay.
use crate::{CaptureEvent, Position};
use input_event::{Event, KEY_REPEATED, KeyboardEvent, scancode::Linux};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::time::{Instant, Sleep};

pub(crate) struct KeyRepeat {
    key: Option<(Position, u32)>,
    timer: Pin<Box<Sleep>>,
    delay: Duration,
    interval: Duration,
}

impl KeyRepeat {
    pub(crate) fn new(delay: Duration, interval: Duration) -> Self {
        Self {
            key: None,
            timer: Box::pin(tokio::time::sleep(delay)),
            delay,
            interval,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.key = None;
    }

    pub(crate) fn observe(&mut self, pos: Position, event: CaptureEvent) {
        match event {
            CaptureEvent::Begin { .. } => self.clear(),
            CaptureEvent::Input(Event::Keyboard(KeyboardEvent::Key { key, state: 1, .. }))
                if repeatable(key) =>
            {
                // Ignore duplicate/native auto-repeat downs: we own the timer.
                if self.key != Some((pos, key)) {
                    self.key = Some((pos, key));
                    self.timer.as_mut().reset(Instant::now() + self.delay);
                }
            }
            CaptureEvent::Input(Event::Keyboard(KeyboardEvent::Key { key, state: 0, .. }))
                if self.key == Some((pos, key)) =>
            {
                self.clear()
            }
            _ => {}
        }
    }

    pub(crate) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<(Position, CaptureEvent)> {
        let Some((pos, key)) = self.key else {
            return Poll::Pending;
        };
        if self.timer.as_mut().poll(cx).is_pending() {
            return Poll::Pending;
        }
        // Skip missed ticks; never replay a burst after the sender was stalled.
        self.timer.as_mut().reset(Instant::now() + self.interval);
        Poll::Ready((
            pos,
            CaptureEvent::Input(Event::Keyboard(KeyboardEvent::Key {
                time: 0,
                key,
                state: KEY_REPEATED,
            })),
        ))
    }
}

fn repeatable(key: u32) -> bool {
    !matches!(
        Linux::try_from(key),
        Ok(Linux::KeyLeftShift
            | Linux::KeyRightShift
            | Linux::KeyLeftCtrl
            | Linux::KeyRightCtrl
            | Linux::KeyLeftAlt
            | Linux::KeyRightalt
            | Linux::KeyLeftMeta
            | Linux::KeyRightmeta
            | Linux::KeyCapsLock
            | Linux::KeyNumlock
            | Linux::KeyScrollLock)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{FutureExt, future::poll_fn};

    fn event(key: u32, state: u8) -> CaptureEvent {
        CaptureEvent::Input(Event::Keyboard(KeyboardEvent::Key {
            time: 0,
            key,
            state,
        }))
    }
    fn repeat() -> KeyRepeat {
        KeyRepeat::new(Duration::from_millis(250), Duration::from_millis(33))
    }

    #[tokio::test(start_paused = true)]
    async fn tap_never_repeats_even_if_release_delivery_to_peer_is_delayed() {
        let mut r = repeat();
        r.observe(Position::Left, event(30, 1));
        tokio::time::advance(Duration::from_millis(50)).await;
        r.observe(Position::Left, event(30, 0));
        tokio::time::advance(Duration::from_secs(2)).await;
        assert!(poll_fn(|cx| r.poll(cx)).now_or_never().is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn hold_repeats_without_catching_up_after_a_stall() {
        let mut r = repeat();
        r.observe(Position::Left, event(30, 1));
        assert!(poll_fn(|cx| r.poll(cx)).now_or_never().is_none());
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(poll_fn(|cx| r.poll(cx)).now_or_never().is_some());
        assert!(poll_fn(|cx| r.poll(cx)).now_or_never().is_none());
        tokio::time::advance(Duration::from_millis(33)).await;
        assert!(poll_fn(|cx| r.poll(cx)).now_or_never().is_some());
        r.clear();
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(poll_fn(|cx| r.poll(cx)).now_or_never().is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn releasing_another_key_or_modifier_does_not_stop_the_held_key() {
        let mut r = repeat();
        r.observe(Position::Left, event(30, 1));
        r.observe(Position::Left, event(31, 1));
        r.observe(Position::Left, event(30, 0));
        r.observe(Position::Left, event(Linux::KeyLeftShift as u32, 1));
        let (_, CaptureEvent::Input(Event::Keyboard(KeyboardEvent::Key { key, state, .. }))) =
            poll_fn(|cx| r.poll(cx)).await
        else {
            panic!("expected repeat")
        };
        assert_eq!((key, state), (31, KEY_REPEATED));
    }
}
