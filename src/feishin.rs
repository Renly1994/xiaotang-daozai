//! Feishin's loopback remote protocol. Durations arrive in milliseconds;
//! positions and seeks use seconds, and volume uses the range 0..=100.
use std::{
    io::{self, Cursor},
    net::{Ipv4Addr, SocketAddr, TcpStream},
    sync::{atomic::{AtomicBool, Ordering}, mpsc::{self, Receiver, SyncSender}, Arc, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use tungstenite::{client::IntoClientRequest, handshake::HandshakeError, protocol::WebSocketConfig, Message, WebSocket};
use tungstenite::client::client_with_config;

pub const KEYRING_SERVICE: &str = "Daozai.Feishin";

pub fn get_password(username: &str) -> Option<String> {
    if username.is_empty() { return None; }
    keyring::Entry::new(KEYRING_SERVICE, username).ok()?.get_password().ok()
}

pub fn set_password(username: &str, password: &str) -> Result<(), keyring::Error> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, username)?;
    if password.is_empty() { entry.delete_credential() } else { entry.set_password(password) }
}

const MAX_MESSAGE: usize = 8 * 1024 * 1024;
const MAX_ARTWORK: usize = 4 * 1024 * 1024;
const POLL: Duration = Duration::from_millis(75);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(3);
const STATE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, PartialEq, Eq)]
pub struct Config { pub port: u16, pub username: String, pub password: String }

