//! Enter / leave hook execution.
//!
//! Hooks are almost always toggles: Mac Mouse Fix off/on, a monitor input
//! switch, audio routing. The cursor can cross an edge several times within
//! a few hundred milliseconds, and running one shell process per crossing,
//! all concurrently, lets `off` and `on` overlap — the final state is then
//! whichever process happened to *finish* last, not the last crossing.
//!
//! So hooks run strictly one at a time, and per client only the *latest*
//! requested state is applied: a bounce that ends where it started runs
//! nothing, and a state equal to the last one applied is skipped.

use futures::FutureExt;
use lan_mouse_ipc::ClientHandle;
use local_channel::mpsc::{Receiver, Sender, channel};
use std::{collections::HashMap, time::Duration};
use tokio::{
    process::Command,
    task::{JoinHandle, spawn_local},
    time::timeout,
};

/// Where the cursor is with respect to a client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HookState {
    /// on the client => its enter hook
    Entered,
    /// back on this device => its leave hook
    Left,
}

struct HookRequest {
    handle: ClientHandle,
    state: HookState,
    /// the hook configured for that state, if any
    cmd: Option<String>,
}

enum Msg {
    Run(HookRequest),
    Terminate,
}

/// a hook that has not exited after this long is killed so it cannot wedge
/// every hook behind it
const HOOK_TIMEOUT: Duration = Duration::from_secs(10);
/// how long shutdown waits for the hooks still pending
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) struct Hooks {
    tx: Sender<Msg>,
    task: JoinHandle<()>,
}

impl Hooks {
    pub(crate) fn new() -> Self {
        let (tx, rx) = channel();
        let task = spawn_local(HookRunner::default().run(rx));
        Self { tx, task }
    }

    /// the cursor is now in `state` with respect to `handle`; `cmd` is the
    /// hook to run for it, if one is configured
    pub(crate) fn run(&self, handle: ClientHandle, state: HookState, cmd: Option<String>) {
        let _ = self.tx.send(Msg::Run(HookRequest { handle, state, cmd }));
    }

    /// run whatever is still pending, then stop
    pub(crate) async fn terminate(&mut self) {
        let _ = self.tx.send(Msg::Terminate);
        if timeout(SHUTDOWN_TIMEOUT, &mut self.task).await.is_err() {
            log::warn!("hook still running after {SHUTDOWN_TIMEOUT:?}, not waiting for it");
        }
    }
}

#[derive(Default)]
struct HookRunner {
    /// last state applied per client
    applied: HashMap<ClientHandle, HookState>,
    /// latest requested state per client, in order of first request
    wanted: Vec<HookRequest>,
    terminate: bool,
}

impl HookRunner {
    async fn run(mut self, mut rx: Receiver<Msg>) {
        while let Some(msg) = rx.recv().await {
            self.fold(msg);
            // everything queued behind it while the previous hook ran
            while let Some(Some(msg)) = rx.recv().now_or_never() {
                self.fold(msg);
            }
            for cmd in self.commands() {
                run_hook(&cmd).await;
            }
            if self.terminate {
                return;
            }
        }
    }

    /// keep only the latest requested state per client
    fn fold(&mut self, msg: Msg) {
        match msg {
            Msg::Terminate => self.terminate = true,
            Msg::Run(req) => match self.wanted.iter_mut().find(|w| w.handle == req.handle) {
                Some(w) => *w = req,
                None => self.wanted.push(req),
            },
        }
    }

    /// the hooks to run for the folded requests, recording their states —
    /// also for clients without a hook for that state, so the next change
    /// back is not mistaken for a repeat
    fn commands(&mut self) -> Vec<String> {
        std::mem::take(&mut self.wanted)
            .into_iter()
            .filter(|req| self.applied.insert(req.handle, req.state) != Some(req.state))
            .filter_map(|req| req.cmd)
            .collect()
    }
}

async fn run_hook(cmd: &str) {
    log::info!("spawning hook: {cmd}");
    let mut child = match Command::new("sh").arg("-c").arg(cmd).spawn() {
        Ok(c) => c,
        Err(e) => {
            log::warn!("could not execute cmd: {e}");
            return;
        }
    };
    match timeout(HOOK_TIMEOUT, child.wait()).await {
        Ok(Ok(s)) if s.success() => log::info!("{cmd} exited successfully"),
        Ok(Ok(s)) => log::warn!("{cmd} exited with {s}"),
        Ok(Err(e)) => log::warn!("{cmd}: {e}"),
        Err(_) => {
            log::warn!("{cmd} did not exit within {HOOK_TIMEOUT:?}, killing it");
            let _ = child.start_kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{HookRequest, HookRunner, HookState, Hooks, Msg};

    fn run(handle: u64, state: HookState, cmd: &str) -> Msg {
        Msg::Run(HookRequest {
            handle,
            state,
            cmd: Some(cmd.to_string()),
        })
    }

    #[test]
    fn a_bounce_that_ends_where_it_started_runs_nothing() {
        let mut r = HookRunner::default();
        r.applied.insert(0, HookState::Entered);
        // queued while `off` was still running: back, and over again
        r.fold(run(0, HookState::Left, "on"));
        r.fold(run(0, HookState::Entered, "off"));
        assert!(r.commands().is_empty());
        assert_eq!(r.applied[&0], HookState::Entered);
    }

    #[test]
    fn only_the_latest_state_per_client_runs_and_clients_keep_their_order() {
        let mut r = HookRunner::default();
        r.fold(run(0, HookState::Entered, "a off"));
        r.fold(run(1, HookState::Entered, "b off"));
        r.fold(run(0, HookState::Left, "a on"));
        assert_eq!(r.commands(), ["a on", "b off"]);
    }

    #[test]
    fn a_state_without_a_hook_is_still_recorded() {
        let mut r = HookRunner::default();
        r.fold(run(0, HookState::Entered, "off"));
        assert_eq!(r.commands(), ["off"]);
        // no leave hook configured: nothing runs, but the state changes ...
        r.fold(Msg::Run(HookRequest {
            handle: 0,
            state: HookState::Left,
            cmd: None,
        }));
        assert!(r.commands().is_empty());
        // ... so entering again is not a repeat and runs the enter hook
        r.fold(run(0, HookState::Entered, "off"));
        assert_eq!(r.commands(), ["off"]);
    }

    #[test]
    fn terminate_still_applies_what_is_pending() {
        let mut r = HookRunner::default();
        r.fold(run(0, HookState::Left, "on"));
        r.fold(Msg::Terminate);
        assert_eq!(r.commands(), ["on"]);
        assert!(r.terminate);
    }

    #[test]
    fn hooks_run_one_at_a_time_in_order() {
        let out = std::env::temp_dir().join(format!("lan-mouse-hooks-{}", std::process::id()));
        let _ = std::fs::remove_file(&out);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();
        rt.block_on(local.run_until(async {
            let mut hooks = Hooks::new();
            // the slow "off" must finish before "on" starts; run concurrently
            // (the old behaviour) "on" would land in the file first
            hooks.run(
                0,
                HookState::Entered,
                Some(format!("sleep 0.2; echo off >> {}", out.display())),
            );
            // let the runner pick it up and spawn it before "on" is requested,
            // otherwise the fold collapses both into a single "on"
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            hooks.run(
                0,
                HookState::Left,
                Some(format!("echo on >> {}", out.display())),
            );
            hooks.terminate().await;
        }));
        let got = std::fs::read_to_string(&out).unwrap();
        let _ = std::fs::remove_file(&out);
        assert_eq!(got, "off\non\n");
    }
}
