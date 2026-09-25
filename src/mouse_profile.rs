//! Opt-in Mac Mouse Fix profile replication over an existing authenticated DTLS
//! connection. Local permissions, identities, licenses and UI state never travel.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{self, Write},
    os::fd::AsRawFd,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use webrtc_util::Conn;

pub(crate) const MAGIC: &[u8; 5] = b"LMMP\x01";
const CHUNK_SIZE: usize = 900;
const MAX_PROFILE: usize = 64 * 1024;
const HEADER: usize = 44;
const GENERAL_KEYS: &[&str] = &[
    "buttonKillSwitch",
    "scrollKillSwitch",
    "lockPointerDuringDrag",
];
static FILE_LOCK: Mutex<()> = Mutex::new(());
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
type Trust = Arc<RwLock<HashMap<String, String>>>;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Profile {
    schema: u32,
    revision: u64,
    author: String,
    settings: Value,
}
impl Profile {
    fn validate(&self) -> Result<()> {
        if self.schema != 1
            || self.revision > (1 << 53)
            || self.author.is_empty()
            || self.author.len() > 128
        {
            return Err("unsupported mouse profile version or revision".into());
        }
        let settings = self
            .settings
            .as_object()
            .ok_or("mouse profile must be an object")?;
        if settings.len() != 4
            || !settings.get("Scroll").is_some_and(Value::is_object)
            || !settings.get("Pointer").is_some_and(Value::is_object)
            || !settings.get("Remaps").is_some_and(Value::is_array)
            || !settings.get("General").is_some_and(Value::is_object)
        {
            return Err(
                "mouse profile must contain only Scroll, Pointer, Remaps and General".into(),
            );
        }
        let general = settings["General"].as_object().unwrap();
        if general.len() != GENERAL_KEYS.len()
            || general
                .keys()
                .any(|key| !GENERAL_KEYS.contains(&key.as_str()))
            || general.values().any(|v| !v.is_boolean())
        {
            return Err("mouse profile contains device-local general settings".into());
        }
        validate_settings(settings)?;
        if serde_json::to_vec(self)?.len() > MAX_PROFILE {
            return Err("mouse profile is too large".into());
        }
        Ok(())
    }
    fn newer_than(&self, other: &Self) -> bool {
        (self.revision, &self.author) > (other.revision, &other.author)
    }
}

