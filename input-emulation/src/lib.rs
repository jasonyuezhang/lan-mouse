use async_trait::async_trait;
use std::{
    collections::{HashMap, HashSet},
    fmt::Display,
};

use input_event::{Event, KeyboardEvent};

pub use self::error::{EmulationCreationError, EmulationError, InputEmulationError};

/// Edge used to place the cursor when a peer enters this device.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WarpPosition {
    Left,
    Right,
    Top,
    Bottom,
}

#[cfg(windows)]
mod windows;

#[cfg(x11)]
mod x11;

#[cfg(wlroots)]
mod wlroots;

#[cfg(rdp)]
mod xdg_desktop_portal;

#[cfg(libei)]
mod libei;

#[cfg(target_os = "macos")]
mod macos;

/// fallback input emulation (logs events)
mod dummy;
mod error;

pub type EmulationHandle = u64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Backend {
    #[cfg(wlroots)]
    Wlroots,
    #[cfg(libei)]
    Libei,
    #[cfg(rdp)]
    Xdp,
    #[cfg(x11)]
    X11,
    #[cfg(windows)]
    Windows,
    #[cfg(target_os = "macos")]
    MacOs,
    Dummy,
}

impl Display for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            #[cfg(wlroots)]
            Backend::Wlroots => write!(f, "wlroots"),
            #[cfg(libei)]
            Backend::Libei => write!(f, "libei"),
            #[cfg(rdp)]
            Backend::Xdp => write!(f, "xdg-desktop-portal"),
            #[cfg(x11)]
            Backend::X11 => write!(f, "X11"),
            #[cfg(windows)]
            Backend::Windows => write!(f, "windows"),
            #[cfg(target_os = "macos")]
            Backend::MacOs => write!(f, "macos"),
            Backend::Dummy => write!(f, "dummy"),
        }
    }
}

pub struct InputEmulation {
    emulation: Box<dyn Emulation>,
    handles: HashSet<EmulationHandle>,
    pressed_keys: HashMap<EmulationHandle, HashMap<u32, u8>>,
}

impl InputEmulation {
    async fn with_backend(backend: Backend) -> Result<InputEmulation, EmulationCreationError> {
        let emulation: Box<dyn Emulation> = match backend {
            #[cfg(wlroots)]
            Backend::Wlroots => Box::new(wlroots::WlrootsEmulation::new()?),
            #[cfg(libei)]
            Backend::Libei => Box::new(libei::LibeiEmulation::new().await?),
            #[cfg(x11)]
            Backend::X11 => Box::new(x11::X11Emulation::new()?),
            #[cfg(rdp)]
            Backend::Xdp => Box::new(xdg_desktop_portal::DesktopPortalEmulation::new().await?),
            #[cfg(windows)]
            Backend::Windows => Box::new(windows::WindowsEmulation::new()?),
            #[cfg(target_os = "macos")]
            Backend::MacOs => Box::new(macos::MacOSEmulation::new()?),
            Backend::Dummy => Box::new(dummy::DummyEmulation::new()),
        };
        Ok(Self {
            emulation,
            handles: HashSet::new(),
            pressed_keys: HashMap::new(),
        })
    }

    pub async fn new(backend: Option<Backend>) -> Result<InputEmulation, EmulationCreationError> {
        if let Some(backend) = backend {
            let b = Self::with_backend(backend).await;
            if b.is_ok() {
                log::info!("using emulation backend: {backend}");
            }
            return b;
        }

        // No dummy fallback here (matching input-capture): the dummy only
        // logs events, but a daemon running it still answers pings and
        // acknowledges Enter, so peers hand over their input to a black hole
        // and are frozen until they find the release bind. Missing
        // permissions must surface as "emulation disabled", which peers
        // check before sending. The dummy stays available explicitly via
        // `emulation_backend = "dummy"`.
        for backend in [
            #[cfg(wlroots)]
            Backend::Wlroots,
            #[cfg(libei)]
            Backend::Libei,
            #[cfg(rdp)]
            Backend::Xdp,
            #[cfg(x11)]
            Backend::X11,
            #[cfg(windows)]
            Backend::Windows,
            #[cfg(target_os = "macos")]
            Backend::MacOs,
        ] {
            match Self::with_backend(backend).await {
                Ok(b) => {
                    log::info!("using emulation backend: {backend}");
                    return Ok(b);
                }
                Err(e) if e.cancelled_by_user() => return Err(e),
                Err(e) => log::warn!("{e}"),
            }
        }

        Err(EmulationCreationError::NoAvailableBackend)
    }

    pub async fn consume(
        &mut self,
        event: Event,
        handle: EmulationHandle,
    ) -> Result<(), EmulationError> {
        match event {
            Event::Keyboard(KeyboardEvent::Key { key, state, .. }) => {
                // prevent double pressed / released keys
                if self.update_pressed_keys(handle, key, state) {
                    self.emulation.consume(event, handle).await?;
                }
                Ok(())
            }
            _ => self.emulation.consume(event, handle).await,
        }
    }

    pub async fn create(&mut self, handle: EmulationHandle) -> bool {
        if self.handles.insert(handle) {
            self.pressed_keys.insert(handle, HashMap::new());
            self.emulation.create(handle).await;
            true
        } else {
            false
        }
    }

    pub async fn destroy(&mut self, handle: EmulationHandle) {
        let _ = self.release_keys(handle).await;
        if self.handles.remove(&handle) {
            self.pressed_keys.remove(&handle);
            self.emulation.destroy(handle).await
        }
    }

    pub async fn terminate(&mut self) {
        for handle in self.handles.iter().cloned().collect::<Vec<_>>() {
            self.destroy(handle).await
        }
        self.emulation.terminate().await
    }

