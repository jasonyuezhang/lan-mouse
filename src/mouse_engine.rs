//! Explicit input ownership for the locally installed MMF integration helper.
use std::sync::{
    Once, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

static SENDING: AtomicBool = AtomicBool::new(false);
static RECEIVING: AtomicBool = AtomicBool::new(false);
static ENABLED: OnceLock<bool> = OnceLock::new();
static HEARTBEAT: Once = Once::new();
static UPDATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn enabled() -> bool {
    *ENABLED.get_or_init(|| {
        std::env::var_os("HOME").is_some_and(|home| {
            std::path::PathBuf::from(home)
                .join(".config/lan-mouse/mouse-engine.enabled")
                .exists()
        })
    })
}

pub(crate) async fn sending(active: bool) {
    SENDING.store(active, Ordering::SeqCst);
    update().await;
}

pub(crate) async fn receiving(active: bool) {
    RECEIVING.store(active, Ordering::SeqCst);
    update().await;
}

fn mode(sending: bool, receiving: bool) -> i32 {
    if sending {
        1
    } else if receiving {
        2
    } else {
        0
    }
}

async fn update() {
    if !enabled() {
        return;
    }
    HEARTBEAT.call_once(|| {
        tokio::spawn(async {
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                apply().await;
            }
        });
    });
    // Never stall input on the helper's reply: each apply reads the latest
    // ownership under the lock, so the last one to run is always current.
    tokio::spawn(apply());
}

async fn apply() {
    let _lock = UPDATE.lock().await;
    let requested = mode(
        SENDING.load(Ordering::SeqCst),
        RECEIVING.load(Ordering::SeqCst),
    );
    let supported = tokio::task::spawn_blocking(move || {
        // The Objective-C shim bounds both waits and checks the reply version.
        unsafe { lan_mouse_engine_update(requested) == 1 }
    })
    .await
    .unwrap_or(false);
    static LAST_SUPPORTED: AtomicBool = AtomicBool::new(true);
    if LAST_SUPPORTED.swap(supported, Ordering::SeqCst) != supported {
        if supported {
            log::info!("integrated mouse engine is available");
        } else {
            log::warn!(
                "integrated mouse engine unavailable; input sharing continues without mouse enhancements"
            );
        }
    }
}

extern "C" {
    fn lan_mouse_engine_update(mode: i32) -> i32;
}

#[cfg(test)]
mod tests {
    #[test]
    fn forwarding_wins_during_ownership_transition() {
        assert_eq!(super::mode(false, false), 0);
        assert_eq!(super::mode(true, false), 1);
        assert_eq!(super::mode(false, true), 2);
        assert_eq!(super::mode(true, true), 1);
    }
}
