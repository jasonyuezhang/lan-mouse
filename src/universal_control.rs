//! Temporarily suspend the competing macOS KVM while the sharing daemon runs.
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    sync::mpsc,
    time::Duration,
};

pub(crate) struct UniversalControl {
    stop: Option<mpsc::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl UniversalControl {
    pub(crate) fn suspend(path: PathBuf) -> Self {
        let (stop, stopped) = mpsc::channel();
        // Preferences and the durable restoration record do not run on the
        // input task. Dropping the owner also restores on an unwinding exit.
        let task = tokio::task::spawn_blocking(move || {
            let mut lease = match PreferenceLease::load(path, SystemPreferences) {
                Ok(lease) => lease,
                Err(e) => {
                    log::warn!("cannot manage Universal Control: {e}");
                    return;
                }
            };
            loop {
                if let Err(e) = lease.suspend() {
                    log::warn!("could not suspend Universal Control: {e}");
                }
                if stopped.recv_timeout(Duration::from_secs(2))
                    != Err(mpsc::RecvTimeoutError::Timeout)
                {
                    break;
                }
            }
            if let Err(e) = lease.restore() {
                log::warn!("could not restore Universal Control; saved preference retained: {e}");
            }
        });
        Self {
            stop: Some(stop),
            task: Some(task),
        }
    }

    pub(crate) async fn restore(mut self) {
        self.stop.take();
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for UniversalControl {
    fn drop(&mut self) {
        self.stop.take();
    }
}

trait Preferences {
    fn read(&self) -> io::Result<Option<bool>>;
    fn write(&mut self, disabled: Option<bool>) -> io::Result<()>;
}

struct SystemPreferences;

unsafe extern "C" {
    fn lan_mouse_universal_control_read() -> i32;
    fn lan_mouse_universal_control_write(disabled: i32) -> i32;
}

impl Preferences for SystemPreferences {
    fn read(&self) -> io::Result<Option<bool>> {
        match unsafe { lan_mouse_universal_control_read() } {
            -1 => Ok(None),
            0 => Ok(Some(false)),
            1 => Ok(Some(true)),
            _ => Err(io::Error::other("cannot read Universal Control preference")),
        }
    }

    fn write(&mut self, disabled: Option<bool>) -> io::Result<()> {
        let value = disabled.map(i32::from).unwrap_or(-1);
        if unsafe { lan_mouse_universal_control_write(value) } == 0 {
            return Err(io::Error::other("cannot save Universal Control preference"));
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct OriginalPreference {
    disable: Option<bool>,
}

struct PreferenceLease<P> {
    path: PathBuf,
    preferences: P,
    original: Option<OriginalPreference>,
}

impl<P: Preferences> PreferenceLease<P> {
    fn load(path: PathBuf, preferences: P) -> io::Result<Self> {
        let original = match fs::read(&path) {
            Ok(data) => Some(serde_json::from_slice(&data)?),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        Ok(Self {
            path,
            preferences,
            original,
        })
    }

    fn suspend(&mut self) -> io::Result<()> {
        let current = self.preferences.read()?;
        if current == Some(true) {
            return Ok(());
        }
        if self.original.is_none() {
            let original = OriginalPreference { disable: current };
            // Save before changing macOS. A restarted daemon must not mistake
            // our temporary Disable=true for the user's original preference.
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&self.path)?;
            file.write_all(&serde_json::to_vec(&original)?)?;
            file.sync_all()?;
            self.original = Some(original);
        }
        self.preferences.write(Some(true))?;
        log::info!("suspended Universal Control while Lan Mouse sharing is enabled");
        Ok(())
    }

    fn restore(&mut self) -> io::Result<()> {
        let Some(original) = &self.original else {
            return Ok(());
        };
        // Preserve a preference the user changed immediately before shutdown.
        if self.preferences.read()? == Some(true) {
            self.preferences.write(original.disable)?;
        }
        fs::remove_file(&self.path)?;
        self.original = None;
        log::info!("restored the previous Universal Control setting");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakePreferences {
        value: Option<bool>,
        fail_write: bool,
    }
    impl Preferences for FakePreferences {
        fn read(&self) -> io::Result<Option<bool>> {
            Ok(self.value)
        }
        fn write(&mut self, value: Option<bool>) -> io::Result<()> {
            if self.fail_write {
                return Err(io::Error::other("write failed"));
            }
            self.value = value;
            Ok(())
        }
    }
    fn lease(value: Option<bool>) -> PreferenceLease<FakePreferences> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "lan-mouse-uc-test-{}-{}.json",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        PreferenceLease::load(
            path,
            FakePreferences {
                value,
                fail_write: false,
            },
        )
        .unwrap()
    }

    #[test]
    fn enabled_and_unset_preferences_are_restored_exactly() {
        for original in [None, Some(false)] {
            let mut lease = lease(original);
            lease.suspend().unwrap();
            assert_eq!(lease.preferences.value, Some(true));
            lease.suspend().unwrap();
            lease.restore().unwrap();
            assert_eq!(lease.preferences.value, original);
            assert!(!lease.path.exists());
        }
    }

    #[test]
    fn already_disabled_is_left_disabled_without_a_restoration_record() {
        let mut lease = lease(Some(true));
        lease.suspend().unwrap();
        lease.restore().unwrap();
        assert_eq!(lease.preferences.value, Some(true));
        assert!(!lease.path.exists());
    }

    #[test]
    fn restart_preserves_the_original_preference_and_rechecks_conflicts() {
        let mut first = lease(None);
        first.suspend().unwrap();
        let mut restarted = PreferenceLease::load(first.path.clone(), first.preferences).unwrap();
        restarted.suspend().unwrap();
        restarted.preferences.value = Some(false);
        restarted.suspend().unwrap();
        assert_eq!(restarted.preferences.value, Some(true));
        restarted.restore().unwrap();
        assert_eq!(restarted.preferences.value, None);
    }

    #[test]
    fn failed_restore_keeps_the_record_for_retry() {
        let mut lease = lease(None);
        lease.suspend().unwrap();
        lease.preferences.fail_write = true;
        assert!(lease.restore().is_err());
        assert!(lease.path.exists());
        lease.preferences.fail_write = false;
        lease.restore().unwrap();
        assert_eq!(lease.preferences.value, None);
    }

    #[test]
    fn shutdown_preserves_a_new_user_preference() {
        let mut lease = lease(None);
        lease.suspend().unwrap();
        lease.preferences.value = Some(false);
        lease.restore().unwrap();
        assert_eq!(lease.preferences.value, Some(false));
    }
}