fn validate_settings(settings: &Map<String, Value>) -> Result<()> {
    let scroll = &settings["Scroll"];
    let pointer = &settings["Pointer"];
    let one_of = |v: &Value, choices: &[&str]| v.as_str().is_some_and(|s| choices.contains(&s));
    let uint = |v: &Value, max| v.as_u64().is_some_and(|v| v <= max);
    if !one_of(&scroll["smooth"], &["off", "low", "regular", "high"])
        || !one_of(&scroll["speed"], &["system", "low", "medium", "high"])
        || ["precise", "reverseDirection", "trackpadSimulation"]
            .iter()
            .any(|k| !scroll[k].is_boolean())
        || ["horizontal", "precise", "swift", "zoom"]
            .iter()
            .any(|k| !uint(&scroll["modifiers"][k], u32::MAX as u64))
        || !pointer["useSystemAcceleration"].is_boolean()
        || !pointer["sensitivity"]
            .as_f64()
            .is_some_and(|v| v > 0.0 && v <= 100.0)
        || !pointer["acceleration"]
            .as_f64()
            .is_some_and(|v| (-1.0..=10.0).contains(&v))
    {
        return Err("invalid scrolling or pointer settings".into());
    }
    for row in settings["Remaps"].as_array().unwrap() {
        let trigger = &row["trigger"];
        let effect = &row["effect"];
        let modifiers = &row["modifiers"];
        let button = |v: &Value| {
            uint(&v["button"], 32) && v["button"] != 0 && uint(&v["level"], 16) && v["level"] != 0
        };
        if !effect.is_object()
            || !modifiers.is_object()
            || modifiers
                .get("keyboardModifiers")
                .is_some_and(|v| !uint(v, u32::MAX as u64))
            || modifiers
                .get("buttonModifiers")
                .is_some_and(|v| !v.as_array().is_some_and(|a| a.iter().all(button)))
        {
            return Err("invalid mouse action modifiers".into());
        }
        let valid = match trigger.as_str() {
            Some("dragTrigger") => {
                one_of(
                    &effect["modifiedDragType"],
                    &["twoFingerSwipe", "threeFingerSwipe", "zoom"],
                ) || (effect["modifiedDragType"] == "fakeDrag"
                    && uint(&effect["buttonNumber"], 32)
                    && effect["buttonNumber"] != 0)
            }
            Some("scrollTrigger") => {
                let input = effect.get("modifiedScrollInputModification");
                let output = effect.get("modifiedScrollEffectModification");
                (input.is_some() || output.is_some())
                    && input.is_none_or(|v| one_of(v, &["precision", "fast"]))
                    && output.is_none_or(|v| {
                        one_of(
                            v,
                            &[
                                "zoom",
                                "horizontal",
                                "fourFingerPinch",
                                "threeFingerSwipeHorizontal",
                                "rotate",
                                "commandTab",
                            ],
                        )
                    })
            }
            None if trigger.is_object()
                && button(trigger)
                && one_of(&trigger["duration"], &["click", "hold"]) =>
            {
                match effect["type"].as_str() {
                    Some("smartZoom") => true,
                    Some("symbolicHotkey") => uint(&effect["variant"], 65535),
                    Some("navigationSwipe") => {
                        one_of(&effect["variant"], &["up", "down", "left", "right"])
                    }
                    Some("keyboardShortcut") => {
                        uint(&effect["keycode"], 65535) && uint(&effect["flags"], u32::MAX as u64)
                    }
                    Some("systemDefinedEvent") => {
                        uint(&effect["systemDefinedEventType"], 65535)
                            && uint(&effect["flags"], u32::MAX as u64)
                    }
                    Some("mouseButton") => {
                        uint(&effect["button"], 32)
                            && effect["button"] != 0
                            && uint(&effect["nOfClicks"], 16)
                            && effect["nOfClicks"] != 0
                    }
                    _ => false,
                }
            }
            _ => false,
        };
        if !valid {
            return Err("unsupported or incomplete mouse action".into());
        }
    }
    // Reject values that cannot be represented in the destination plist, even in
    // preserved advanced fields, before beginning a filesystem transaction.
    json_to_plist(&Value::Object(settings.clone()))?;
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    enabled: bool,
    node: String,
    document: Profile,
}

// Also held by the control panel. MMF does not participate, so applying a
// remote edit additionally checks that its config has not changed under us.
struct FileLock(fs::File);
impl FileLock {
    fn acquire(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path.with_extension("lock"))?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(Self(file))
    }
}
impl Drop for FileLock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