    pub async fn warp_cursor(
        &mut self,
        handle: EmulationHandle,
        pos: WarpPosition,
        cross_axis: f32,
    ) -> Result<(), EmulationError> {
        self.emulation.warp_cursor(handle, pos, cross_axis).await
    }

    pub async fn release_keys(&mut self, handle: EmulationHandle) -> Result<(), EmulationError> {
        if let Some(keys) = self.pressed_keys.get_mut(&handle) {
            let keys = keys.drain().map(|(key, _)| key).collect::<Vec<_>>();
            for key in keys {
                let event = Event::Keyboard(KeyboardEvent::Key {
                    time: 0,
                    key,
                    state: 0,
                });
                self.emulation.consume(event, handle).await?;
                if let Ok(key) = input_event::scancode::Linux::try_from(key) {
                    log::warn!("releasing stuck key: {key:?}");
                }
            }
        }

        let event = Event::Keyboard(KeyboardEvent::Modifiers {
            depressed: 0,
            latched: 0,
            locked: 0,
            group: 0,
        });
        self.emulation.consume(event, handle).await?;
        Ok(())
    }

    pub fn has_pressed_keys(&self, handle: EmulationHandle) -> bool {
        self.pressed_keys
            .get(&handle)
            .is_some_and(|p| !p.is_empty())
    }

    /// update the pressed_keys for the given handle
    /// returns whether the event should be processed
    fn update_pressed_keys(&mut self, handle: EmulationHandle, key: u32, state: u8) -> bool {
        let Some(pressed_keys) = self.pressed_keys.get_mut(&handle) else {
            return false;
        };

        match state {
            0 => pressed_keys.remove(&key).is_some(),
            1 | input_event::KEY_PRESSED_NO_REPEAT => {
                if let std::collections::hash_map::Entry::Vacant(entry) = pressed_keys.entry(key) {
                    entry.insert(state);
                    true
                } else {
                    false
                }
            }
            // A capability reply can arrive halfway through a legacy hold.
            // Only explicit source-timed presses may accept source repeats.
            input_event::KEY_REPEATED => {
                pressed_keys.get(&key) == Some(&input_event::KEY_PRESSED_NO_REPEAT)
            }
            _ => false,
        }
    }
}

#[async_trait]
trait Emulation: Send {
    async fn consume(
        &mut self,
        event: Event,
        handle: EmulationHandle,
    ) -> Result<(), EmulationError>;
    async fn create(&mut self, handle: EmulationHandle);
    async fn destroy(&mut self, handle: EmulationHandle);
    async fn terminate(&mut self);
    async fn warp_cursor(
        &mut self,
        _handle: EmulationHandle,
        _pos: WarpPosition,
        _cross_axis: f32,
    ) -> Result<(), EmulationError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use input_event::{KEY_PRESSED_NO_REPEAT, KEY_REPEATED};
    use std::sync::{Arc, Mutex};

    struct RecordingEmulation(Arc<Mutex<Vec<Event>>>);
    #[async_trait]
    impl Emulation for RecordingEmulation {
        async fn consume(
            &mut self,
            event: Event,
            _: EmulationHandle,
        ) -> Result<(), EmulationError> {
            self.0.lock().unwrap().push(event);
            Ok(())
        }
        async fn create(&mut self, _: EmulationHandle) {}
        async fn destroy(&mut self, _: EmulationHandle) {}
        async fn terminate(&mut self) {}
    }

    fn key(key: u32, state: u8) -> Event {
        Event::Keyboard(KeyboardEvent::Key {
            time: 0,
            key,
            state,
        })
    }
    async fn emulation() -> (InputEmulation, Arc<Mutex<Vec<Event>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut emulation = InputEmulation {
            emulation: Box::new(RecordingEmulation(events.clone())),
            handles: HashSet::new(),
            pressed_keys: HashMap::new(),
        };
        emulation.create(0).await;
        (emulation, events)
    }

    #[tokio::test]
    async fn repeats_require_a_source_press_and_stop_after_release() {
        let (mut e, events) = emulation().await;
        for state in [
            KEY_REPEATED,
            KEY_PRESSED_NO_REPEAT,
            KEY_PRESSED_NO_REPEAT,
            KEY_REPEATED,
            0,
            KEY_REPEATED,
            0,
        ] {
            e.consume(key(30, state), 0).await.unwrap();
        }
        assert_eq!(
            *events.lock().unwrap(),
            vec![
                key(30, KEY_PRESSED_NO_REPEAT),
                key(30, KEY_REPEATED),
                key(30, 0)
            ]
        );
        assert!(!e.has_pressed_keys(0));
    }

    #[tokio::test]
    async fn negotiation_during_a_legacy_hold_does_not_double_repeat() {
        let (mut e, events) = emulation().await;
        for state in [1, KEY_PRESSED_NO_REPEAT, KEY_REPEATED, 0] {
            e.consume(key(30, state), 0).await.unwrap();
        }
        assert_eq!(*events.lock().unwrap(), vec![key(30, 1), key(30, 0)]);
    }

    #[tokio::test]
    async fn terminate_releases_source_keys_and_modifiers() {
        let (mut e, events) = emulation().await;
        e.consume(key(30, KEY_PRESSED_NO_REPEAT), 0).await.unwrap();
        e.consume(key(42, KEY_PRESSED_NO_REPEAT), 0).await.unwrap();
        e.terminate().await;
        assert!(!e.has_pressed_keys(0));
        let events = events.lock().unwrap();
        assert!(events.contains(&key(30, 0)));
        assert!(events.contains(&key(42, 0)));
        assert_eq!(
            events.last(),
            Some(&Event::Keyboard(KeyboardEvent::Modifiers {
                depressed: 0,
                latched: 0,
                locked: 0,
                group: 0,
            }))
        );
    }
}