#[derive(Clone, Debug)]
pub struct NowPlaying {
    pub name: String, pub artist: String, pub album: String, pub source: String,
    pub duration_s: f64, pub position_s: f64, pub sampled_at: Instant,
    pub playing: bool, pub volume: Option<f64>, pub artwork: Option<Arc<Vec<u8>>>,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot { pub connected: bool, pub now_playing: Option<NowPlaying> }

/// 当前 Feishin 歌曲中歌词模块需要的信息：内嵌歌词，以及能用来向同一个音乐服务器取词的封面地址。
#[derive(Clone, Debug)]
pub struct Track {
    pub name: String, pub artist: String, pub album: String, pub id: String,
    pub server_type: String, pub image_url: String, pub lyrics: Option<String>,
}

static TRACK: Mutex<Option<Track>> = Mutex::new(None);

/// 歌词模块读取当前 Feishin 歌曲；Feishin 未连接或未播放时为 `None`。
pub fn track() -> Option<Track> { TRACK.lock().ok().and_then(|track| track.clone()) }

fn remember(track: Option<Track>) {
    if let Ok(mut slot) = TRACK.lock() { *slot = track; }
}

/// Feishin 的歌曲对象只在歌词与服务器信息变化时才包含这些字段；缺省时保留空值。
fn track_from_song(song: &Value) -> Option<Track> {
    let name = song.get("name").and_then(Value::as_str)?;
    Some(Track {
        name: name.into(),
        artist: song.get("artistName").and_then(Value::as_str).unwrap_or("").into(),
        album: song.get("album").and_then(Value::as_str).unwrap_or("").into(),
        id: text_id(song.get("id")).unwrap_or_default(),
        server_type: song.get("_serverType").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase(),
        image_url: song.get("imageUrl").and_then(Value::as_str).unwrap_or("").into(),
        lyrics: song.get("lyrics").and_then(Value::as_str).filter(|raw| !raw.trim().is_empty()).map(str::to_owned),
    })
}

fn text_id(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(id) => Some(id.clone()),
        Value::Number(id) => Some(id.to_string()),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Command { Toggle, Next, Previous, Seek(f64), SetVolume(f64) }
struct Desired { config: Option<Config>, epoch: u64 }
struct Shared { desired: Mutex<Desired>, snapshot: Mutex<Snapshot>, stopped: AtomicBool }

#[derive(Clone)]
pub struct Controller { shared: Arc<Shared>, tx: SyncSender<(u64, Command)> }
impl Controller {
    pub fn send(&self, command: Command) -> bool {
        if self.shared.stopped.load(Ordering::Acquire) { return false; }
        let desired = self.shared.desired.lock().unwrap();
        if desired.config.is_none() || !self.shared.snapshot.lock().unwrap().connected { return false; }
        if matches!(command, Command::Seek(v) | Command::SetVolume(v) if !v.is_finite()) { return false; }
        self.tx.try_send((desired.epoch, command)).is_ok()
    }
}

pub struct Client { shared: Arc<Shared>, controller: Controller, worker: Option<JoinHandle<()>> }
impl Client {
    pub fn start() -> Option<Self> {
        let shared = Arc::new(Shared {
            desired: Mutex::new(Desired { config: None, epoch: 0 }),
            snapshot: Mutex::new(Snapshot::default()), stopped: AtomicBool::new(false),
        });
        let (tx, rx) = mpsc::sync_channel(32);
        let state = shared.clone();
        let worker = crate::resources::FEISHIN.spawn(move || run(state, rx))?;
        Some(Self { controller: Controller { shared: shared.clone(), tx }, shared, worker: Some(worker) })
    }
    pub fn set_config(&self, config: Option<Config>) {
        let mut desired = self.shared.desired.lock().unwrap();
        if desired.config != config {
            desired.config = config;
            desired.epoch = desired.epoch.wrapping_add(1);
            *self.shared.snapshot.lock().unwrap() = Snapshot::default();
            if let Some(worker) = &self.worker { worker.thread().unpark(); }
        }
    }
    pub fn snapshot(&self) -> Snapshot { self.shared.snapshot.lock().unwrap().clone() }
    pub fn send(&self, command: Command) -> bool { self.controller.send(command) }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.shared.stopped.store(true, Ordering::Release);
        *self.shared.snapshot.lock().unwrap() = Snapshot::default();
        remember(None);
        if let Some(worker) = self.worker.take() { worker.thread().unpark(); drop(worker); }
    }
}

fn current(shared: &Shared, epoch: u64) -> bool {
    !shared.stopped.load(Ordering::Acquire) && shared.desired.lock().unwrap().epoch == epoch
}
fn publish(shared: &Shared, epoch: u64, snapshot: &Snapshot) {
    let desired = shared.desired.lock().unwrap();
    if desired.epoch == epoch && !shared.stopped.load(Ordering::Acquire) {
        *shared.snapshot.lock().unwrap() = snapshot.clone();
    }
}
fn wait(shared: &Shared, epoch: u64, duration: Duration) {
    let deadline = Instant::now() + duration;
    while current(shared, epoch) && Instant::now() < deadline {
        thread::park_timeout(Duration::from_millis(25).min(deadline.saturating_duration_since(Instant::now())));
    }
}
fn run(shared: Arc<Shared>, rx: Receiver<(u64, Command)>) {
    let mut last_epoch = u64::MAX;
    let mut backoff = Duration::from_millis(250);
    while !shared.stopped.load(Ordering::Acquire) {
        let (config, epoch) = { let desired = shared.desired.lock().unwrap(); (desired.config.clone(), desired.epoch) };
        if epoch != last_epoch { backoff = Duration::from_millis(250); last_epoch = epoch; }
        let Some(config) = config else { remember(None); wait(&shared, epoch, Duration::from_millis(250)); continue; };
        while rx.try_recv().is_ok() {}
        let mut stable_connection = false;
        if let Some(mut socket) = connect(&config, &shared, epoch) {
            let mut parser = Parser::default();
            let started = Instant::now();
            while current(&shared, epoch) {
                let mut failed = false;
                for _ in 0..32 {
                    let Ok((command_epoch, command)) = rx.try_recv() else { break };
                    if command_epoch == epoch && parser.snapshot.connected {
                        if let Some(message) = control_message(command, parser.playing) {
                            if send(&mut socket, message).is_err() { failed = true; break; }
                        }
                    }
                }
                if failed { break; }
                match socket.read() {
                    Ok(Message::Text(text)) => {
                        if let Some(request) = parser.receive(text.as_str()) { if send(&mut socket, request).is_err() { break; } }
                        stable_connection |= parser.snapshot.connected && started.elapsed() >= Duration::from_secs(10);
                        publish(&shared, epoch, &parser.snapshot);
                    }
                    Ok(Message::Ping(_)) => { if socket.flush().is_err() { break; } }
                    Ok(Message::Close(_)) => break,
                    Ok(_) => {}
                    Err(tungstenite::Error::Io(error)) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
                    Err(_) => break,
                }
                if !parser.snapshot.connected && started.elapsed() > STATE_TIMEOUT { break; }
                if parser.pending_cover.is_some_and(|(_, at)| at.elapsed() > STATE_TIMEOUT) { break; }
            }
        }
        publish(&shared, epoch, &Snapshot::default());
        remember(None);
        while rx.try_recv().is_ok() {}
        if stable_connection { backoff = Duration::from_millis(250); }
        wait(&shared, epoch, backoff);
        backoff = (backoff * 2).min(Duration::from_secs(8));
    }
}
fn connect(config: &Config, shared: &Shared, epoch: u64) -> Option<WebSocket<TcpStream>> {
    if config.port == 0 || config.username.len() > 4096 || config.password.len() > 4096 || config.username.contains(':') { return None; }
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, config.port));
    let stream = TcpStream::connect_timeout(&address, Duration::from_millis(500)).ok()?;
    stream.set_nonblocking(true).ok()?;
    let _ = stream.set_nodelay(true);
    let auth = format!("Basic {}", STANDARD.encode(format!("{}:{}", config.username, config.password)));
    let mut request = format!("ws://127.0.0.1:{}/", config.port).into_client_request().ok()?;
    request.headers_mut().insert("Authorization", auth.parse().ok()?);
    let limits = WebSocketConfig::default().max_message_size(Some(MAX_MESSAGE)).max_frame_size(Some(MAX_MESSAGE)).max_write_buffer_size(256 * 1024);
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    let mut handshake = client_with_config(request, stream, Some(limits));
    let mut socket = loop {
        if !current(shared, epoch) || Instant::now() >= deadline { return None; }
        match handshake {
            Ok((socket, _)) => break socket,
            Err(HandshakeError::Interrupted(mid)) => { thread::park_timeout(Duration::from_millis(10)); handshake = mid.handshake(); }
            Err(HandshakeError::Failure(_)) => return None,
        }
    };
    socket.get_mut().set_nonblocking(false).ok()?;
    socket.get_mut().set_read_timeout(Some(POLL)).ok()?;
    socket.get_mut().set_write_timeout(Some(Duration::from_millis(200))).ok()?;
    send(&mut socket, json!({"event":"authenticate", "header":auth})).ok()?;
    Some(socket)
}
fn send(socket: &mut WebSocket<TcpStream>, value: Value) -> tungstenite::Result<()> { socket.send(Message::Text(value.to_string().into())) }
fn control_message(command: Command, playing: bool) -> Option<Value> {
    Some(match command {
        Command::Toggle => json!({"event": if playing { "pause" } else { "play" }}),
        Command::Next => json!({"event":"next"}),
        Command::Previous => json!({"event":"previous"}),
        Command::Seek(position) if position.is_finite() => json!({"event":"position", "position":position.max(0.0)}),
        Command::SetVolume(volume) if volume.is_finite() => json!({"event":"volume", "volume":volume.clamp(0.0, 100.0)}),
        _ => return None,
    })
}

