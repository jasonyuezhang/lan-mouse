//! A bounded, opt-in file channel on the already authenticated DTLS session.
//! Disk work runs off the input loop; the small send window yields to input.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{self, Read},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, RwLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc;
use webrtc_util::Conn;

pub(crate) const MAGIC: &[u8; 5] = b"LMFD\x02";
const CHUNK: usize = 900;
const MAX_FILE: usize = 64 * 1024 * 1024;
const WINDOW: usize = 32;
type Trust = Arc<RwLock<HashMap<String, String>>>;
static CLAIMS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Offer {
    id: String,
    name: String,
    size: usize,
    hash: String,
}
#[derive(Clone, Deserialize)]
struct Job {
    id: String,
    peer: String,
    source: PathBuf,
    created: f64,
    activate: bool,
}
impl Offer {
    fn valid(&self) -> bool {
        valid_id(&self.id)
            && self.size <= MAX_FILE
            && !self.name.is_empty()
            && self.name.len() <= 255
            && self.name != "."
            && self.name != ".."
            && !self
                .name
                .chars()
                .any(|c| c == '/' || c == '\\' || c.is_control())
            && self.hash.len() == 64
            && self.hash.bytes().all(|c| c.is_ascii_hexdigit())
    }
}
fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|c| c.is_ascii_hexdigit())
}
fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
fn packet(kind: u8, id: &str, data: &[u8]) -> Vec<u8> {
    let mut p = MAGIC.to_vec();
    p.push(kind);
    p.extend_from_slice(id.as_bytes());
    p.extend_from_slice(data);
    p
}
fn private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}
fn atomic_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let temp = path.with_extension("tmp");
    fs::write(&temp, serde_json::to_vec(value)?)?;
    fs::rename(temp, path)
}
fn enabled(root: &Path) -> bool {
    root.join("enabled").is_file()
}
fn job_path(root: &Path, id: &str) -> PathBuf {
    root.join("outgoing").join(format!("{id}.json"))
}
fn status(root: &Path, id: &str, state: &str, progress: f64) {
    let _ = atomic_json(
        &root.join("outgoing").join(format!("{id}.status")),
        &serde_json::json!({"state":state,"progress":progress}),
    );
}
struct Outgoing {
    offer: Offer,
    bytes: Vec<u8>,
    ack: usize,
    ready: bool,
    activated: bool,
    start: Instant,
    last_send: Instant,
    sent_end: usize,
    last_status: Instant,
}
impl Drop for Outgoing {
    fn drop(&mut self) {
        if let Ok(mut claims) = CLAIMS.get_or_init(Default::default).lock() {
            claims.remove(&self.offer.id);
        }
    }
}
fn load_outgoing(root: &Path, peer: &str) -> Option<Outgoing> {
    for entry in fs::read_dir(root.join("outgoing")).ok()?.flatten() {
        if entry.path().extension().is_none_or(|s| s != "json") {
            continue;
        }
        let Ok(data) = fs::read(entry.path()) else {
            continue;
        };
        if data.len() > 4096 {
            continue;
        }
        let Ok(job) = serde_json::from_slice::<Job>(&data) else {
            continue;
        };
        if !valid_id(&job.id)
            || job.peer != peer
            || now() - job.created > 120.
            || job.created > now() + 5.
        {
            continue;
        }
        let mut claims = CLAIMS.get_or_init(Default::default).lock().ok()?;
        if claims.contains(&job.id) {
            continue;
        }
        // Never follow a symlink or open a device/FIFO selected in a stale drag.
        let loaded = (|| -> io::Result<(Offer, Vec<u8>)> {
            let mut file = fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(&job.source)?;
            let meta = file.metadata()?;
            if !meta.is_file() || meta.len() > MAX_FILE as u64 {
                return Err(io::Error::other("Use a regular file up to 64 MB"));
            }
            let mut bytes = Vec::new();
            (&mut file)
                .take(MAX_FILE as u64 + 1)
                .read_to_end(&mut bytes)?;
            let offer = Offer {
                id: job.id.clone(),
                name: job
                    .source
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .into(),
                size: bytes.len(),
                hash: format!("{:x}", Sha256::digest(&bytes)),
            };
            if !offer.valid() {
                return Err(io::Error::other("Unsupported filename or file size"));
            }
            Ok((offer, bytes))
        })();
        match loaded {
            Ok((offer, bytes)) => {
                claims.insert(job.id);
                return Some(Outgoing {
                    offer,
                    bytes,
                    ack: 0,
                    ready: false,
                    activated: false,
                    start: Instant::now(),
                    last_send: Instant::now() - Duration::from_secs(1),
                    sent_end: 0,
                    last_status: Instant::now() - Duration::from_secs(1),
                });
            }
            Err(e) => {
                status(root, &job.id, &format!("error: {e}"), 0.);
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    None
}
struct Incoming {
    offer: Offer,
    bytes: Vec<u8>,
    chunks: Vec<bool>,
    prefix: usize,
    touched: Instant,
    saved: bool,
    activated: bool,
}
impl Incoming {
    fn new(offer: Offer) -> Option<Self> {
        if !offer.valid() {
            return None;
        }
        Some(Self {
            bytes: vec![0; offer.size],
            chunks: vec![false; offer.size.div_ceil(CHUNK)],
            offer,
            prefix: 0,
            touched: Instant::now(),
            saved: false,
            activated: false,
        })
    }
    fn accept(&mut self, offset: usize, bytes: &[u8]) -> bool {
        if self.saved
            || !offset.is_multiple_of(CHUNK)
            || offset >= self.bytes.len()
            || bytes.len() != CHUNK.min(self.bytes.len() - offset)
        {
            return false;
        }
        self.bytes[offset..offset + bytes.len()].copy_from_slice(bytes);
        self.chunks[offset / CHUNK] = true;
        while self.prefix < self.chunks.len() && self.chunks[self.prefix] {
            self.prefix += 1;
        }
        self.touched = Instant::now();
        true
    }
    fn complete(&self) -> bool {
        self.prefix == self.chunks.len()
    }
    fn ack(&self) -> usize {
        (self.prefix * CHUNK).min(self.offer.size)
    }
}
fn save_received(root: &Path, peer: &str, offer: &Offer, bytes: &[u8]) -> io::Result<()> {
    if format!("{:x}", Sha256::digest(bytes)) != offer.hash {
        return Err(io::Error::other("File checksum mismatch"));
    }
    let dir = root.join("received").join(&offer.id);
    private_dir(&dir)?;
    let files = dir.join("files");
    private_dir(&files)?;
    let dest = files.join(&offer.name);
    if dest.exists() {
        return Err(io::Error::other("Transfer ID already exists"));
    }
    let temp = dir.join(".partial");
    fs::write(&temp, bytes)?;
    fs::rename(temp, &dest)?;
    atomic_json(
        &dir.join("ready.json"),
        &serde_json::json!({"id":offer.id,"peer":peer,"path":dest,"created":now()}),
    )
}

pub(crate) fn session(
    conn: Arc<dyn Conn + Send + Sync>,
    trust: Trust,
    peer: String,
    outgoing_allowed: bool,
) -> mpsc::Sender<Vec<u8>> {
    let (tx, rx) = mpsc::channel::<Vec<u8>>(128);
    let Some(home) = std::env::var_os("HOME") else {
        return tx;
    };
    let root = PathBuf::from(home).join(".config/lan-mouse/file-bridge");
    session_at_root(conn, trust, peer, outgoing_allowed, root, tx, rx)
}

fn session_at_root(
    conn: Arc<dyn Conn + Send + Sync>,
    trust: Trust,
    peer: String,
    outgoing_allowed: bool,
    root: PathBuf,
    tx: mpsc::Sender<Vec<u8>>,
    mut rx: mpsc::Receiver<Vec<u8>>,
) -> mpsc::Sender<Vec<u8>> {
    tokio::task::spawn_local(async move {
        let mut outgoing: Option<Outgoing> = None;
        let mut incoming: Option<Incoming> = None;
        let mut last_hello = None::<Instant>;
        let mut last_scan = Instant::now() - Duration::from_secs(1);
        let mut tick = tokio::time::interval(Duration::from_millis(20));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    if !trust.read().is_ok_and(|t| t.contains_key(&peer)) { outgoing=None;incoming=None;last_hello=None;continue; }
                    if last_scan.elapsed() >= Duration::from_millis(200) {
                        last_scan=Instant::now();
                        let r=root.clone();
                        if !tokio::task::spawn_blocking(move || enabled(&r)).await.unwrap_or(false) { outgoing=None;incoming=None;last_hello=None;continue; }
                        if conn.send(&[MAGIC.as_slice(), &[0]].concat()).await.is_err() { break; }
                        if last_hello.is_some_and(|t| t.elapsed()<Duration::from_secs(2)) && outgoing_allowed && outgoing.is_none() {
                            let r=root.clone();let p=peer.clone();outgoing=tokio::task::spawn_blocking(move || load_outgoing(&r,&p)).await.ok().flatten();
                        }
                        if let Some(send)=&mut outgoing {
                            let r=root.clone();let id=send.offer.id.clone();
                            let job=tokio::task::spawn_blocking(move || fs::read(job_path(&r,&id)).ok().and_then(|b| serde_json::from_slice::<Job>(&b).ok())).await.ok().flatten();
                            if let Some(job)=job.filter(|_| send.start.elapsed()<Duration::from_secs(120)) { send.activated=job.activate; }
                            else { let _=conn.send(&packet(5,&send.offer.id,&[])).await; outgoing=None; }
                        }
                    }
                    if incoming.as_ref().is_some_and(|r| r.touched.elapsed()>Duration::from_secs(120)) { incoming=None; }
                    if !last_hello.is_some_and(|t| t.elapsed()<Duration::from_secs(2)) { continue; }
                    if let Some(send)=&mut outgoing {
                        if send.ready {
                            if send.activated { let _=conn.send(&packet(4,&send.offer.id,&[])).await; }
                            continue;
                        }
                        if send.last_send.elapsed()<Duration::from_millis(200) && send.ack<send.sent_end { continue; }
                        // Re-offering makes lost metadata and receiver restarts recoverable.
                        let metadata=serde_json::to_vec(&send.offer).unwrap();
                        if conn.send(&[MAGIC.as_slice(),&[1],&metadata].concat()).await.is_err() { break; }
                        let end=(send.ack+WINDOW*CHUNK).min(send.bytes.len());
                        for offset in (send.ack..end).step_by(CHUNK) {
                            let next=(offset+CHUNK).min(end);let mut payload=(offset as u32).to_be_bytes().to_vec();payload.extend_from_slice(&send.bytes[offset..next]);
                            if conn.send(&packet(2,&send.offer.id,&payload)).await.is_err() { return; }
                            tokio::task::yield_now().await;
                        }
                        send.last_send=Instant::now();send.sent_end=end;
                    }
                }
                data = rx.recv() => {
                    let Some(data)=data else { break; };
                    if !trust.read().is_ok_and(|t| t.contains_key(&peer)) { continue; }
                    if data.get(5)==Some(&0) && data.len()==6 {
                        let r=root.clone();if tokio::task::spawn_blocking(move || enabled(&r)).await.unwrap_or(false) { last_hello=Some(Instant::now()); } continue;
                    }
                    if !last_hello.is_some_and(|t| t.elapsed()<Duration::from_secs(2)) { continue; }
                    if data.get(5)==Some(&1) && data.len()<=1024 {
                        if let Ok(offer)=serde_json::from_slice::<Offer>(&data[6..]) {
                            if incoming.as_ref().is_some_and(|i| i.offer.id==offer.id) {
                                let r=incoming.as_mut().unwrap();r.touched=Instant::now();
                                if r.saved { let _=conn.send(&packet(3,&r.offer.id,&u32::MAX.to_be_bytes())).await; }
                            } else if incoming.as_ref().is_none_or(|i| i.saved) { incoming=Incoming::new(offer); }
                        }
                    } else if data.len()>=38 {
                        let Ok(id)=std::str::from_utf8(&data[6..38]) else { continue; };if !valid_id(id) { continue; }
                        match data[5] {
                            2 if data.len()>=42 => {
                                if let Some(r)=incoming.as_mut().filter(|r| r.offer.id==id) {
                                    let offset=u32::from_be_bytes(data[38..42].try_into().unwrap()) as usize;
                                    if r.accept(offset,&data[42..]) { let _=conn.send(&packet(3,id,&(r.ack() as u32).to_be_bytes())).await; }
                                }
                            }
                            3 if data.len()==42 => {
                                if let Some(s)=outgoing.as_mut().filter(|s| s.offer.id==id) {
                                    let ack=u32::from_be_bytes(data[38..42].try_into().unwrap());
                                    if ack==u32::MAX { s.ready=true; }
                                    else if (ack as usize)<=s.bytes.len() && (ack as usize==s.bytes.len() || (ack as usize).is_multiple_of(CHUNK)) { s.ack=s.ack.max(ack as usize); }
                                    // Advance a completed window without waiting for the idle
                                    // 20 ms tick. Keep bursts at least 2 ms apart and retain
                                    // the bounded window/yielding so input stays responsive.
                                    if !s.ready && s.ack >= s.sent_end && s.ack < s.bytes.len() {
                                        tick.reset_at(tokio::time::Instant::from_std(s.last_send + Duration::from_millis(2)));
                                    }
                                    if s.ready || s.last_status.elapsed() >= Duration::from_millis(100) {
                                        s.last_status=Instant::now();
                                        let root=root.clone();let id=id.to_string();let state=if s.ready {"ready"} else {"sending"};let progress=s.ack as f64/s.bytes.len().max(1) as f64;
                                        let _=tokio::task::spawn_blocking(move || status(&root,&id,state,progress)).await;
                                    }
                                }
                            }
                            4 if data.len()==38 => {
                                if let Some(r)=incoming.as_mut().filter(|r| r.offer.id==id && r.saved) {
                                    if !r.activated {
                                        let path=root.join("received").join(id).join("activate");
                                        if tokio::task::spawn_blocking(move || fs::write(path,now().to_string())).await.is_ok_and(|r| r.is_ok()) { r.activated=true; }
                                    }
                                    if r.activated { let _=conn.send(&packet(6,id,&[])).await; }
                                }
                            }
                            5 if data.len()==38 => { if incoming.as_ref().is_some_and(|r| r.offer.id==id) { incoming=None; } }
                            7 if data.len()<=338 && outgoing.as_ref().is_some_and(|s| s.offer.id==id) => {
                                let root=root.clone();let id=id.to_owned();
                                let _=tokio::task::spawn_blocking(move || { status(&root,&id,"error: The other Mac could not save this file. Check its free disk space.",0.);let _=fs::remove_file(job_path(&root,&id)); }).await;
                                outgoing=None;
                            }
                            6 if data.len()==38 && outgoing.as_ref().is_some_and(|s| s.offer.id==id) => {
                                let root=root.clone();let id=id.to_owned();let _=tokio::task::spawn_blocking(move || {status(&root,&id,"activated",1.);let _=fs::remove_file(job_path(&root,&id));}).await;outgoing=None;
                            }
                            _=>{}
                        }
                    }
                    if let Some(r)=incoming.as_mut().filter(|r| !r.saved && r.complete()) {
                        let root=root.clone();let peer=peer.clone();let offer=r.offer.clone();let bytes=std::mem::take(&mut r.bytes);
                        match tokio::task::spawn_blocking(move || save_received(&root,&peer,&offer,&bytes)).await {
                            Ok(Ok(()))=>{r.saved=true;let _=conn.send(&packet(3,&r.offer.id,&u32::MAX.to_be_bytes())).await;}
                            error=>{log::warn!("file transfer could not be saved: {error:?}");let _=conn.send(&packet(7,&r.offer.id,&[])).await;incoming=None;}
                        }
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
    fn offer(data: &[u8]) -> Offer {
        Offer {
            id: "0123456789abcdef0123456789abcdef".into(),
            name: "an image.png".into(),
            size: data.len(),
            hash: format!("{:x}", Sha256::digest(data)),
        }
    }
    #[test]
    fn file_reassembly_handles_loss_duplicates_and_reordering() {
        let bytes = vec![42; CHUNK * 2 + 7];
        let mut r = Incoming::new(offer(&bytes)).unwrap();
        assert!(r.accept(CHUNK, &bytes[CHUNK..CHUNK * 2]));
        assert!(r.accept(CHUNK, &bytes[CHUNK..CHUNK * 2]));
        assert_eq!(r.ack(), 0);
        assert!(!r.accept(1, &bytes[..CHUNK]));
        assert!(!r.accept(0, &bytes[..3]));
        assert!(r.accept(0, &bytes[..CHUNK]));
        assert_eq!(r.ack(), CHUNK * 2);
        assert!(!r.complete());
        assert!(r.accept(CHUNK * 2, &bytes[CHUNK * 2..]));
        assert!(r.complete());
        assert_eq!(r.bytes, bytes);
    }
    #[test]
    fn remote_names_sizes_and_ids_are_bounded() {
        for name in ["../escape", "..", "/tmp/file", "a/b", "a\\b", "a\0b", ""] {
            let mut o = offer(b"x");
            o.name = name.into();
            assert!(!o.valid());
        }
        let mut o = offer(b"");
        assert!(o.valid());
        assert!(Incoming::new(o.clone()).unwrap().complete());
        o.size = MAX_FILE + 1;
        assert!(!o.valid());
        o = offer(b"x");
        o.id = "../escape".into();
        assert!(!o.valid());
    }
    #[test]
    fn received_file_is_verified_and_never_overwrites() {
        let root = std::env::temp_dir().join(format!("lan-mouse-file-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let o = offer(b"hello");
        assert!(save_received(&root, "peer", &o, b"wrong").is_err());
        save_received(&root, "peer", &o, b"hello").unwrap();
        assert_eq!(
            fs::read(
                root.join("received")
                    .join(&o.id)
                    .join("files")
                    .join(&o.name)
            )
            .unwrap(),
            b"hello"
        );
        assert!(save_received(&root, "peer", &o, b"hello").is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn sessions_require_trust_and_retry_missing_file_and_activation_packets() {
        tokio::task::LocalSet::new().run_until(async {
            let root=std::env::temp_dir().join(format!("lan-mouse-file-session-{}",std::process::id()));let _=fs::remove_dir_all(&root);
            let a_root=root.join("a");let b_root=root.join("b");
            for root in [&a_root,&b_root] {private_dir(&root.join("outgoing")).unwrap();fs::write(root.join("enabled"),b"").unwrap();}
            let bytes=vec![7;CHUNK*70+19];let source=root.join("sample file.txt");fs::write(&source,&bytes).unwrap();
            let id="11111111111111111111111111111111";
            let job=serde_json::json!({"id":id,"peer":"b","source":source,"created":now(),"activate":false});
            atomic_json(&job_path(&a_root,id),&job).unwrap();
            let a_trust:Trust=Default::default();let b_trust:Trust=Arc::new(RwLock::new(HashMap::from([("a".into(),"Mac A".into())])));
            let (a,b)=webrtc_util::conn::conn_pipe::pipe();let a=Arc::new(a);let b=Arc::new(b);
            let (a_tx,a_rx)=mpsc::channel(128);let (b_tx,b_rx)=mpsc::channel(128);
            let a_tx=session_at_root(a.clone(),a_trust.clone(),"b".into(),true,a_root.clone(),a_tx,a_rx);
            let b_tx=session_at_root(b.clone(),b_trust,"a".into(),false,b_root.clone(),b_tx,b_rx);
            let pump=|conn:Arc<dyn Conn+Send+Sync>,tx:mpsc::Sender<Vec<u8>>| tokio::task::spawn_local(async move {
                let mut seen=HashSet::new();let mut buf=[0;1024];
                while let Ok(n)=conn.recv(&mut buf).await {
                    if [2,3,4,6].contains(&buf[5]) && seen.insert(buf[5]) {continue;}
                    if tx.send(buf[..n].to_vec()).await.is_err(){break;}
                }
            });
            let pa=pump(a,a_tx);let pb=pump(b,b_tx);
            tokio::time::sleep(Duration::from_millis(500)).await;
            assert!(!b_root.join("received").exists());a_trust.write().unwrap().insert("b".into(),"Mac B".into());
            let received=b_root.join("received").join(id);let state=a_root.join("outgoing").join(format!("{id}.status"));
            tokio::time::timeout(Duration::from_secs(8),async {loop {
                if fs::read(&state).ok().and_then(|b|serde_json::from_slice::<serde_json::Value>(&b).ok()).is_some_and(|v|v["state"]=="ready") {break;}
                tokio::time::sleep(Duration::from_millis(50)).await;
            }}).await.unwrap();
            assert_eq!(fs::read(received.join("files/sample file.txt")).unwrap(),bytes);
            assert!(!received.join("activate").exists());
            let mut job=job;job["activate"]=serde_json::json!(true);atomic_json(&job_path(&a_root,id),&job).unwrap();
            tokio::time::timeout(Duration::from_secs(5),async {while job_path(&a_root,id).exists(){tokio::time::sleep(Duration::from_millis(50)).await;}}).await.unwrap();
            assert!(received.join("activate").exists());
            pa.abort();pb.abort();fs::remove_dir_all(root).unwrap();
        }).await;
    }
}
