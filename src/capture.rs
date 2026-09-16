use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use futures::StreamExt;
use input_capture::{
    CaptureError, CaptureEvent, CaptureHandle, InputCapture, InputCaptureError, Position,
};
use input_event::{Event, KeyboardEvent, PointerEvent, scancode};
use lan_mouse_proto::ProtoEvent;
use local_channel::mpsc::{Receiver, Sender, channel};
use tokio::task::{JoinHandle, spawn_local};
use tokio_util::sync::CancellationToken;

use crate::connect::LanMouseConnection;

pub(crate) struct Capture {
    cancellation_token: CancellationToken,
    request_tx: Sender<CaptureRequest>,
    task: JoinHandle<()>,
    event_rx: Receiver<ICaptureEvent>,
}

pub(crate) enum ICaptureEvent {
    /// a client was entered
    CaptureBegin(CaptureHandle),
    /// capture disabled
    CaptureDisabled,
    /// capture disabled
    CaptureEnabled,
    /// A (new) client was entered.
    /// In contrast to [`ICaptureEvent::CaptureBegin`] this
    /// event is only triggered when the capture was
    /// explicitly released in the meantime by
    /// either the remote client leaving its device region,
    /// a new device entering the screen or the release bind.
    ClientEntered(u64),
    /// The capture was released while this client was active:
    /// the cursor is back on this device.
    ClientLeft(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaptureType {
    /// a normal input capture
    Default,
    /// A capture only interested in [`CaptureEvent::Begin`] events.
    /// The capture is released immediately, if there is no
    /// Default capture at the same position.
    EnterOnly,
}

#[derive(Clone, Debug)]
enum CaptureRequest {
    /// capture must release the mouse
    Release,
    /// add a capture client (with its key remapping)
    Create(CaptureHandle, Position, CaptureType, KeyMap),
    /// destory a capture client
    Destroy(CaptureHandle),
    /// reenable input capture
    Reenable,
    /// set release bind
    SetReleaseBind(Vec<scancode::Linux>),
}

impl Capture {
    pub(crate) fn new(
        backend: Option<input_capture::Backend>,
        conn: LanMouseConnection,
        release_bind: Vec<scancode::Linux>,
    ) -> Self {
        let (request_tx, request_rx) = channel();
        let (event_tx, event_rx) = channel();
        let cancellation_token = CancellationToken::new();
        let capture_task = CaptureTask {
            active_client: None,
            backend,
            cancellation_token: cancellation_token.clone(),
            captures: Default::default(),
            conn,
            event_tx,
            request_rx,
            release_bind: Rc::new(RefCell::new(release_bind)),
            state: Default::default(),
            transition_epoch: transition_epoch(),
            next_transition_serial: 1,
            ack_deadline: None,
        };
        let task = spawn_local(capture_task.run());
        Self {
            cancellation_token,
            request_tx,
            task,
            event_rx,
        }
    }

    pub(crate) fn reenable(&self) {
        self.request_tx
            .send(CaptureRequest::Reenable)
            .expect("channel closed");
    }

    pub(crate) async fn terminate(&mut self) {
        self.cancellation_token.cancel();
        log::debug!("terminating capture");
        if let Err(e) = (&mut self.task).await {
            log::warn!("{e}");
        }
    }

    pub(crate) fn create(
        &self,
        handle: CaptureHandle,
        pos: lan_mouse_ipc::Position,
        capture_type: CaptureType,
        key_map: KeyMap,
    ) {
        let pos = to_capture_pos(pos);
        self.request_tx
            .send(CaptureRequest::Create(handle, pos, capture_type, key_map))
            .expect("channel closed");
    }

    pub(crate) fn destroy(&self, handle: CaptureHandle) {
        self.request_tx
            .send(CaptureRequest::Destroy(handle))
            .expect("channel closed");
    }

    pub(crate) fn release(&self) {
        self.request_tx
            .send(CaptureRequest::Release)
            .expect("channel closed");
    }

    pub(crate) async fn event(&mut self) -> ICaptureEvent {
        self.event_rx.recv().await.expect("channel closed")
    }

    pub(crate) fn set_release_bind(&mut self, bind: Vec<scancode::Linux>) {
        let _ = self.request_tx.send(CaptureRequest::SetReleaseBind(bind));
    }
}

/// debounce a statement `$st`, i.e. the statement is executed only if the
/// time since the previous execution is at least `$dur`.
/// `$prev` is used to keep track of this timestamp
macro_rules! debounce {
    ($prev:ident, $dur:expr, $st:stmt) => {
        let exec = match $prev.get() {
            None => true,
            Some(instant) if instant.elapsed() > $dur => true,
            _ => false,
        };
        if exec {
            $prev.replace(Some(Instant::now()));
            $st
        }
    };
}

/// evdev keycode -> evdev keycode
pub(crate) type KeyMap = HashMap<u32, u32>;

struct CaptureTask {
    active_client: Option<CaptureHandle>,
    backend: Option<input_capture::Backend>,
    cancellation_token: CancellationToken,
    captures: Vec<(CaptureHandle, Position, CaptureType, KeyMap)>,
    conn: LanMouseConnection,
    event_tx: Sender<ICaptureEvent>,
    release_bind: Rc<RefCell<Vec<scancode::Linux>>>,
    request_rx: Receiver<CaptureRequest>,
    state: State,
    transition_epoch: u64,
    next_transition_serial: u32,
    /// when the pending `Enter` must have been acknowledged, otherwise the
    /// capture is released so an unresponsive peer cannot freeze this desk
    ack_deadline: Option<tokio::time::Instant>,
}

/// How long we hold the capture waiting for the peer to acknowledge an
/// `Enter`. On a LAN an ack takes milliseconds; anything longer means the
/// peer is asleep, offline or its emulation is stuck.
const ACK_TIMEOUT: Duration = Duration::from_millis(1500);

impl CaptureTask {
    fn add_capture(
        &mut self,
        handle: CaptureHandle,
        pos: Position,
        capture_type: CaptureType,
        key_map: KeyMap,
    ) {
        self.captures.push((handle, pos, capture_type, key_map));
    }

    fn remove_capture(&mut self, handle: CaptureHandle) {
        self.captures.retain(|&(h, ..)| handle != h);
    }

    fn is_default_capture_at(&self, pos: Position) -> bool {
        self.captures
            .iter()
            .any(|(_, p, t, _)| *p == pos && *t == CaptureType::Default)
    }

    /// apply the client's key remapping to an outgoing input event
    fn remap(&self, handle: CaptureHandle, event: Event) -> Event {
        let Some((_, _, _, map)) = self.captures.iter().find(|(h, ..)| *h == handle) else {
            return event;
        };
        if map.is_empty() {
            return event;
        }
        match event {
            Event::Keyboard(KeyboardEvent::Key { time, key, state }) => {
                Event::Keyboard(KeyboardEvent::Key {
                    time,
                    key: *map.get(&key).unwrap_or(&key),
                    state,
                })
            }
            Event::Pointer(PointerEvent::Button {
                time,
                button,
                state,
            }) => Event::Pointer(PointerEvent::Button {
                time,
                button: *map.get(&button).unwrap_or(&button),
                state,
            }),
            // macOS emulation derives CGEvent flags from this mask, so a
            // remapped modifier key must move its bit too
            Event::Keyboard(KeyboardEvent::Modifiers {
                depressed,
                latched,
                locked,
                group,
            }) => Event::Keyboard(KeyboardEvent::Modifiers {
                depressed: remap_modifier_mask(depressed, map),
                latched,
                locked: remap_modifier_mask(locked, map),
                group,
            }),
            other => other,
        }
    }

    fn get_pos(&self, handle: CaptureHandle) -> Position {
        self.captures
            .iter()
            .find(|(h, ..)| *h == handle)
            .expect("no such capture")
            .1
    }

    fn get_type(&self, handle: CaptureHandle) -> CaptureType {
        self.captures
            .iter()
            .find(|(h, ..)| *h == handle)
            .expect("no such capture")
            .2
    }

    async fn run(mut self) {
        loop {
            if let Err(e) = self.do_capture().await {
                log::warn!("input capture exited: {e}");
            }
            loop {
                tokio::select! {
                    r = self.request_rx.recv() => match r.expect("channel closed") {
                        CaptureRequest::Reenable => break,
                        CaptureRequest::Create(h, p, t, m) => self.add_capture(h, p, t, m),
                        CaptureRequest::Destroy(h) => self.remove_capture(h),
                        CaptureRequest::Release => { /* nothing to do */ }
                        CaptureRequest::SetReleaseBind(bind) => {
                            self.release_bind.borrow_mut().clone_from(&bind);
                        }
                    },
                    _ = self.cancellation_token.cancelled() => return,
                }
            }
        }
    }

    async fn do_capture(&mut self) -> Result<(), InputCaptureError> {
        /* allow cancelling capture request */
        let mut capture = tokio::select! {
            r = InputCapture::new(self.backend) => r?,
            _ = self.cancellation_token.cancelled() => return Ok(()),
        };

        let _capture_guard = DropGuard::new(
            self.event_tx.clone(),
            ICaptureEvent::CaptureEnabled,
            ICaptureEvent::CaptureDisabled,
        );

        /* create barriers for active clients */
        let r = self.create_captures(&mut capture).await;
        if let Err(e) = r {
            capture.terminate().await?;
            return Err(e.into());
        }

        let r = self.do_capture_session(&mut capture).await;

        // FIXME replace with async drop when stabilized
        capture.terminate().await?;

        r
    }

    async fn create_captures(&mut self, capture: &mut InputCapture) -> Result<(), CaptureError> {
        let captures = self.captures.clone();
        for (handle, pos, ..) in captures {
            tokio::select! {
                r = capture.create(handle, pos) => r?,
                _ = self.cancellation_token.cancelled() => return Ok(()),
            }
        }
        Ok(())
    }

    async fn do_capture_session(
        &mut self,
        capture: &mut InputCapture,
    ) -> Result<(), InputCaptureError> {
        loop {
            tokio::select! {
                event = capture.next() => match event {
                    Some(event) => self.handle_capture_event(capture, event?).await?,
                    None => return Ok(()),
                },
                (handle, event) = self.conn.recv() => {
                    if let Some(active) = self.active_client {
                        if handle != active {
                            // we only care about events coming from the client we are currently connected to
                            // only `Ack` and `Leave` are relevant
                            continue
                        }
                    }

                    match event {
                        // connection acknowlegded => set state to Sending
                        ProtoEvent::Ack(serial) if self.state.acknowledges(serial) => {
                            log::info!("client {handle} acknowledged the connection!");
                            self.state = State::Sending;
                            self.ack_deadline = None;
                        }
                        // client disconnected
                        ProtoEvent::Leave(_) => {
                            log::info!("releasing capture: left remote client device region");
                            self.release_capture(capture).await?;
                        },
                        _ => {}
                    }
                },
                // peer never acknowledged our Enter: give the desk back
                _ = tokio::time::sleep_until(self.ack_deadline.unwrap_or_else(tokio::time::Instant::now)),
                    if self.ack_deadline.is_some() => {
                    log::warn!(
                        "releasing capture: client {:?} did not acknowledge within {ACK_TIMEOUT:?}",
                        self.active_client
                    );
                    self.release_capture(capture).await?;
                },
                e = self.request_rx.recv() => match e.expect("channel closed") {
                    CaptureRequest::Reenable => { /* already active */ },
                    CaptureRequest::Release => self.release_capture(capture).await?,
                    CaptureRequest::Create(h, p, t, m) => {
                        self.add_capture(h, p, t, m);
                        capture.create(h, p).await?;
                    }
                    CaptureRequest::Destroy(h) => {
                        self.remove_capture(h);
                        capture.destroy(h).await?;
                    }
                    CaptureRequest::SetReleaseBind(bind) => {
                        self.release_bind.borrow_mut().clone_from(&bind);
                    }
                },
                _ = self.cancellation_token.cancelled() => break,
            }
        }
        Ok(())
    }

    async fn handle_capture_event(
        &mut self,
        capture: &mut InputCapture,
        event: (CaptureHandle, CaptureEvent),
    ) -> Result<(), CaptureError> {
        let (handle, event) = event;
        log::trace!("({handle}): {event:?}");

        if capture.keys_pressed(&self.release_bind.borrow()) {
            log::info!("releasing capture: release-bind pressed");
            return self.release_capture(capture).await;
        }

        if matches!(event, CaptureEvent::Begin { .. }) {
            self.event_tx
                .send(ICaptureEvent::CaptureBegin(handle))
                .expect("channel closed");
        }

        // enter only capture (for incoming connections)
        if self.get_type(handle) == CaptureType::EnterOnly {
            // if there is no active outgoing connection at the current capture,
            // we release the capture
            if !self.is_default_capture_at(self.get_pos(handle)) {
                log::info!("releasing capture: no active client at this position");
                capture.release().await?;
            }
            // we dont care about events from incoming handles except for releasing the capture
            return Ok(());
        }

        // activated a new client
        if matches!(event, CaptureEvent::Begin { .. }) && Some(handle) != self.active_client {
            self.active_client.replace(handle);
            self.event_tx
                .send(ICaptureEvent::ClientEntered(handle))
                .expect("channel closed");
        }

        let opposite_pos = to_proto_pos(self.get_pos(handle).opposite());

        let event = match event {
            CaptureEvent::Begin { cross_axis } => {
                let serial = self.next_transition_serial;
                self.next_transition_serial = self.next_transition_serial.wrapping_add(1).max(1);
                let event = if self.conn.supports_enter_with_position(handle) {
                    ProtoEvent::EnterWithPosition {
                        pos: opposite_pos,
                        cross_axis,
                        epoch: self.transition_epoch,
                        serial,
                    }
                } else {
                    ProtoEvent::Enter(opposite_pos)
                };
                let serial = matches!(event, ProtoEvent::EnterWithPosition { .. })
                    .then_some(serial)
                    .unwrap_or(0);
                self.state = State::WaitingForAck { serial, event };
                self.ack_deadline = Some(tokio::time::Instant::now() + ACK_TIMEOUT);
                event
            }
            CaptureEvent::Input(e) => match self.state {
                // connection not acknowledged, repeat `Enter` event
                State::WaitingForAck { event, .. } => event,
                State::Sending => ProtoEvent::Input(self.remap(handle, e)),
            },
        };

        if let Err(e) = self.conn.send(event, handle).await {
            const DUR: Duration = Duration::from_millis(500);
            debounce!(PREV_LOG, DUR, log::warn!("releasing capture: {e}"));
            capture.release().await?;
        }
        Ok(())
    }

    async fn release_capture(&mut self, capture: &mut InputCapture) -> Result<(), CaptureError> {
        self.ack_deadline = None;
        // If we have an active client, notify them we're leaving
        if let Some(handle) = self.active_client.take() {
            // Synthesize key-up events for every key still held in the
            // capture's pressed_keys set BEFORE sending Leave. Without
            // this, pressing the release-bind chord (typically all four
            // modifiers) leaves the peer with phantom held modifiers:
            // the down events were forwarded while capture was active,
            // but the matching up events arrive after the local tap
            // flips to passthrough and never reach the peer. The peer
            // then runs every subsequent keystroke through those held
            // mods until its watchdog times out (1+ s) or our Leave
            // arrives — and Leave can be lost over UDP/DTLS.
            for key in capture.take_pressed_keys() {
                let key_up = ProtoEvent::Input(self.remap(
                    handle,
                    Event::Keyboard(KeyboardEvent::Key {
                        time: 0,
                        key: key as u32,
                        state: 0,
                    }),
                ));
                if let Err(e) = self.conn.send(key_up, handle).await {
                    log::warn!("failed to send key-up to client {handle}: {e}");
                }
            }
            // Reset the modifier mask too. The peer's input-emulation
            // layer keeps a separate XKB-style modifier state that's
            // updated by KeyboardEvent::Modifiers, distinct from the
            // pressed_keys set drained above. Without this, an
            // already-locked CapsLock would survive the release.
            let mods_zero = ProtoEvent::Input(Event::Keyboard(KeyboardEvent::Modifiers {
                depressed: 0,
                latched: 0,
                locked: 0,
                group: 0,
            }));
            if let Err(e) = self.conn.send(mods_zero, handle).await {
                log::warn!("failed to reset modifiers on client {handle}: {e}");
            }

            log::info!("sending Leave event to client {handle}");
            if let Err(e) = self.conn.send(ProtoEvent::Leave(0), handle).await {
                log::warn!("failed to send Leave to client {handle}: {e}");
            }
            self.event_tx
                .send(ICaptureEvent::ClientLeft(handle))
                .expect("channel closed");
        }
        capture.release().await
    }
}

/// XKB-style modifier bit produced by a key, if it is a modifier
/// (bit layout shared by the capture and emulation backends)
fn modifier_bit(key: u32) -> Option<u32> {
    use scancode::Linux::*;
    Some(match scancode::Linux::try_from(key).ok()? {
        KeyLeftShift | KeyRightShift => 1 << 0,
        KeyCapsLock => 1 << 1,
        KeyLeftCtrl | KeyRightCtrl => 1 << 2,
        KeyLeftAlt | KeyRightalt => 1 << 3,
        KeyLeftMeta | KeyRightmeta => 1 << 6,
        _ => return None,
    })
}

/// move modifier bits according to the key map, e.g. Ctrl->Meta moves
/// the ControlMask bit to Mod4Mask
// ponytail: left/right variants share a bit, so mapping only KeyLeftCtrl
// also moves the bit when KeyRightCtrl is held; split masks if that matters
fn remap_modifier_mask(mods: u32, map: &KeyMap) -> u32 {
    let (mut clear, mut set) = (0, 0);
    for (&from, &to) in map {
        let Some(from_bit) = modifier_bit(from) else {
            continue;
        };
        if mods & from_bit == 0 {
            continue;
        }
        clear |= from_bit;
        set |= modifier_bit(to).unwrap_or(0);
    }
    (mods & !clear) | set
}

thread_local! {
    static PREV_LOG: Cell<Option<Instant>> = const { Cell::new(None) };
}

#[derive(Clone, Copy, Debug)]
enum State {
    WaitingForAck { serial: u32, event: ProtoEvent },
    Sending,
}

impl Default for State {
    fn default() -> Self {
        Self::WaitingForAck {
            serial: 0,
            event: ProtoEvent::Enter(lan_mouse_proto::Position::Left),
        }
    }
}

impl State {
    fn acknowledges(self, serial: u32) -> bool {
        matches!(self, Self::WaitingForAck { serial: expected, .. } if serial == expected)
    }
}

fn to_capture_pos(pos: lan_mouse_ipc::Position) -> input_capture::Position {
    match pos {
        lan_mouse_ipc::Position::Left => input_capture::Position::Left,
        lan_mouse_ipc::Position::Right => input_capture::Position::Right,
        lan_mouse_ipc::Position::Top => input_capture::Position::Top,
        lan_mouse_ipc::Position::Bottom => input_capture::Position::Bottom,
    }
}

fn transition_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

fn to_proto_pos(pos: input_capture::Position) -> lan_mouse_proto::Position {
    match pos {
        input_capture::Position::Left => lan_mouse_proto::Position::Left,
        input_capture::Position::Right => lan_mouse_proto::Position::Right,
        input_capture::Position::Top => lan_mouse_proto::Position::Top,
        input_capture::Position::Bottom => lan_mouse_proto::Position::Bottom,
    }
}

struct DropGuard<T> {
    tx: Sender<T>,
    on_drop: Option<T>,
}

impl<T> DropGuard<T> {
    fn new(tx: Sender<T>, on_new: T, on_drop: T) -> Self {
        tx.send(on_new).expect("channel closed");
        let on_drop = Some(on_drop);
        Self { tx, on_drop }
    }
}

impl<T> Drop for DropGuard<T> {
    fn drop(&mut self) {
        self.tx
            .send(self.on_drop.take().expect("item"))
            .expect("channel closed");
    }
}

#[cfg(test)]
mod tests {
    use super::{KeyMap, State, remap_modifier_mask};
    use input_event::scancode::Linux::*;
    use lan_mouse_proto::{Position, ProtoEvent};

    #[test]
    fn swapping_ctrl_and_meta_moves_modifier_bits() {
        let map: KeyMap = [
            (KeyLeftCtrl as u32, KeyLeftMeta as u32),
            (KeyLeftMeta as u32, KeyLeftCtrl as u32),
        ]
        .into_iter()
        .collect();
        const CTRL: u32 = 1 << 2;
        const META: u32 = 1 << 6;
        const SHIFT: u32 = 1 << 0;
        assert_eq!(remap_modifier_mask(CTRL, &map), META);
        assert_eq!(remap_modifier_mask(META, &map), CTRL);
        assert_eq!(remap_modifier_mask(CTRL | META, &map), CTRL | META);
        assert_eq!(remap_modifier_mask(SHIFT | CTRL, &map), SHIFT | META);
        // non-modifier mapping (CapsLock -> Esc) drops the lock bit
        let caps: KeyMap = [(KeyCapsLock as u32, KeyEsc as u32)].into_iter().collect();
        assert_eq!(remap_modifier_mask(1 << 1, &caps), 0);
        assert_eq!(remap_modifier_mask(0, &map), 0);
    }

    #[test]
    fn waiting_for_ack_only_accepts_the_pending_transition() {
        let state = State::WaitingForAck {
            serial: 9,
            event: ProtoEvent::EnterWithPosition {
                pos: Position::Left,
                cross_axis: Some(0.5),
                epoch: 1,
                serial: 9,
            },
        };

        assert!(state.acknowledges(9));
        assert!(!state.acknowledges(8));
    }
}