#[derive(Default)]
struct Parser {
    snapshot: Snapshot, playing: bool, volume: Option<f64>, generation: u64, song_key: String,
    image_url: String, cover_needed: bool, pending_cover: Option<(u64, Instant)>,
}
impl Parser {
    fn receive(&mut self, text: &str) -> Option<Value> {
        if text.len() > MAX_MESSAGE { return None; }
        let Ok(value) = serde_json::from_str::<Value>(text) else { return None };
        let data = value.get("data")?;
        let now = Instant::now();
        match value.get("event")?.as_str()? {
            "state" if data.is_object() => {
                if let Some(song) = data.get("song") { self.song(song, now); }
                if let Some(status) = data.get("status") { self.playback(status, now); }
                if let Some(position) = data.get("position") { self.position(position, now); }
                if let Some(volume) = data.get("volume") { self.set_volume(volume); }
                self.snapshot.connected = true;
            }
            "song" => self.song(data, now),
            "position" => self.position(data, now),
            "playback" => self.playback(data, now),
            "volume" => self.set_volume(data),
            "proxy" => if let Some((generation, _)) = self.pending_cover.take() {
                if generation == self.generation { if let Some(track) = &mut self.snapshot.now_playing { track.artwork = data.as_str().and_then(artwork).map(Arc::new); } }
            },
            "error" => { self.pending_cover = None; }
            _ => return None,
        }
        if self.snapshot.connected && self.cover_needed && self.pending_cover.is_none() {
            self.cover_needed = false;
            self.pending_cover = Some((self.generation, now));
            return Some(json!({"event":"proxy"}));
        }
        None
    }
    fn song(&mut self, song: &Value, now: Instant) {
        if song.is_null() {
            self.snapshot.now_playing = None; self.song_key.clear(); self.image_url.clear();
            self.generation = self.generation.wrapping_add(1); self.cover_needed = false;
            remember(None); return;
        }
        let Some(name) = song.get("name").and_then(Value::as_str) else { return };
        let artist = song.get("artistName").and_then(Value::as_str).unwrap_or("");
        let album = song.get("album").and_then(Value::as_str).unwrap_or("");
        let key = json!([song.get("_serverId"), song.get("id"), name, artist, album]).to_string();
        let image_url = song.get("imageUrl").and_then(Value::as_str).unwrap_or("");
        let changed = self.song_key != key || self.snapshot.now_playing.is_none();
        let cover_changed = changed || self.image_url != image_url;
        if cover_changed { self.generation = self.generation.wrapping_add(1); self.cover_needed = !image_url.is_empty(); }
        self.song_key = key; self.image_url = image_url.into();
        let previous = self.snapshot.now_playing.take();
        let position = if changed { 0.0 } else { previous.as_ref().map_or(0.0, |p| p.position_s) };
        let sampled_at = if changed { now } else { previous.as_ref().map_or(now, |p| p.sampled_at) };
        let cover = if cover_changed { None } else { previous.and_then(|p| p.artwork) };
        self.snapshot.now_playing = Some(NowPlaying {
            name: name.into(), artist: artist.into(),
            album: album.into(), source: "Feishin".into(),
            duration_s: nonnegative(song.get("duration").unwrap_or(&Value::Null)).unwrap_or(0.0) / 1000.0,
            position_s: position, sampled_at, playing: self.playing, volume: self.volume, artwork: cover,
        });
        remember(track_from_song(song));
    }
    fn position(&mut self, data: &Value, now: Instant) {
        if let (Some(position), Some(track)) = (nonnegative(data), &mut self.snapshot.now_playing) { track.position_s = position; track.sampled_at = now; }
    }
    fn playback(&mut self, data: &Value, now: Instant) {
        let playing = match data.as_str() { Some("playing") => true, Some("paused" | "stopped") => false, _ => return };
        if let Some(track) = &mut self.snapshot.now_playing {
            if track.playing != playing {
                if track.playing { track.position_s += now.saturating_duration_since(track.sampled_at).as_secs_f64(); if track.duration_s > 0.0 { track.position_s = track.position_s.min(track.duration_s); } }
                track.sampled_at = now; track.playing = playing;
            }
        }
        self.playing = playing;
    }
    fn set_volume(&mut self, data: &Value) {
        if let Some(volume) = nonnegative(data) { self.volume = Some(volume.min(100.0)); if let Some(track) = &mut self.snapshot.now_playing { track.volume = self.volume; } }
    }
}
fn nonnegative(value: &Value) -> Option<f64> { value.as_f64().filter(|v| v.is_finite() && *v >= 0.0) }
fn artwork(encoded: &str) -> Option<Vec<u8>> {
    if encoded.len() > MAX_ARTWORK.div_ceil(3) * 4 { return None; }
    let bytes = STANDARD.decode(encoded).ok()?;
    if bytes.len() > MAX_ARTWORK { return None; }
    let mut reader = image::ImageReader::new(Cursor::new(&bytes)).with_guessed_format().ok()?;
    let mut limits = image::Limits::default(); limits.max_image_width = Some(4096); limits.max_image_height = Some(4096); limits.max_alloc = Some(32 * 1024 * 1024);
    reader.limits(limits); reader.decode().ok()?; Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> &'static str { r#"{"event":"state","data":{"song":{"id":"a","name":"Song","artistName":"Artist","album":"Album","duration":123000,"imageUrl":"http://server/cover"},"position":12.5,"status":"playing","volume":42}}"# }
    #[test]
    fn exposes_track_for_lyrics_with_server_and_embedded_lyrics() {
        let song = json!({"id":"tr-1","name":"Song","artistName":"Artist","album":"Album","_serverType":"Navidrome","imageUrl":"http://127.0.0.1:4533/rest/getCoverArt.view?id=tr-1&u=u&t=t&s=s","lyrics":"[00:01.00]词"});
        let track = track_from_song(&song).unwrap();
        assert_eq!((&*track.name, &*track.artist, &*track.album, &*track.id, &*track.server_type), ("Song", "Artist", "Album", "tr-1", "navidrome"));
        assert_eq!(track.lyrics.as_deref(), Some("[00:01.00]词"));
        assert_eq!(track_from_song(&json!({"id":7,"name":"Song"})).unwrap().id, "7");
        assert!(track_from_song(&json!({"id":"x"})).is_none());
    }
    #[test]
    fn parses_state_units_and_requests_proxy() {
        let mut parser = Parser::default(); assert_eq!(parser.receive(state()), Some(json!({"event":"proxy"}))); assert!(parser.snapshot.connected);
        let track = parser.snapshot.now_playing.as_ref().unwrap();
        assert_eq!((&*track.name, &*track.artist, &*track.album, &*track.source), ("Song", "Artist", "Album", "Feishin"));
        assert_eq!((track.duration_s, track.position_s, track.playing, track.volume), (123.0, 12.5, true, Some(42.0)));
        assert_eq!(parser.receive(state()), None);
    }
    #[test]
    fn incremental_events_and_null_song() {
        let mut parser = Parser::default(); parser.receive(state()); parser.receive(r#"{"event":"position","data":25}"#); parser.receive(r#"{"event":"playback","data":"paused"}"#); parser.receive(r#"{"event":"volume","data":150}"#);
        let track = parser.snapshot.now_playing.as_ref().unwrap(); assert!(!track.playing); assert!((25.0..26.0).contains(&track.position_s)); assert_eq!(track.volume, Some(100.0));
        parser.receive(r#"{"event":"song","data":null}"#); assert!(parser.snapshot.now_playing.is_none()); assert!(parser.snapshot.connected);
    }
    #[test]
    fn rejects_malformed_and_invalid_fields() {
        let mut parser = Parser::default();
        for text in ["not json", "[]", r#"{"event":"state","data":null}"#, r#"{"event":"state"}"#] { assert!(parser.receive(text).is_none()); assert!(!parser.snapshot.connected); }
        parser.receive(state());
        for text in [r#"{"event":"song","data":{}}"#, r#"{"event":"position","data":-5}"#, r#"{"event":"playback","data":true}"#, r#"{"event":"volume","data":"bad"}"#] { parser.receive(text); }
        let track = parser.snapshot.now_playing.as_ref().unwrap(); assert_eq!(track.name, "Song"); assert_eq!(track.position_s, 12.5); assert!(track.playing); assert_eq!(track.volume, Some(42.0));
    }
    #[test]
    fn maps_controls_and_rejects_nonfinite_values() {
        assert_eq!(control_message(Command::Toggle, true), Some(json!({"event":"pause"}))); assert_eq!(control_message(Command::Toggle, false), Some(json!({"event":"play"})));
        assert_eq!(control_message(Command::Next, false), Some(json!({"event":"next"}))); assert_eq!(control_message(Command::Previous, false), Some(json!({"event":"previous"})));
        assert_eq!(control_message(Command::Seek(12.5), false), Some(json!({"event":"position","position":12.5}))); assert_eq!(control_message(Command::Seek(-1.0), false), Some(json!({"event":"position","position":0.0})));
        assert_eq!(control_message(Command::SetVolume(123.0), false), Some(json!({"event":"volume","volume":100.0}))); assert_eq!(control_message(Command::SetVolume(-2.0), false), Some(json!({"event":"volume","volume":0.0})));
        assert!(control_message(Command::Seek(f64::NAN), false).is_none()); assert!(control_message(Command::SetVolume(f64::INFINITY), false).is_none());
    }
    #[test]
    fn old_cover_is_discarded_before_requesting_current_cover() {
        let mut parser = Parser::default(); parser.receive(state());
        assert!(parser.receive(r#"{"event":"song","data":{"id":"b","name":"Next","imageUrl":"cover-b"}}"#).is_none());
        assert_eq!(parser.receive(r#"{"event":"proxy","data":"old"}"#), Some(json!({"event":"proxy"}))); assert!(parser.snapshot.now_playing.as_ref().unwrap().artwork.is_none()); assert!(parser.pending_cover.is_some());
        parser.receive(r#"{"event":"song","data":null}"#); parser.receive(r#"{"event":"proxy","data":"old"}"#); assert!(parser.snapshot.now_playing.is_none());
    }
    #[test]
    fn cover_requires_a_bounded_valid_image() {
        assert!(artwork("not base64").is_none()); assert!(artwork(&STANDARD.encode(b"not an image")).is_none()); assert!(artwork(&"A".repeat(MAX_ARTWORK.div_ceil(3) * 4 + 4)).is_none());
        let image = image::DynamicImage::new_rgba8(1, 1); let mut bytes = Cursor::new(Vec::new()); image.write_to(&mut bytes, image::ImageFormat::Png).unwrap(); assert_eq!(artwork(&STANDARD.encode(bytes.get_ref())).unwrap(), bytes.into_inner());
    }
}