#[derive(Clone)]
struct Store {
    state: PathBuf,
    config: PathBuf,
}
impl Store {
    fn local() -> Result<Self> {
        let home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME unavailable")?);
        Ok(Self {
            state: home.join(".config/lan-mouse/mouse-profile.json"),
            config: home
                .join("Library/Application Support/com.nuebling.mac-mouse-fix/config.plist"),
        })
    }
    fn read(&self) -> Result<Option<(State, plist::Value)>> {
        let bytes = match fs::read(&self.state) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        if bytes.len() > MAX_PROFILE * 2 {
            return Err("mouse profile state is too large".into());
        }
        let mut state: State = serde_json::from_slice(&bytes)?;
        if !state.enabled {
            return Ok(None);
        }
        state.document.validate()?;
        if state.node.is_empty() || state.node.len() > 128 {
            return Err("invalid local profile node".into());
        }
        let config = plist::Value::from_file(&self.config)?;
        let settings = extract(&config)?;
        if state.document.settings != settings {
            state.document.revision += 1;
            state.document.author = state.node.clone();
            state.document.settings = settings;
            state.document.validate()?;
            atomic_write(&self.state, &serde_json::to_vec_pretty(&state)?)?;
        }
        Ok(Some((state, config)))
    }
    fn current(&self) -> Result<Option<Profile>> {
        let _lock = FILE_LOCK.lock().map_err(|_| "profile lock poisoned")?;
        if !self.state.exists() {
            return Ok(None);
        }
        let _file_lock = FileLock::acquire(&self.state)?;
        Ok(self.read()?.map(|(s, _)| s.document))
    }
    fn apply(&self, remote: Profile) -> Result<bool> {
        remote.validate()?;
        let _lock = FILE_LOCK.lock().map_err(|_| "profile lock poisoned")?;
        if !self.state.exists() {
            return Ok(false);
        }
        let _file_lock = FileLock::acquire(&self.state)?;
        let Some((mut state, mut config)) = self.read()? else {
            return Ok(false);
        };
        if !remote.newer_than(&state.document) {
            return Ok(true);
        }
        let original = fs::read(&self.config)?;
        if plist::Value::from_reader(std::io::Cursor::new(&original))? != config {
            return Err("mouse settings changed during sync; will retry".into());
        }
        // Keep a recoverable copy before the first remote change. Never replace it.
        let backup = self
            .config
            .with_file_name("config.before-lan-mouse-sync.plist");
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&backup)
        {
            Ok(mut file) => {
                file.write_all(&original)?;
                file.sync_all()?;
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        let root = config
            .as_dictionary_mut()
            .ok_or("invalid Mac Mouse Fix configuration")?;
        for key in ["Scroll", "Pointer", "Remaps"] {
            root.insert(key.into(), json_to_plist(&remote.settings[key])?);
        }
        let general = root
            .get_mut("General")
            .and_then(plist::Value::as_dictionary_mut)
            .ok_or("missing General settings")?;
        for (key, value) in remote.settings["General"].as_object().unwrap() {
            general.insert(key.clone(), json_to_plist(value)?);
        }
        let mut bytes = Vec::new();
        config.to_writer_xml(&mut bytes)?;
        if fs::read(&self.config)? != original {
            return Err("mouse settings changed during sync; will retry".into());
        }
        atomic_write(&self.config, &bytes)?;
        state.document = remote;
        atomic_write(&self.state, &serde_json::to_vec_pretty(&state)?)?;
        log::info!(
            "shared mouse profile applied (revision {})",
            state.document.revision
        );
        Ok(true)
    }
}

fn extract(config: &plist::Value) -> Result<Value> {
    let root = config
        .as_dictionary()
        .ok_or("invalid Mac Mouse Fix configuration")?;
    if root
        .get("Constants")
        .and_then(plist::Value::as_dictionary)
        .and_then(|c| c.get("configVersion"))
        .and_then(plist::Value::as_unsigned_integer)
        != Some(24)
    {
        return Err("unsupported Mac Mouse Fix configuration version".into());
    }
    let mut output = Map::new();
    for key in ["Scroll", "Pointer", "Remaps"] {
        output.insert(
            key.into(),
            serde_json::to_value(root.get(key).ok_or("incomplete mouse configuration")?)?,
        );
    }
    let general = root
        .get("General")
        .and_then(plist::Value::as_dictionary)
        .ok_or("missing General settings")?;
    let mut shared = Map::new();
    for key in GENERAL_KEYS {
        if let Some(value) = general.get(key) {
            shared.insert((*key).into(), serde_json::to_value(value)?);
        }
    }
    output.insert("General".into(), Value::Object(shared));
    Ok(Value::Object(output))
}
fn json_to_plist(value: &Value) -> Result<plist::Value> {
    Ok(match value {
        Value::Bool(v) => (*v).into(),
        Value::String(v) => v.clone().into(),
        Value::Number(v) => {
            if let Some(v) = v.as_i64() {
                v.into()
            } else if let Some(v) = v.as_u64() {
                v.into()
            } else {
                v.as_f64().ok_or("invalid number")?.into()
            }
        }
        Value::Array(v) => plist::Value::Array(v.iter().map(json_to_plist).collect::<Result<_>>()?),
        Value::Object(v) => plist::Value::Dictionary(
            v.iter()
                .map(|(k, v)| Ok((k.clone(), json_to_plist(v)?)))
                .collect::<Result<_>>()?,
        ),
        Value::Null => return Err("null is not a mouse setting".into()),
    })
}
fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    let temp = path.with_extension(format!("lan-mouse-{}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)?;
    let result = (|| -> Result<()> {
        file.write_all(data)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

fn packets(profile: &Profile) -> Result<Vec<Vec<u8>>> {
    profile.validate()?;
    let bytes = serde_json::to_vec(profile)?;
    let hash = Sha256::digest(&bytes);
    let count = bytes.len().div_ceil(CHUNK_SIZE);
    Ok(bytes
        .chunks(CHUNK_SIZE)
        .enumerate()
        .map(|(index, chunk)| {
            let mut out = MAGIC.to_vec();
            out.push(1);
            out.extend_from_slice(&hash);
            out.extend_from_slice(&(index as u16).to_be_bytes());
            out.extend_from_slice(&(count as u16).to_be_bytes());
            out.extend_from_slice(&(chunk.len() as u16).to_be_bytes());
            out.extend_from_slice(chunk);
            out
        })
        .collect())
}
struct Assembly {
    hash: [u8; 32],
    parts: Vec<Option<Vec<u8>>>,
    started: Instant,
}
impl Assembly {
    fn accept(slot: &mut Option<Self>, bytes: &[u8]) -> Result<Option<(Profile, [u8; 32])>> {
        if bytes.len() < HEADER || &bytes[..5] != MAGIC || bytes[5] != 1 {
            return Err("invalid profile packet".into());
        }
        let hash: [u8; 32] = bytes[6..38].try_into()?;
        let index = u16::from_be_bytes(bytes[38..40].try_into()?) as usize;
        let count = u16::from_be_bytes(bytes[40..42].try_into()?) as usize;
        // Packet length itself frames each chunk; exactly one DTLS record per chunk.
        let payload = bytes.get(44..).ok_or("short profile chunk")?;
        let length = u16::from_be_bytes(bytes[42..44].try_into()?) as usize;
        if count == 0
            || count > MAX_PROFILE.div_ceil(CHUNK_SIZE)
            || index >= count
            || length != payload.len()
            || length == 0
            || length > CHUNK_SIZE
            || (index + 1 < count && length != CHUNK_SIZE)
        {
            return Err("invalid profile chunk bounds".into());
        }
        if slot
            .as_ref()
            .is_none_or(|s| s.hash != hash || s.started.elapsed() > Duration::from_secs(10))
        {
            *slot = Some(Self {
                hash,
                parts: vec![None; count],
                started: Instant::now(),
            });
        }
        let state = slot.as_mut().unwrap();
        if state.parts.len() != count {
            return Err("inconsistent profile chunks".into());
        }
        state.parts[index] = Some(payload.to_vec());
        if state.parts.iter().any(Option::is_none) {
            return Ok(None);
        }
        let bytes: Vec<u8> = state
            .parts
            .iter()
            .flat_map(|p| p.as_ref().unwrap().iter().copied())
            .collect();
        *slot = None;
        if bytes.len() > MAX_PROFILE || Sha256::digest(&bytes)[..] != hash {
            return Err("profile digest mismatch".into());
        }
        let profile: Profile = serde_json::from_slice(&bytes)?;
        profile.validate()?;
        Ok(Some((profile, hash)))
    }
}

/// Filesystem work and profile retransmission run outside the input receive loop.
/// Certificates must still be authorized on BOTH inbound and outbound sessions.
pub(crate) fn session(
    conn: Arc<dyn Conn + Send + Sync>,
    trust: Trust,
    peer: String,
) -> mpsc::Sender<Vec<u8>> {
    let store = match Store::local() {
        Ok(store) => store,
        Err(e) => {
            log::warn!("mouse profile storage unavailable: {e}");
            return mpsc::channel(1).0;
        }
    };
    session_with_store(conn, trust, peer, store)
}

fn session_with_store(
    conn: Arc<dyn Conn + Send + Sync>,
    trust: Trust,
    peer: String,
    store: Store,
) -> mpsc::Sender<Vec<u8>> {
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(96);
    tokio::task::spawn_local(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last_hello: Option<Instant> = None;
        let mut acked: Option<[u8; 32]> = None;
        let mut assembly = None;
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if !trust.read().is_ok_and(|t| t.contains_key(&peer)) { continue; }
                    let store = store.clone();
                    let current = tokio::task::spawn_blocking(move || store.current()).await;
                    let profile = match current {
                        Ok(Ok(Some(p))) => p,
                        Ok(Ok(None)) => continue,
                        other => { log::debug!("mouse profile unavailable: {other:?}"); continue; }
                    };
                    let mut hello = MAGIC.to_vec(); hello.push(0);
                    if conn.send(&hello).await.is_err() { break; }
                    if last_hello.is_some_and(|time| time.elapsed() < Duration::from_secs(6)) {
                        if let Ok(chunks) = packets(&profile) {
                            let hash: [u8;32] = chunks[0][6..38].try_into().unwrap();
                            if acked != Some(hash) {
                                for chunk in chunks { if conn.send(&chunk).await.is_err() { return; } tokio::task::yield_now().await; }
                            }
                        }
                    }
                }
                packet = rx.recv() => {
                    let Some(packet) = packet else { break; };
                    if !trust.read().is_ok_and(|t| t.contains_key(&peer)) { continue; }
                    match packet.get(5) {
                        Some(0) if packet.len() == 6 => { last_hello = Some(Instant::now()); },
                        Some(2) if packet.len() == 38 => { acked = packet[6..38].try_into().ok(); },
                        Some(1) if last_hello.is_some_and(|time| time.elapsed() < Duration::from_secs(6)) => match Assembly::accept(&mut assembly, &packet) {
                            Ok(Some((profile, hash))) => {
                                let store = store.clone();
                                match tokio::task::spawn_blocking(move || store.apply(profile)).await {
                                    Ok(Ok(true)) => {
                                        let mut ack = MAGIC.to_vec(); ack.push(2); ack.extend_from_slice(&hash);
                                        let _ = conn.send(&ack).await;
                                    }
                                    other => log::warn!("mouse profile was not applied: {other:?}"),
                                }
                            }
                            Ok(None) => {},
                            Err(e) => log::debug!("ignoring invalid mouse profile: {e}"),
                        },
                        _ => {},
                    }
                }
            }
        }
    });
    tx
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn profile() -> Profile {
        Profile {
            schema: 1,
            revision: 1,
            author: "mac-a".into(),
            settings: json!({
                "Scroll": {"smooth":"off", "speed":"system", "reverseDirection":false,"precise":false,"trackpadSimulation":true,"modifiers":{"horizontal":131072,"precise":524288,"swift":262144,"zoom":1048576}},
                "Pointer": {"sensitivity":1.0,"acceleration":0.6875,"useSystemAcceleration":false}, "Remaps": [],
                "General": {"scrollKillSwitch":false,"buttonKillSwitch":false,"lockPointerDuringDrag":false}
            }),
        }
    }
    #[test]
    fn fragmented_profile_survives_reordering_duplicates_and_missing_packets() {
        let mut profile = profile();
        profile.settings["Remaps"] = json!((0..60).map(|_| json!({"effect":{"type":"keyboardShortcut","keycode":12,"flags":1048576},"trigger":{"button":4,"level":1,"duration":"click"},"modifiers":{}})).collect::<Vec<_>>());
        let chunks = packets(&profile).unwrap();
        assert!(chunks.len() > 3);
        assert!(chunks.iter().all(|c| c.len() <= 1024));
        let mut assembly = None;
        for chunk in chunks.iter().skip(1).rev() {
            assert!(Assembly::accept(&mut assembly, chunk).unwrap().is_none());
            assert!(Assembly::accept(&mut assembly, chunk).unwrap().is_none());
        }
        assert_eq!(
            Assembly::accept(&mut assembly, &chunks[0])
                .unwrap()
                .unwrap()
                .0,
            profile
        );
    }
    #[test]
    fn corrupt_and_unbounded_profiles_are_rejected() {
        let mut chunk = packets(&profile()).unwrap().remove(0);
        *chunk.last_mut().unwrap() ^= 1;
        assert!(Assembly::accept(&mut None, &chunk).is_err());
        chunk[40..42].copy_from_slice(&u16::MAX.to_be_bytes());
        assert!(Assembly::accept(&mut None, &chunk).is_err());
        for length in 0..44 {
            assert!(Assembly::accept(&mut None, &chunk[..length]).is_err());
        }
        let mut malicious = profile();
        malicious.settings = json!({"License":{},"State":{},"UI":{},"Permissions":{}});
        assert!(malicious.validate().is_err());
        let mut malicious = profile();
        malicious.settings["General"]["showMenuBarItem"] = json!(false);
        assert!(malicious.validate().is_err());
    }
    #[test]
    fn drag_zoom_survives_profile_transport() {
        let mut zoom = profile();
        zoom.settings["Remaps"] = json!([{
            "trigger": "dragTrigger",
            "modifiers": {"buttonModifiers": [{"button": 4, "level": 1}]},
            "effect": {"modifiedDragType": "zoom"}
        }]);
        let mut assembly = None;
        let mut received = None;
        for packet in packets(&zoom).unwrap() {
            if let Some((profile, _)) = Assembly::accept(&mut assembly, &packet).unwrap() {
                received = Some(profile);
            }
        }
        assert_eq!(received, Some(zoom.clone()));
        zoom.settings["Remaps"][0]["effect"]["modifiedDragType"] = json!("unknownDrag");
        assert!(zoom.validate().is_err());
    }
    #[test]
    fn malformed_engine_settings_are_rejected() {
        for (path, value) in [
            ("/Scroll/smooth", json!("invalid")),
            ("/Scroll/modifiers/zoom", json!("not a number")),
            ("/Pointer/sensitivity", json!(-1)),
            ("/General/scrollKillSwitch", json!(null)),
            (
                "/Remaps",
                json!([{"trigger":{"button":4,"level":1,"duration":"click"},"modifiers":{},"effect":{"type":"keyboardShortcut","flags":0}}]),
            ),
        ] {
            let mut invalid = profile();
            *invalid.settings.pointer_mut(path).unwrap() = value;
            assert!(invalid.validate().is_err(), "accepted {path}");
        }
        let mut invalid = profile();
        invalid.settings["General"]
            .as_object_mut()
            .unwrap()
            .remove("scrollKillSwitch");
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn same_revision_conflicts_converge_deterministically() {
        let a = profile();
        let mut b = a.clone();
        b.author = "mac-b".into();
        assert!(b.newer_than(&a));
        assert!(!a.newer_than(&b));
        let mut next = a.clone();
        next.revision = 2;
        assert!(next.newer_than(&b));
    }
    #[test]
    fn sync_keeps_license_identity_and_local_ui_and_records_local_edits() {
        let dir =
            std::env::temp_dir().join(format!("lan-mouse-profile-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let store = Store {
            state: dir.join("profile.json"),
            config: dir.join("config.plist"),
        };
        let mut config = json_to_plist(&profile().settings).unwrap();
        let root = config.as_dictionary_mut().unwrap();
        root.insert(
            "Constants".into(),
            json_to_plist(&json!({"configVersion":24})).unwrap(),
        );
        root.insert(
            "License".into(),
            json_to_plist(&json!({"localSecret":"test-only"})).unwrap(),
        );
        root.insert(
            "State".into(),
            json_to_plist(&json!({"tab":"local"})).unwrap(),
        );
        root.get_mut("General")
            .unwrap()
            .as_dictionary_mut()
            .unwrap()
            .insert("showMenuBarItem".into(), true.into());
        config.to_file_xml(&store.config).unwrap();
        let state = State {
            enabled: true,
            node: "mac-a".into(),
            document: profile(),
        };
        fs::write(&store.state, serde_json::to_vec(&state).unwrap()).unwrap();
        assert_eq!(store.current().unwrap().unwrap(), profile());
        let mut remote = profile();
        remote.author = "mac-b".into();
        remote.revision = 4;
        remote.settings["Scroll"]["reverseDirection"] = json!(true);
        assert!(store.apply(remote.clone()).unwrap());
        let got = plist::Value::from_file(&store.config).unwrap();
        let got = got.as_dictionary().unwrap();
        let old = config.as_dictionary().unwrap();
        assert_eq!(got["License"], old["License"]);
        assert_eq!(got["State"], old["State"]);
        assert_eq!(
            got["General"].as_dictionary().unwrap()["showMenuBarItem"],
            true.into()
        );
        assert_eq!(store.current().unwrap().unwrap(), remote);
        let mut local = plist::Value::from_file(&store.config).unwrap();
        local
            .as_dictionary_mut()
            .unwrap()
            .get_mut("Scroll")
            .unwrap()
            .as_dictionary_mut()
            .unwrap()
            .insert("reverseDirection".into(), false.into());
        local.to_file_xml(&store.config).unwrap();
        let edited = store.current().unwrap().unwrap();
        assert_eq!(edited.revision, 5);
        assert_eq!(edited.author, "mac-a");
        assert_eq!(edited.settings["Scroll"]["reverseDirection"], false);
        assert!(dir.join("config.before-lan-mouse-sync.plist").exists());
        let mut disabled: State = serde_json::from_slice(&fs::read(&store.state).unwrap()).unwrap();
        disabled.enabled = false;
        fs::write(&store.state, serde_json::to_vec(&disabled).unwrap()).unwrap();
        let unchanged = fs::read(&store.config).unwrap();
        assert!(store.current().unwrap().is_none());
        assert!(!store.apply(edited.clone()).unwrap());
        assert_eq!(fs::read(&store.config).unwrap(), unchanged);
        disabled.enabled = true;
        fs::write(&store.state, serde_json::to_vec(&disabled).unwrap()).unwrap();
        local
            .as_dictionary_mut()
            .unwrap()
            .get_mut("Constants")
            .unwrap()
            .as_dictionary_mut()
            .unwrap()
            .insert("configVersion".into(), 25.into());
        local.to_file_xml(&store.config).unwrap();
        assert!(store.current().is_err());
        assert!(store.apply(edited).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn sessions_retry_and_sync_both_directions_only_with_approved_peers() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let directory = std::env::temp_dir()
                    .join(format!("lan-mouse-sync-session-{}", std::process::id()));
                fs::create_dir_all(&directory).unwrap();
                let make_store = |name: &str, revision: u64, reversed: bool| {
                    let path = directory.join(name);
                    fs::create_dir_all(&path).unwrap();
                    let store = Store {
                        state: path.join("profile.json"),
                        config: path.join("config.plist"),
                    };
                    let mut profile = profile();
                    profile.author = name.into();
                    profile.revision = revision;
                    profile.settings["Scroll"]["reverseDirection"] = json!(reversed);
                    let mut config = json_to_plist(&profile.settings).unwrap();
                    config.as_dictionary_mut().unwrap().insert(
                        "Constants".into(),
                        json_to_plist(&json!({"configVersion":24})).unwrap(),
                    );
                    config.to_file_xml(&store.config).unwrap();
                    fs::write(
                        &store.state,
                        serde_json::to_vec(&State {
                            enabled: true,
                            node: name.into(),
                            document: profile,
                        })
                        .unwrap(),
                    )
                    .unwrap();
                    store
                };
                let a_store = make_store("a", 2, true);
                let b_store = make_store("b", 1, false);
                let a_trust: Trust = Default::default();
                let b_trust: Trust =
                    Arc::new(RwLock::new(HashMap::from([("a".into(), "Mac A".into())])));
                let (a, b) = webrtc_util::conn::conn_pipe::pipe();
                let a = Arc::new(a);
                let b = Arc::new(b);
                let a_tx =
                    session_with_store(a.clone(), a_trust.clone(), "b".into(), a_store.clone());
                let b_tx = session_with_store(b.clone(), b_trust, "a".into(), b_store.clone());
                let pump = |conn: Arc<dyn Conn + Send + Sync>,
                            tx: mpsc::Sender<Vec<u8>>,
                            mut drop_chunk: bool| {
                    tokio::task::spawn_local(async move {
                        let mut bytes = [0u8; 1024];
                        while let Ok(size) = conn.recv(&mut bytes).await {
                            if bytes.get(5) == Some(&1) && drop_chunk {
                                drop_chunk = false;
                                continue;
                            }
                            if tx.send(bytes[..size].to_vec()).await.is_err() {
                                break;
                            }
                        }
                    })
                };
                let a_pump = pump(a, a_tx, false);
                let b_pump = pump(b, b_tx, true);
                // B sends a hello, but A must neither send settings nor accept B's
                // profile until B's certificate identity is approved locally.
                tokio::time::sleep(Duration::from_millis(2200)).await;
                assert_eq!(a_store.current().unwrap().unwrap().revision, 2);
                assert_eq!(b_store.current().unwrap().unwrap().revision, 1);
                a_trust.write().unwrap().insert("b".into(), "Mac B".into());
                async fn wait_for(store: &Store, revision: u64) {
                    tokio::time::timeout(Duration::from_secs(9), async {
                        loop {
                            if store.current().unwrap().unwrap().revision == revision {
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(50)).await;
                        }
                    })
                    .await
                    .expect("settings sync timed out");
                }
                // The first profile chunk is lost. A retries without reconnecting.
                wait_for(&b_store, 2).await;
                assert_eq!(b_store.current().unwrap(), a_store.current().unwrap());
                let mut edited = plist::Value::from_file(&b_store.config).unwrap();
                edited
                    .as_dictionary_mut()
                    .unwrap()
                    .get_mut("Scroll")
                    .unwrap()
                    .as_dictionary_mut()
                    .unwrap()
                    .insert("reverseDirection".into(), false.into());
                edited.to_file_xml(&b_store.config).unwrap();
                wait_for(&a_store, 3).await;
                assert_eq!(a_store.current().unwrap(), b_store.current().unwrap());
                a_trust.write().unwrap().clear();
                edited
                    .as_dictionary_mut()
                    .unwrap()
                    .get_mut("Scroll")
                    .unwrap()
                    .as_dictionary_mut()
                    .unwrap()
                    .insert("reverseDirection".into(), true.into());
                edited.to_file_xml(&b_store.config).unwrap();
                wait_for(&b_store, 4).await;
                tokio::time::sleep(Duration::from_millis(2200)).await;
                assert_eq!(a_store.current().unwrap().unwrap().revision, 3);
                a_pump.abort();
                b_pump.abort();
                fs::remove_dir_all(directory).unwrap();
            })
            .await;
    }
}
