use crate::client::ClientManager;
use crate::config::local_commit;
use lan_mouse_ipc::{ClientHandle, DEFAULT_PORT};
use lan_mouse_proto::{MAX_EVENT_SIZE, ProtoEvent};
use local_channel::mpsc::{Receiver, Sender, channel};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    io,
    net::SocketAddr,
    rc::Rc,
    sync::{Arc, RwLock},
    time::Duration,
};
use thiserror::Error;
use tokio::{
    net::UdpSocket,
    sync::Mutex,
    task::{JoinSet, spawn_local},
};
use webrtc_dtls::{
    config::{Config, ExtendedMasterSecretType},
    conn::DTLSConn,
    crypto::Certificate,
};
use webrtc_util::Conn;

#[derive(Debug, Error)]
pub(crate) enum LanMouseConnectionError {
    #[error(transparent)]
    Bind(#[from] io::Error),
    #[error(transparent)]
    Dtls(#[from] webrtc_dtls::Error),
    #[error(transparent)]
    Webrtc(#[from] webrtc_util::Error),
    #[error("not connected")]
    NotConnected,
    #[error("emulation is disabled on the target device")]
    TargetEmulationDisabled,
    #[error("Connection timed out")]
    Timeout,
}

const DEFAULT_CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);
// Give a configured wired address priority without waiting for its full DTLS
// timeout when the cable is unplugged. Fallback paths race after this delay.
const PREFERRED_ADDRESS_GRACE: Duration = Duration::from_millis(250);

async fn connect(
    addr: SocketAddr,
    cert: Certificate,
) -> Result<(Arc<dyn Conn + Sync + Send>, SocketAddr), (SocketAddr, LanMouseConnectionError)> {
    log::info!("connecting to {addr} ...");
    let conn = Arc::new(
        UdpSocket::bind("0.0.0.0:0")
            .await
            .map_err(|e| (addr, e.into()))?,
    );
    conn.connect(addr).await.map_err(|e| (addr, e.into()))?;
    let config = Config {
        certificates: vec![cert],
        server_name: "ignored".to_owned(),
        insecure_skip_verify: true,
        extended_master_secret: ExtendedMasterSecretType::Require,
        ..Default::default()
    };
    let timeout = tokio::time::sleep(DEFAULT_CONNECTION_TIMEOUT);
    tokio::select! {
        _ = timeout => Err((addr, LanMouseConnectionError::Timeout)),
        result = DTLSConn::new(conn, config, true, None) => match result {
            Ok(dtls_conn) => Ok((Arc::new(dtls_conn), addr)),
            Err(e) => Err((addr, e.into())),
        }
    }
}

async fn connect_any(
    addrs: &[SocketAddr],
    cert: Certificate,
) -> Result<(Arc<dyn Conn + Send + Sync>, SocketAddr), LanMouseConnectionError> {
    let Some((&preferred, fallbacks)) = addrs.split_first() else {
        return Err(LanMouseConnectionError::NotConnected);
    };
    let mut joinset = JoinSet::new();
    joinset.spawn_local(connect(preferred, cert.clone()));
    tokio::select! {
        biased;
        result = joinset.join_next() => match result.expect("preferred attempt").expect("join error") {
            Ok(conn) => return Ok(conn),
            Err((addr, error)) => log::warn!("failed to connect to {addr}: `{error}`"),
        },
        _ = tokio::time::sleep(PREFERRED_ADDRESS_GRACE) => {},
    }
    for &addr in fallbacks {
        joinset.spawn_local(connect(addr, cert.clone()));
    }
    loop {
        match joinset.join_next().await {
            None => return Err(LanMouseConnectionError::NotConnected),
            Some(r) => match r.expect("join error") {
                Ok(conn) => return Ok(conn),
                Err((a, e)) => {
                    log::warn!("failed to connect to {a}: `{e}`")
                }
            },
        };
    }
}

pub(crate) struct LanMouseConnection {
    cert: Certificate,
    client_manager: ClientManager,
    authorized_keys: Arc<RwLock<HashMap<String, String>>>,
    conns: Rc<Mutex<HashMap<SocketAddr, Arc<dyn Conn + Send + Sync>>>>,
    connecting: Rc<Mutex<HashSet<ClientHandle>>>,
    recv_rx: Receiver<(ClientHandle, ProtoEvent)>,
    recv_tx: Sender<(ClientHandle, ProtoEvent)>,
    ping_response: Rc<RefCell<HashSet<SocketAddr>>>,
}

impl LanMouseConnection {
    pub(crate) fn new(
        cert: Certificate,
        client_manager: ClientManager,
        authorized_keys: Arc<RwLock<HashMap<String, String>>>,
    ) -> Self {
        let (recv_tx, recv_rx) = channel();
        Self {
            cert,
            client_manager,
            authorized_keys,
            conns: Default::default(),
            connecting: Default::default(),
            recv_rx,
            recv_tx,
            ping_response: Default::default(),
        }
    }

    pub(crate) async fn recv(&mut self) -> (ClientHandle, ProtoEvent) {
        self.recv_rx.recv().await.expect("channel closed")
    }

    pub(crate) fn supports_enter_with_position(&self, handle: ClientHandle) -> bool {
        self.client_manager.supports_enter_with_position(handle)
    }

    pub(crate) async fn send(
        &self,
        event: ProtoEvent,
        handle: ClientHandle,
    ) -> Result<(), LanMouseConnectionError> {
        let Some(event) = keyboard_event_for_peer(
            event,
            self.client_manager.supports_source_key_repeat(handle),
        ) else {
            return Ok(());
        };
        let (buf, len): ([u8; MAX_EVENT_SIZE], usize) = event.into();
        let buf = &buf[..len];
        if let Some(addr) = self.client_manager.active_addr(handle) {
            let conn = {
                let conns = self.conns.lock().await;
                conns.get(&addr).cloned()
            };
            if let Some(conn) = conn {
                if !self.client_manager.alive(handle) {
                    return Err(LanMouseConnectionError::TargetEmulationDisabled);
                }
                match conn.send(buf).await {
                    Ok(_) => {}
                    Err(e) => {
                        log::warn!("client {handle} failed to send: {e}");
                        disconnect(&self.client_manager, handle, addr, &self.conns).await;
                    }
                }
                log::trace!("{event} >->->->->- {addr}");
                return Ok(());
            }
        }

        // check if we are already trying to connect
        let mut connecting = self.connecting.lock().await;
        if !connecting.contains(&handle) {
            connecting.insert(handle);
            // connect in the background
            spawn_local(connect_to_handle(
                self.client_manager.clone(),
                self.cert.clone(),
                handle,
                self.conns.clone(),
                self.connecting.clone(),
                self.recv_tx.clone(),
                self.ping_response.clone(),
                self.authorized_keys.clone(),
            ));
        }
        Err(LanMouseConnectionError::NotConnected)
    }
}

// Each connection task owns clones of its I/O and authorization state.
#[allow(clippy::too_many_arguments)]
async fn connect_to_handle(
    client_manager: ClientManager,
    cert: Certificate,
    handle: ClientHandle,
    conns: Rc<Mutex<HashMap<SocketAddr, Arc<dyn Conn + Send + Sync>>>>,
    connecting: Rc<Mutex<HashSet<ClientHandle>>>,
    tx: Sender<(ClientHandle, ProtoEvent)>,
    ping_response: Rc<RefCell<HashSet<SocketAddr>>>,
    authorized_keys: Arc<RwLock<HashMap<String, String>>>,
) -> Result<(), LanMouseConnectionError> {
    log::info!("client {handle} connecting ...");
    client_manager.set_peer_protocol_capabilities(handle, false, false);
    // sending did not work, figure out active conn.
    if let Some(addrs) = client_manager.get_ips(handle) {
        let port = client_manager.get_port(handle).unwrap_or(DEFAULT_PORT);
        let addrs = addrs
            .into_iter()
            .map(|a| SocketAddr::new(a, port))
            .collect::<Vec<_>>();
        log::info!("client ({handle}) connecting ... (ips: {addrs:?})");
        let res = connect_any(&addrs, cert).await;
        let (conn, addr) = match res {
            Ok(c) => c,
            Err(e) => {
                connecting.lock().await.remove(&handle);
                return Err(e);
            }
        };
        log::info!("client ({handle}) connected @ {addr}");
        client_manager.set_active_addr(handle, Some(addr));
        conns.lock().await.insert(addr, conn.clone());
        connecting.lock().await.remove(&handle);

        // Best-effort version handshake. Send our commit hash once
        // immediately after the DTLS handshake; the listen side
        // mirrors a Hello back so the receive loop can populate
        // `peer_commit`. Old peers will silently skip this event
        // per the forward-compat handler in [`receive_loop`].
        let (buf, len) = ProtoEvent::Hello {
            commit: local_commit(),
        }
        .into();
        if let Err(e) = conn.send(&buf[..len]).await {
            log::debug!("hello send to {addr} failed: {e}");
        }
        let (buf, len) = ProtoEvent::Capabilities {
            enter_with_position: true,
            source_key_repeat: cfg!(target_os = "macos"),
        }
        .into();
        if let Err(e) = conn.send(&buf[..len]).await {
            log::debug!("capabilities send to {addr} failed: {e}");
        }

        // poll connection for active
        spawn_local(ping_pong(addr, conn.clone(), ping_response.clone()));

        // receiver
        spawn_local(receive_loop(
            client_manager,
            handle,
            addr,
            conn,
            conns,
            tx,
            ping_response.clone(),
            authorized_keys,
        ));
        return Ok(());
    }
    connecting.lock().await.remove(&handle);
    Err(LanMouseConnectionError::NotConnected)
}

async fn ping_pong(
    addr: SocketAddr,
    conn: Arc<dyn Conn + Send + Sync>,
    ping_response: Rc<RefCell<HashSet<SocketAddr>>>,
) {
    loop {
        // Retry negotiation even when a previous capability reply was lost.
        {
            let (buf, len) = ProtoEvent::Capabilities {
                enter_with_position: true,
                source_key_repeat: cfg!(target_os = "macos"),
            }
            .into();
            if let Err(e) = conn.send(&buf[..len]).await {
                log::warn!("{addr}: capability send error `{e}`, closing connection");
                let _ = conn.close().await;
                return;
            }
        }
        let (buf, len) = ProtoEvent::Ping.into();

        // send 4 pings, at least one must be answered
        for _ in 0..4 {
            if let Err(e) = conn.send(&buf[..len]).await {
                log::warn!("{addr}: send error `{e}`, closing connection");
                let _ = conn.close().await;
                break;
            }
            log::trace!("PING >->->->->- {addr}");

            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        if !ping_response.borrow_mut().remove(&addr) {
            log::warn!("{addr} did not respond, closing connection");
            let _ = conn.close().await;
            return;
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn receive_loop(
    client_manager: ClientManager,
    handle: ClientHandle,
    addr: SocketAddr,
    conn: Arc<dyn Conn + Send + Sync>,
    conns: Rc<Mutex<HashMap<SocketAddr, Arc<dyn Conn + Send + Sync>>>>,
    tx: Sender<(ClientHandle, ProtoEvent)>,
    ping_response: Rc<RefCell<HashSet<SocketAddr>>>,
    authorized_keys: Arc<RwLock<HashMap<String, String>>>,
) {
    #[cfg(target_os = "macos")]
    let (profiles, files) = {
        let dtls = conn
            .as_any()
            .downcast_ref::<DTLSConn>()
            .expect("DTLS connection");
        let certificates = dtls.connection_state().await.peer_certificates;
        let fingerprint = certificates
            .first()
            .map(|c| crate::crypto::generate_fingerprint(c))
            .unwrap_or_default();
        (
            crate::mouse_profile::session(
                conn.clone(),
                authorized_keys.clone(),
                fingerprint.clone(),
            ),
            crate::file_bridge::session(conn.clone(), authorized_keys, fingerprint, true),
        )
    };
    #[cfg(not(target_os = "macos"))]
    let _ = authorized_keys;
    let mut received = [0u8; 1024];
    while let Ok(size) = conn.recv(&mut received).await {
        #[cfg(target_os = "macos")]
        if received[..size].starts_with(crate::mouse_profile::MAGIC) {
            let _ = profiles.try_send(received[..size].to_vec());
            continue;
        }
        #[cfg(target_os = "macos")]
        if received[..size].starts_with(crate::file_bridge::MAGIC) {
            let _ = files.try_send(received[..size].to_vec());
            continue;
        }
        if size == 0 || size > MAX_EVENT_SIZE {
            continue;
        }
        let mut buf = [0u8; MAX_EVENT_SIZE];
        buf[..size].copy_from_slice(&received[..size]);
        match buf.try_into() {
            Ok(event) => {
                log::trace!("{addr} <==<==<== {event}");
                match event {
                    ProtoEvent::Pong(b) => {
                        client_manager.set_active_addr(handle, Some(addr));
                        client_manager.set_alive(handle, b);
                        ping_response.borrow_mut().insert(addr);
                        // the capture task wants to know when the peer it is
                        // sending to stops being a live target (see there)
                        tx.send((handle, event)).expect("channel closed");
                    }
                    ProtoEvent::Hello { commit } => {
                        client_manager.set_peer_commit(handle, Some(commit));
                    }
                    ProtoEvent::Capabilities {
                        enter_with_position,
                        source_key_repeat,
                    } => {
                        client_manager.set_peer_protocol_capabilities(
                            handle,
                            enter_with_position,
                            source_key_repeat,
                        );
                    }
                    event => tx.send((handle, event)).expect("channel closed"),
                }
            }
            // Skip undecodable datagrams without dropping the
            // connection. Each DTLS recv is one framed message, so
            // skipping is safe and keeps us forward-compatible with
            // peers that send event types we don't yet know about.
            Err(e) => log::debug!("ignoring undecodable event from {addr}: {e}"),
        }
    }
    log::warn!("recv error");
    disconnect(&client_manager, handle, addr, &conns).await;
}

async fn disconnect(
    client_manager: &ClientManager,
    handle: ClientHandle,
    addr: SocketAddr,
    conns: &Mutex<HashMap<SocketAddr, Arc<dyn Conn + Send + Sync>>>,
) {
    log::warn!("client ({handle}) @ {addr} connection closed");
    conns.lock().await.remove(&addr);
    client_manager.set_active_addr(handle, None);
    client_manager.set_peer_commit(handle, None);
    client_manager.set_peer_protocol_capabilities(handle, false, false);
    let active: Vec<SocketAddr> = conns.lock().await.keys().copied().collect();
    log::info!("active connections: {active:?}");
}

/// Only macOS capture currently produces source-timed repeats. Other backends
/// and older receivers retain the existing press/release behavior.
fn keyboard_event_for_peer(mut event: ProtoEvent, source_repeat: bool) -> Option<ProtoEvent> {
    if cfg!(target_os = "macos") {
        if let ProtoEvent::Input(input_event::Event::Keyboard(input_event::KeyboardEvent::Key {
            state,
            ..
        })) = &mut event
        {
            *state = match (*state, source_repeat) {
                (1, true) => input_event::KEY_PRESSED_NO_REPEAT,
                (input_event::KEY_REPEATED, false) => return None,
                (state, _) => state,
            };
        }
    }
    Some(event)
}

#[cfg(all(test, target_os = "macos"))]
mod repeat_tests {
    use super::*;
    use input_event::{Event, KEY_PRESSED_NO_REPEAT, KEY_REPEATED, KeyboardEvent};

    fn key(state: u8) -> ProtoEvent {
        ProtoEvent::Input(Event::Keyboard(KeyboardEvent::Key {
            time: 42,
            key: 30,
            state,
        }))
    }
    fn state(event: Option<ProtoEvent>) -> Option<u8> {
        match event {
            Some(ProtoEvent::Input(Event::Keyboard(KeyboardEvent::Key { state, .. }))) => {
                Some(state)
            }
            _ => None,
        }
    }

    #[test]
    fn old_peers_get_legacy_presses_and_no_source_repeats() {
        assert_eq!(state(keyboard_event_for_peer(key(1), false)), Some(1));
        assert_eq!(
            state(keyboard_event_for_peer(key(KEY_REPEATED), false)),
            None
        );
        assert_eq!(state(keyboard_event_for_peer(key(0), false)), Some(0));
    }

    #[test]
    fn new_peers_get_explicit_presses_repeats_and_releases() {
        assert_eq!(
            state(keyboard_event_for_peer(key(1), true)),
            Some(KEY_PRESSED_NO_REPEAT)
        );
        assert_eq!(
            state(keyboard_event_for_peer(key(KEY_REPEATED), true)),
            Some(KEY_REPEATED)
        );
        assert_eq!(state(keyboard_event_for_peer(key(0), true)), Some(0));
    }
}

#[cfg(test)]
mod address_preference_tests {
    use super::*;
    use webrtc_util::conn::Listener;

    async fn check_preference(preferred_available: bool) {
        let cert = Certificate::generate_self_signed(vec!["localhost".to_owned()]).unwrap();
        let listener = webrtc_dtls::listener::listen(
            "127.0.0.1:0",
            Config {
                certificates: vec![cert.clone()],
                extended_master_secret: ExtendedMasterSecretType::Require,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let live_addr = listener.addr().await.unwrap();
        // A bound socket that never responds models a disconnected cable.
        let silent = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let silent_addr = silent.local_addr().unwrap();
        let addresses = if preferred_available {
            [live_addr, silent_addr]
        } else {
            [silent_addr, live_addr]
        };
        let (outbound, inbound) = tokio::time::timeout(Duration::from_secs(3), async {
            tokio::join!(connect_any(&addresses, cert), listener.accept())
        })
        .await
        .expect("Wi-Fi fallback must not wait for the five-second connection timeout");
        let (outbound, chosen) = outbound.unwrap();
        let (inbound, _) = inbound.unwrap();
        assert_eq!(chosen, live_addr);
        let mut packet = [0; 2048];
        if preferred_available {
            assert_eq!(
                silent.try_recv(&mut packet).unwrap_err().kind(),
                io::ErrorKind::WouldBlock,
                "fallback must not be attempted when the preferred address connects promptly"
            );
        } else {
            assert!(
                silent.try_recv(&mut packet).is_ok(),
                "preferred address must be tried first"
            );
        }
        outbound.close().await.unwrap();
        inbound.close().await.unwrap();
        listener.close().await.unwrap();
    }

    #[tokio::test]
    async fn healthy_preferred_address_is_used_before_fallback() {
        tokio::task::LocalSet::new()
            .run_until(check_preference(true))
            .await;
    }

    #[tokio::test]
    async fn unavailable_preferred_address_falls_back_without_full_timeout() {
        tokio::task::LocalSet::new()
            .run_until(check_preference(false))
            .await;
    }
}
