use std::time::{Duration, Instant};

use input_event::scancode::Linux;
use keycode::{KeyMap, KeyMapping};
use lan_mouse_ipc::SharingShortcut;

pub(crate) struct CaptureShortcut {
    pub(crate) shortcut: SharingShortcut,
    key: u32,
    expires: Instant,
}

impl CaptureShortcut {
    pub(crate) fn new(shortcut: SharingShortcut) -> Option<Self> {
        let allowed = (1 << 17) | (1 << 18) | (1 << 19) | (1 << 20);
        if shortcut.key_code >= 128
            || (54..=63).contains(&shortcut.key_code)
            || shortcut.modifiers & !allowed != 0
            || shortcut.modifiers & ((1 << 18) | (1 << 19) | (1 << 20)) == 0
            || shortcut.modifiers == allowed
        {
            return None;
        }
        let key = KeyMap::from_key_mapping(KeyMapping::Mac(shortcut.key_code as u16))
            .ok()?
            .evdev as u32;
        Some(Self {
            shortcut,
            key,
            // The frontend renews while connected. Quitting it must not leave
            // an invisible shortcut swallowing input indefinitely.
            expires: Instant::now() + Duration::from_secs(10),
        })
    }

    pub(crate) fn matches(
        &self,
        key: u32,
        state: u8,
        now: Instant,
        pressed: impl Fn(Linux) -> bool,
    ) -> bool {
        if state != 1 || key != self.key || now >= self.expires {
            return false;
        }
        [
            (17, Linux::KeyLeftShift, Linux::KeyRightShift),
            (18, Linux::KeyLeftCtrl, Linux::KeyRightCtrl),
            (19, Linux::KeyLeftAlt, Linux::KeyRightalt),
            (20, Linux::KeyLeftMeta, Linux::KeyRightmeta),
        ]
        .into_iter()
        .all(|(bit, left, right)| {
            (pressed(left) || pressed(right)) == (self.shortcut.modifiers & (1 << bit) != 0)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captured_shortcut_matches_either_side_but_not_extra_modifiers() {
        let shortcut = CaptureShortcut::new(SharingShortcut {
            key_code: 40, // K
            modifiers: (1 << 18) | (1 << 19),
        })
        .unwrap();
        let now = Instant::now();
        for held in [
            vec![Linux::KeyLeftCtrl, Linux::KeyLeftAlt],
            vec![Linux::KeyRightCtrl, Linux::KeyRightalt],
            vec![Linux::KeyLeftCtrl, Linux::KeyRightalt],
        ] {
            assert!(shortcut.matches(Linux::KeyK as u32, 1, now, |key| held.contains(&key)));
            assert!(!shortcut.matches(Linux::KeyK as u32, 0, now, |key| held.contains(&key)));
            assert!(!shortcut.matches(Linux::KeyK as u32, 2, now, |key| held.contains(&key)));
            assert!(!shortcut.matches(Linux::KeyJ as u32, 1, now, |key| held.contains(&key)));
            assert!(
                !shortcut.matches(Linux::KeyK as u32, 1, shortcut.expires, |key| held
                    .contains(&key))
            );
        }
        assert!(!shortcut.matches(Linux::KeyK as u32, 1, now, |key| {
            [Linux::KeyLeftCtrl, Linux::KeyLeftAlt, Linux::KeyLeftShift].contains(&key)
        }));
        assert!(!shortcut.matches(Linux::KeyK as u32, 1, now, |key| key == Linux::KeyLeftCtrl));
    }

    #[test]
    fn invalid_and_emergency_chords_are_not_registered() {
        for (key_code, modifiers) in [
            (128, 1 << 18),
            (59, 1 << 18),
            (40, 0),
            (40, 1 << 17),
            (40, 0x1e0000),
            (40, 1 << 16),
        ] {
            assert!(
                CaptureShortcut::new(SharingShortcut {
                    key_code,
                    modifiers
                })
                .is_none()
            );
        }
    }

    #[test]
    fn native_frontend_wire_format() {
        let request: lan_mouse_ipc::FrontendRequest =
            serde_json::from_str(r#"{"SetSharingShortcut":{"key_code":40,"modifiers":786432}}"#)
                .unwrap();
        assert_eq!(
            request,
            lan_mouse_ipc::FrontendRequest::SetSharingShortcut(Some(SharingShortcut {
                key_code: 40,
                modifiers: 786432
            }))
        );
        assert_eq!(
            serde_json::to_value(lan_mouse_ipc::FrontendEvent::SharingShortcutPressed {
                key_code: 40,
                modifiers: 786432
            })
            .unwrap(),
            serde_json::json!({"SharingShortcutPressed":{"key_code":40,"modifiers":786432}})
        );
        assert_eq!(
            serde_json::from_str::<lan_mouse_ipc::FrontendRequest>(
                r#"{"SetSharingShortcut":null}"#
            )
            .unwrap(),
            lan_mouse_ipc::FrontendRequest::SetSharingShortcut(None)
        );
    }
}
