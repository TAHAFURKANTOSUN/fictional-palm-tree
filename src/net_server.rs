//! Çok oyunculu sunucu ve WebSocket + HTTP taşıma katmanı.
//!
//! Harici crate kullanmadan yalnızca Rust standart kütüphanesi (`std::net`)
//! ile çalışır:
//! * Aynı TCP portu üzerinden hem oyunun statik web dosyalarını (`index.html`,
//!   `pkg/*`, `voxelcraft-rs.html`) HTTP ile sunar, hem de `Upgrade: websocket`
//!   isteklerini RFC 6455 WebSocket bağlantısına yükseltir.
//! * WebSocket ikili (binary) çerçeveleri içinden [`crate::protocol`] paketlerini
//!   (`Handshaking` → `Status` / `Login` → `Configuration` → `Play`) geçirir.
//! * Dünyayı ([`World`]), tarımı ([`Farming`]) ve yaratıkları ([`Mobs`]) sunucu
//!   tarafında 20 Hz hızında ilerletir; blok değişikliklerini, oyuncu konumlarını,
//!   sohbet iletilerini ve yaratık hareketlerini bağlı tüm istemcilere yayınlar.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::blocks::{is_crop, AIR, BONE, FARMLAND, HOE, SEEDS, TORCH, WATER, WHEAT_0};
use crate::farming::Farming;
use crate::mobs::{MobEvent, Mobs};
use crate::protocol::{
    status_json, Angle, Connection, Decoder, Direction, Face, Intent, Packet, Position, State,
};
use crate::world::{World, CHUNK_SIZE};

const DAY_LENGTH: f64 = 600.0;
const MAX_PLAYERS: u32 = 32;
const SERVER_VERSION: &str = "VoxelCraft 1.21";
const SERVER_MOTD: &str = "VoxelCraft Çok Oyunculu Sunucu — Tarım & Yaratıklar";

// --- Saf Rust SHA-1 ve Base64 (RFC 6455 Sec-WebSocket-Accept için) ----------

pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h0: u32 = 0x6745_2301;
    let mut h1: u32 = 0xEFCD_AB89;
    let mut h2: u32 = 0x98BA_DCFE;
    let mut h3: u32 = 0x1032_5476;
    let mut h4: u32 = 0xC3D2_E1F0;

    let bit_len = (data.len() as u64) * 8;
    let mut msg = data.to_vec();
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let (mut a, mut b, mut c, mut d, mut e) = (h0, h1, h2, h3, h4);
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }

    let mut out = [0u8; 20];
    out[0..4].copy_from_slice(&h0.to_be_bytes());
    out[4..8].copy_from_slice(&h1.to_be_bytes());
    out[8..12].copy_from_slice(&h2.to_be_bytes());
    out[12..16].copy_from_slice(&h3.to_be_bytes());
    out[16..20].copy_from_slice(&h4.to_be_bytes());
    out
}

pub fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

pub fn ws_accept_key(client_key: &str) -> String {
    let mut input = String::from(client_key.trim());
    input.push_str("258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    base64_encode(&sha1(input.as_bytes()))
}

// --- RFC 6455 WebSocket çerçeveleme -----------------------------------------

/// Sunucudan istemciye (maskesiz) ikili WebSocket çerçevesi üretir.
pub fn encode_ws_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(10 + payload.len());
    out.push(0x80 | (opcode & 0x0F));
    if payload.len() < 126 {
        out.push(payload.len() as u8);
    } else if payload.len() <= 65535 {
        out.push(126);
        out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    } else {
        out.push(127);
        out.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    }
    out.extend_from_slice(payload);
    out
}

/// İstemciden sunucuya (maskeli) veya sunucudan istemciye WebSocket çerçevesi üretir.
pub fn encode_ws_masked_frame(opcode: u8, payload: &[u8], mask: [u8; 4]) -> Vec<u8> {
    let mut out = Vec::with_capacity(14 + payload.len());
    out.push(0x80 | (opcode & 0x0F));
    if payload.len() < 126 {
        out.push(0x80 | (payload.len() as u8));
    } else if payload.len() <= 65535 {
        out.push(0x80 | 126);
        out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    } else {
        out.push(0x80 | 127);
        out.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    }
    out.extend_from_slice(&mask);
    for (i, &b) in payload.iter().enumerate() {
        out.push(b ^ mask[i & 3]);
    }
    out
}

/// Tampondan tek bir WebSocket çerçevesi ayıklar: `Some((opcode, payload, consumed_bytes))`.
pub fn try_decode_ws_frame(buf: &[u8]) -> Result<Option<(u8, Vec<u8>, usize)>, &'static str> {
    if buf.len() < 2 {
        return Ok(None);
    }
    let opcode = buf[0] & 0x0F;
    let masked = (buf[1] & 0x80) != 0;
    let mut len = (buf[1] & 0x7F) as usize;
    let mut idx = 2;
    if len == 126 {
        if buf.len() < 4 {
            return Ok(None);
        }
        len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
        idx = 4;
    } else if len == 127 {
        if buf.len() < 10 {
            return Ok(None);
        }
        let mut arr = [0u8; 8];
        arr.copy_from_slice(&buf[2..10]);
        let l64 = u64::from_be_bytes(arr);
        if l64 > 4 * 1024 * 1024 {
            return Err("WebSocket çerçevesi çok büyük");
        }
        len = l64 as usize;
        idx = 10;
    }
    let mask = if masked {
        if buf.len() < idx + 4 {
            return Ok(None);
        }
        let m = [buf[idx], buf[idx + 1], buf[idx + 2], buf[idx + 3]];
        idx += 4;
        Some(m)
    } else {
        None
    };
    if buf.len() < idx + len {
        return Ok(None);
    }
    let mut payload = buf[idx..idx + len].to_vec();
    if let Some(m) = mask {
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= m[i & 3];
        }
    }
    Ok(Some((opcode, payload, idx + len)))
}

// --- Bağlı İstemci Durumu ---------------------------------------------------

struct ClientPeer {
    id: i32,
    stream: Option<TcpStream>,
    raw_in: Vec<u8>,
    raw_out: Vec<u8>,
    decoder: Decoder,
    conn: Connection,
    name: String,
    pos: [f64; 3],
    yaw: f32,
    pitch: f32,
    closed: bool,
    last_seen: Instant,
}

impl ClientPeer {
    fn send_packet(&mut self, packet: &Packet) {
        if self.closed {
            return;
        }
        if let Ok(bytes) = packet.encode(None) {
            self.send_raw_ws(0x02, &bytes);
        }
    }

    fn send_raw_ws(&mut self, opcode: u8, payload: &[u8]) {
        if self.closed {
            return;
        }
        let frame = encode_ws_frame(opcode, payload);
        if let Some(stream) = &mut self.stream {
            if stream.write_all(&frame).is_err() {
                self.closed = true;
            }
        } else {
            self.raw_out.extend_from_slice(&frame);
        }
    }
}

// --- Çok Oyunculu Oyun Sunucusu ---------------------------------------------

pub struct GameServer {
    pub seed: i32,
    pub world: World,
    pub farming: Farming,
    pub mobs: Mobs,
    pub day_time: f64,
    pub edits: HashMap<(i32, i32, i32), u8>,
    pub torches: Vec<[i32; 3]>,
    clients: Vec<ClientPeer>,
    next_player_id: i32,
    known_mobs: HashMap<i32, ([f64; 3], f64, bool)>,
    keepalive_timer: f64,
}

impl GameServer {
    pub fn new(seed: i32) -> Self {
        let mut world = World::new(seed);
        let spawn = world.find_spawn();
        world.ensure_around(spawn[0], spawn[2]);
        Self {
            seed,
            world,
            farming: Farming::default(),
            mobs: Mobs::default(),
            day_time: 0.22,
            edits: HashMap::new(),
            torches: Vec::new(),
            clients: Vec::new(),
            next_player_id: 1,
            known_mobs: HashMap::new(),
            keepalive_timer: 5.0,
        }
    }

    pub fn online_count(&self) -> u32 {
        self.clients
            .iter()
            .filter(|c| !c.closed && c.conn.state == State::Play)
            .count() as u32
    }

    /// Yükseltilmiş (WebSocket el sıkışması tamamlanmış) bir TCP akışını sunucuya ekler.
    pub fn add_websocket_client(&mut self, stream: TcpStream) -> i32 {
        let _ = stream.set_nonblocking(true);
        let _ = stream.set_nodelay(true);
        self.add_peer(Some(stream))
    }

    /// Bellek içi (soketsiz) WebSocket istemcisi ekler (birim testleri ve yerel köprü için).
    pub fn add_memory_client(&mut self) -> i32 {
        self.add_peer(None)
    }

    fn add_peer(&mut self, stream: Option<TcpStream>) -> i32 {
        let id = self.next_player_id;
        self.next_player_id += 1;
        let spawn = self.world.find_spawn();
        self.clients.push(ClientPeer {
            id,
            stream,
            raw_in: Vec::with_capacity(4096),
            raw_out: Vec::new(),
            decoder: Decoder::default(),
            conn: Connection::default(),
            name: format!("Oyuncu{id}"),
            pos: spawn,
            yaw: 0.0,
            pitch: 0.0,
            closed: false,
            last_seen: Instant::now(),
        });
        id
    }

    /// Bellek içi istemciye ham WebSocket çerçevesi besler.
    pub fn push_client_ws_bytes(&mut self, client_id: i32, bytes: &[u8]) {
        if let Some(c) = self.clients.iter_mut().find(|c| c.id == client_id && !c.closed) {
            c.raw_in.extend_from_slice(bytes);
            c.last_seen = Instant::now();
        }
    }

    /// Bellek içi istemcinin giden WebSocket çerçevelerini alır.
    pub fn take_client_ws_bytes(&mut self, client_id: i32) -> Vec<u8> {
        if let Some(c) = self.clients.iter_mut().find(|c| c.id == client_id) {
            std::mem::take(&mut c.raw_out)
        } else {
            Vec::new()
        }
    }

    /// Gelen TCP bağlantısının ilk HTTP başlığını okur:
    /// * `Upgrade: websocket` ise RFC 6455 yanıtını gönderip istemciyi çok oyunculu
    ///   oturuma ekler.
    /// * Normal HTTP `GET` ise çalışma dizinindeki statik dosyayı (`index.html`, `pkg/*`) sunar.
    pub fn accept_http_or_ws(&mut self, mut stream: TcpStream, root_dir: &Path) {
        let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
        let _ = stream.set_write_timeout(Some(Duration::from_millis(2000)));
        let mut buf = [0u8; 4096];
        let mut req = Vec::new();
        loop {
            match stream.read(&mut buf) {
                Ok(0) => return,
                Ok(n) => {
                    req.extend_from_slice(&buf[..n]);
                    if req.windows(4).any(|w| w == b"\r\n\r\n") || req.len() >= 8192 {
                        break;
                    }
                }
                Err(ref e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {
                    break;
                }
                Err(_) => return,
            }
        }
        let Ok(text) = std::str::from_utf8(&req) else { return };
        let mut is_ws = false;
        let mut ws_key = None;
        let mut first_line = "";
        for (i, line) in text.lines().enumerate() {
            if i == 0 {
                first_line = line;
            }
            if let Some((k, v)) = line.split_once(':') {
                if k.trim().eq_ignore_ascii_case("upgrade")
                    && v.trim().to_ascii_lowercase().contains("websocket")
                {
                    is_ws = true;
                }
                if k.trim().eq_ignore_ascii_case("sec-websocket-key") {
                    ws_key = Some(v.trim().to_string());
                }
            }
        }

        if is_ws {
            if let Some(key) = ws_key {
                let accept = ws_accept_key(&key);
                let resp = format!(
                    "HTTP/1.1 101 Switching Protocols\r\n\
                     Upgrade: websocket\r\n\
                     Connection: Upgrade\r\n\
                     Sec-WebSocket-Accept: {accept}\r\n\r\n"
                );
                if stream.write_all(resp.as_bytes()).is_ok() {
                    self.add_websocket_client(stream);
                }
            }
            return;
        }

        // Normal HTTP GET isteği -> statik dosya sun.
        serve_static_http(&mut stream, first_line, root_dir);
    }

    /// Bağlı tüm istemcilerden gelen paketleri okur ve simülasyonu `dt` saniye ilerletir.
    pub fn tick(&mut self, dt: f64) {
        self.poll_clients();
        self.step_world(dt);
    }

    fn poll_clients(&mut self) {
        let mut buf = [0u8; 8192];
        let mut incoming: Vec<(usize, Packet)> = Vec::new();

        for (idx, client) in self.clients.iter_mut().enumerate() {
            if client.closed {
                continue;
            }
            if let Some(stream) = &mut client.stream {
                loop {
                    match stream.read(&mut buf) {
                        Ok(0) => {
                            client.closed = true;
                            break;
                        }
                        Ok(n) => {
                            client.raw_in.extend_from_slice(&buf[..n]);
                            client.last_seen = Instant::now();
                        }
                        Err(ref e) if e.kind() == ErrorKind::WouldBlock => break,
                        Err(_) => {
                            client.closed = true;
                            break;
                        }
                    }
                }
            }

            let mut pings = Vec::new();
            let mut close = false;
            loop {
                match try_decode_ws_frame(&client.raw_in) {
                    Ok(Some((opcode, payload, consumed))) => {
                        client.raw_in.drain(..consumed);
                        match opcode {
                            0x01 | 0x02 | 0x00 => client.decoder.push(&payload),
                            0x08 => {
                                close = true;
                                break;
                            }
                            0x09 => pings.push(payload),
                            _ => {}
                        }
                    }
                    Ok(None) => break,
                    Err(_) => {
                        close = true;
                        break;
                    }
                }
            }
            for ping in pings {
                client.send_raw_ws(0x0A, &ping);
            }
            if close {
                client.closed = true;
                continue;
            }

            loop {
                match client.decoder.next_packet() {
                    Ok(Some((pkt_id, body))) => {
                        let state = client.conn.state;
                        match Packet::decode(state, Direction::Serverbound, pkt_id, &body) {
                            Ok(pkt) => {
                                if client.conn.handle(&pkt).is_err() {
                                    client.closed = true;
                                    break;
                                }
                                incoming.push((idx, pkt));
                            }
                            Err(_) => {
                                client.closed = true;
                                break;
                            }
                        }
                    }
                    Ok(None) => break,
                    Err(_) => {
                        client.closed = true;
                        break;
                    }
                }
            }
        }

        for (idx, pkt) in incoming {
            self.handle_packet(idx, pkt);
        }

        // Bağlantısı kopan Play oyuncularını diğerlerine bildir.
        let mut removed_ids = Vec::new();
        let mut leave_msgs = Vec::new();
        self.clients.retain(|c| {
            if c.closed {
                if c.conn.state == State::Play {
                    removed_ids.push(c.id);
                    leave_msgs.push(format!("{} oyundan ayrıldı.", c.name));
                }
                false
            } else {
                true
            }
        });
        if !removed_ids.is_empty() {
            self.broadcast_play(&Packet::RemoveEntities { entity_ids: removed_ids }, None);
            for msg in leave_msgs {
                self.broadcast_play(
                    &Packet::SystemChat { sender: "Sunucu".into(), message: msg },
                    None,
                );
            }
        }
    }

    fn handle_packet(&mut self, idx: usize, pkt: Packet) {
        if idx >= self.clients.len() || self.clients[idx].closed {
            return;
        }
        match pkt {
            Packet::Handshake { intent, .. } => {
                if intent == Intent::Transfer {
                    self.clients[idx].closed = true;
                }
            }
            Packet::StatusRequest => {
                let online = self.online_count();
                let json = status_json(SERVER_VERSION, online, MAX_PLAYERS, SERVER_MOTD);
                self.clients[idx].send_packet(&Packet::StatusResponse { json });
            }
            Packet::Ping { payload } => {
                self.clients[idx].send_packet(&Packet::Pong { payload });
            }
            Packet::LoginStart { name } => {
                let clean: String = name
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(16)
                    .collect();
                let clean = if clean.is_empty() {
                    format!("Oyuncu{}", self.clients[idx].id)
                } else {
                    clean
                };
                self.clients[idx].name = clean.clone();
                self.clients[idx].send_packet(&Packet::LoginSuccess { name: clean });
            }
            Packet::LoginAcknowledged => {
                // İstemci Configuration durumuna geçti -> sunucu da FinishConfiguration gönderir.
                self.clients[idx].send_packet(&Packet::FinishConfiguration);
            }
            Packet::AcknowledgeFinishConfiguration => {
                // İstemci Play durumuna geçti!
                let pid = self.clients[idx].id;
                let pname = self.clients[idx].name.clone();
                let ppos = self.clients[idx].pos;
                let pyaw = self.clients[idx].yaw;
                let ppitch = self.clients[idx].pitch;

                self.world.ensure_around(ppos[0], ppos[2]);

                // 1. JoinGame (oyuncu kimliği, dünya tohumu, gün saati)
                self.clients[idx].send_packet(&Packet::JoinGame {
                    player_id: pid,
                    seed: self.seed,
                    day_time: self.day_time as f32,
                });

                // 2. Dünyada şimdiye kadar değiştirilmiş tüm bloklar
                let edits: Vec<((i32, i32, i32), u8)> =
                    self.edits.iter().map(|(&k, &v)| (k, v)).collect();
                for ((x, y, z), b) in edits {
                    let pos = Position { x, y, z };
                    if pos.in_range() {
                        self.clients[idx].send_packet(&Packet::BlockUpdate {
                            pos,
                            block: b as i32,
                        });
                    }
                }

                // 3. Mevcut diğer oyuncuları yeni oyuncuya gönder, yeni oyuncuyu da diğerlerine
                let others: Vec<(i32, String, [f64; 3], f32, f32)> = self
                    .clients
                    .iter()
                    .enumerate()
                    .filter(|&(i, c)| i != idx && !c.closed && c.conn.state == State::Play)
                    .map(|(_, c)| (c.id, c.name.clone(), c.pos, c.yaw, c.pitch))
                    .collect();
                for (oid, oname, opos, oyaw, opitch) in others {
                    self.clients[idx].send_packet(&Packet::SpawnPlayer {
                        player_id: oid,
                        name: oname,
                        x: opos[0],
                        y: opos[1],
                        z: opos[2],
                        yaw: Angle::from_radians(oyaw as f64),
                        pitch: Angle::from_radians(opitch as f64),
                    });
                }
                self.broadcast_play(
                    &Packet::SpawnPlayer {
                        player_id: pid,
                        name: pname.clone(),
                        x: ppos[0],
                        y: ppos[1],
                        z: ppos[2],
                        yaw: Angle::from_radians(pyaw as f64),
                        pitch: Angle::from_radians(ppitch as f64),
                    },
                    Some(pid),
                );

                // 4. Mevcut canlıları yeni oyuncuya gönder
                let mobs_snapshot: Vec<(i32, &'static str, [f64; 3], f64)> = self
                    .mobs
                    .list
                    .iter()
                    .map(|m| (m.net_id, m.kind.identifier(), m.body.pos, m.yaw))
                    .collect();
                for (mid, kind, mpos, myaw) in mobs_snapshot {
                    self.clients[idx].send_packet(&Packet::SpawnEntity {
                        entity_id: mid,
                        kind: kind.to_string(),
                        x: mpos[0],
                        y: mpos[1],
                        z: mpos[2],
                        pitch: Angle(0),
                        yaw: Angle::from_radians(myaw),
                    });
                }

                // 5. Katılım duyurusu
                self.broadcast_play(
                    &Packet::SystemChat {
                        sender: "Sunucu".into(),
                        message: format!("{pname} oyuna katıldı!"),
                    },
                    None,
                );
            }
            Packet::PlayerPosition { x, y, z, yaw, pitch, .. } => {
                if !x.is_finite() || !y.is_finite() || !z.is_finite() {
                    return;
                }
                let pid = self.clients[idx].id;
                self.clients[idx].pos = [x, y, z];
                self.clients[idx].yaw = yaw;
                self.clients[idx].pitch = pitch;
                self.world.ensure_around(x, z);
                self.broadcast_play(
                    &Packet::MoveEntity {
                        entity_id: pid,
                        x,
                        y,
                        z,
                        yaw: Angle::from_radians(yaw as f64),
                        pitch: Angle::from_radians(pitch as f64),
                        hurt: false,
                    },
                    Some(pid),
                );
            }
            Packet::PlayerAction { pos, block } => {
                if !(0..=255).contains(&block) {
                    return;
                }
                let b = block as u8;
                self.apply_block_change(pos.x, pos.y, pos.z, b);
                // Suyun yayılması (basit komşu su kontrolü)
                for (dx, dy, dz) in [
                    (-1, 0, 0),
                    (1, 0, 0),
                    (0, 0, -1),
                    (0, 0, 1),
                    (0, 1, 0),
                ] {
                    if b == 0
                        && self.world.get_block(pos.x + dx, pos.y + dy, pos.z + dz) == WATER
                    {
                        self.apply_block_change(pos.x, pos.y, pos.z, WATER);
                        break;
                    }
                }
            }
            Packet::UseItemOn { pos, face, item_id } => {
                self.world
                    .ensure_chunk(pos.x.div_euclid(CHUNK_SIZE), pos.z.div_euclid(CHUNK_SIZE));
                let item = item_id.clamp(0, u16::MAX as i32) as u16;
                if item == HOE {
                    let had_grass = self.world.get_block(pos.x, pos.y + 1, pos.z);
                    if Farming::till(&mut self.world, pos.x, pos.y, pos.z) {
                        if had_grass != AIR && self.world.get_block(pos.x, pos.y + 1, pos.z) == AIR
                        {
                            self.record_and_broadcast_block(pos.x, pos.y + 1, pos.z, AIR);
                        }
                        self.record_and_broadcast_block(pos.x, pos.y, pos.z, FARMLAND);
                    }
                } else if item == SEEDS && face == Face::Top {
                    if self
                        .farming
                        .plant_seeds(&mut self.world, pos.x, pos.y, pos.z)
                    {
                        self.record_and_broadcast_block(pos.x, pos.y + 1, pos.z, WHEAT_0);
                    }
                } else if item == BONE {
                    if let Some([cx, cy, cz]) =
                        self.farming.fertilize(&mut self.world, pos.x, pos.y, pos.z)
                    {
                        let new_b = self.world.get_block(cx, cy, cz);
                        self.record_and_broadcast_block(cx, cy, cz, new_b);
                    }
                }
            }
            Packet::ChatMessage { message } => {
                let sender = self.clients[idx].name.clone();
                let clean: String = message.chars().filter(|c| !c.is_control()).take(200).collect();
                if !clean.is_empty() {
                    self.broadcast_play(&Packet::SystemChat { sender, message: clean }, None);
                }
            }
            Packet::InteractEntity { entity_id, damage, item_id } => {
                let attacker_pos = self.clients[idx].pos;
                if let Some(m_idx) = self.mobs.list.iter().position(|m| m.net_id == entity_id) {
                    if damage > 0 {
                        self.mobs.list[m_idx].damage(damage.min(50), attacker_pos);
                        let m = &self.mobs.list[m_idx];
                        if m.health > 0 {
                            self.broadcast_play(
                                &Packet::MoveEntity {
                                    entity_id: m.net_id,
                                    x: m.body.pos[0],
                                    y: m.body.pos[1],
                                    z: m.body.pos[2],
                                    yaw: Angle::from_radians(m.yaw),
                                    pitch: Angle(0),
                                    hurt: true,
                                },
                                None,
                            );
                        }
                    } else if item_id > 0 {
                        self.mobs.feed(m_idx, item_id as u16);
                    }
                } else if damage > 0 {
                    // Başka bir oyuncuya yakın dövüş vuruşu
                    if let Some(target) = self
                        .clients
                        .iter()
                        .find(|c| !c.closed && c.conn.state == State::Play && c.id == entity_id)
                    {
                        let tid = target.id;
                        let tpos = target.pos;
                        let tyaw = target.yaw;
                        let tpitch = target.pitch;
                        self.broadcast_play(
                            &Packet::EntityDamage {
                                target_player: tid,
                                damage: damage.clamp(1, 10),
                                poison: false,
                            },
                            None,
                        );
                        self.broadcast_play(
                            &Packet::MoveEntity {
                                entity_id: tid,
                                x: tpos[0],
                                y: tpos[1],
                                z: tpos[2],
                                yaw: Angle::from_radians(tyaw as f64),
                                pitch: Angle::from_radians(tpitch as f64),
                                hurt: true,
                            },
                            None,
                        );
                    }
                }
            }
            Packet::KeepAliveServerbound { .. } => {}
            _ => {}
        }
    }

    fn apply_block_change(&mut self, x: i32, y: i32, z: i32, b: u8) {
        self.world.ensure_chunk(x.div_euclid(CHUNK_SIZE), z.div_euclid(CHUNK_SIZE));
        let prev = self.world.get_block(x, y, z);
        if prev == TORCH && b != TORCH {
            self.torches.retain(|&t| t != [x, y, z]);
        } else if b == TORCH && !self.torches.contains(&[x, y, z]) {
            self.torches.push([x, y, z]);
        }
        self.world.set_block_raw(x, y, z, b);
        self.record_and_broadcast_block(x, y, z, b);

        // Tarım toprağı kırıldıysa üstündeki ekin de temizlenir
        let above = self.world.get_block(x, y + 1, z);
        if b != FARMLAND && is_crop(above) {
            self.world.set_block_raw(x, y + 1, z, AIR);
            self.farming
                .plots
                .retain(|p| p.pos != Position { x, y: y + 1, z });
            self.record_and_broadcast_block(x, y + 1, z, AIR);
        }
    }

    fn record_and_broadcast_block(&mut self, x: i32, y: i32, z: i32, b: u8) {
        self.edits.insert((x, y, z), b);
        let pos = Position { x, y, z };
        if pos.in_range() {
            self.broadcast_play(&Packet::BlockUpdate { pos, block: b as i32 }, None);
        }
    }

    fn step_world(&mut self, dt: f64) {
        self.day_time = (self.day_time + dt / DAY_LENGTH) % 1.0;

        // Tarım büyüme adımı
        let mut changed_crops = Vec::new();
        self.farming
            .tick(&mut self.world, dt, &mut changed_crops);
        for [cx, cy, cz] in changed_crops {
            let new_b = self.world.get_block(cx, cy, cz);
            self.record_and_broadcast_block(cx, cy, cz, new_b);
        }

        // Aktif Play oyuncularının konumları
        let play_players: Vec<(i32, [f64; 3])> = self
            .clients
            .iter()
            .filter(|c| !c.closed && c.conn.state == State::Play)
            .map(|c| (c.id, c.pos))
            .collect();

        if !play_players.is_empty() {
            for &(_, p) in &play_players {
                let pcx = (p[0] / CHUNK_SIZE as f64).floor() as i32;
                let pcz = (p[2] / CHUNK_SIZE as f64).floor() as i32;
                for dz in -2..=2 {
                    for dx in -2..=2 {
                        self.world.ensure_chunk(pcx + dx, pcz + dz);
                    }
                }
            }
            let positions: Vec<[f64; 3]> = play_players.iter().map(|&(_, p)| p).collect();
            let sun_elev = (self.day_time * std::f64::consts::TAU).sin();
            let is_night = sun_elev < -0.05;
            let mut events = Vec::new();
            self.mobs
                .tick_multi(&self.world, dt, &positions, is_night, &self.torches, &mut events);

            for ev in events {
                match ev {
                    MobEvent::Attack(p_idx, dmg) | MobEvent::ArrowHit(p_idx, dmg) => {
                        if let Some(&(pid, _)) = play_players.get(p_idx) {
                            self.broadcast_play(
                                &Packet::EntityDamage {
                                    target_player: pid,
                                    damage: dmg,
                                    poison: false,
                                },
                                None,
                            );
                        }
                    }
                    MobEvent::Poison(p_idx) => {
                        if let Some(&(pid, _)) = play_players.get(p_idx) {
                            self.broadcast_play(
                                &Packet::EntityDamage {
                                    target_player: pid,
                                    damage: 1,
                                    poison: true,
                                },
                                None,
                            );
                        }
                    }
                    _ => {}
                }
            }

            // Canlıların doğma / hareket / silinme farklarını istemcilere yayınla
            let mut current_ids = HashSet::with_capacity(self.mobs.list.len());
            let mut spawns = Vec::new();
            let mut moves = Vec::new();
            for m in &self.mobs.list {
                current_ids.insert(m.net_id);
                let hurt = m.hurt > 0.5;
                if let Some(&(prev_pos, prev_yaw, prev_hurt)) = self.known_mobs.get(&m.net_id) {
                    let moved = (prev_pos[0] - m.body.pos[0]).powi(2)
                        + (prev_pos[1] - m.body.pos[1]).powi(2)
                        + (prev_pos[2] - m.body.pos[2]).powi(2)
                        > 0.0004
                        || (prev_yaw - m.yaw).abs() > 0.03
                        || (hurt && !prev_hurt);
                    if moved {
                        moves.push(Packet::MoveEntity {
                            entity_id: m.net_id,
                            x: m.body.pos[0],
                            y: m.body.pos[1],
                            z: m.body.pos[2],
                            yaw: Angle::from_radians(m.yaw),
                            pitch: Angle(0),
                            hurt,
                        });
                    }
                } else {
                    spawns.push(Packet::SpawnEntity {
                        entity_id: m.net_id,
                        kind: m.kind.identifier().to_string(),
                        x: m.body.pos[0],
                        y: m.body.pos[1],
                        z: m.body.pos[2],
                        pitch: Angle(0),
                        yaw: Angle::from_radians(m.yaw),
                    });
                }
                self.known_mobs
                    .insert(m.net_id, (m.body.pos, m.yaw, hurt));
            }

            let mut removed = Vec::new();
            self.known_mobs.retain(|&id, _| {
                if current_ids.contains(&id) {
                    true
                } else {
                    removed.push(id);
                    false
                }
            });

            for pkt in spawns {
                self.broadcast_play(&pkt, None);
            }
            for pkt in moves {
                self.broadcast_play(&pkt, None);
            }
            if !removed.is_empty() {
                self.broadcast_play(&Packet::RemoveEntities { entity_ids: removed }, None);
            }
        }

        // KeepAlive
        self.keepalive_timer -= dt;
        if self.keepalive_timer <= 0.0 {
            self.keepalive_timer = 10.0;
            let ka_id = (self.day_time * 1_000_000.0) as i64;
            self.broadcast_play(&Packet::KeepAliveClientbound { id: ka_id }, None);
        }
    }

    fn broadcast_play(&mut self, packet: &Packet, exclude_id: Option<i32>) {
        for client in &mut self.clients {
            if !client.closed
                && client.conn.state == State::Play
                && exclude_id != Some(client.id)
            {
                client.send_packet(packet);
            }
        }
    }
}

fn serve_static_http(stream: &mut TcpStream, request_line: &str, root_dir: &Path) {
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let raw_path = parts.next().unwrap_or("/");
    if method != "GET" && method != "HEAD" {
        let _ = stream.write_all(b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\n\r\n");
        return;
    }
    let clean_path = raw_path.split('?').next().unwrap_or("/");
    let rel = match clean_path {
        "/" | "" => "index.html",
        p => p.trim_start_matches('/'),
    };
    if rel.contains("..") || rel.contains('\\') {
        let _ = stream.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n");
        return;
    }
    let full = root_dir.join(rel);
    let Ok(bytes) = fs::read(&full) else {
        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\n\r\nNot Found");
        return;
    };
    let mime = if rel.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if rel.ends_with(".js") {
        "text/javascript; charset=utf-8"
    } else if rel.ends_with(".wasm") {
        "application/wasm"
    } else if rel.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if rel.ends_with(".json") {
        "application/json"
    } else {
        "application/octet-stream"
    };
    let header = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: {mime}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-cache\r\n\
         Connection: close\r\n\r\n",
        bytes.len()
    );
    let _ = stream.write_all(header.as_bytes());
    if method == "GET" {
        let _ = stream.write_all(&bytes);
    }
}

/// Verilen portta hem HTTP web sunucusunu hem de çok oyunculu WebSocket
/// sunucusunu başlatır (sonsuz döngü).
pub fn run_server(port: u16, seed: i32, root_dir: &Path) -> std::io::Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", port))?;
    listener.set_nonblocking(true)?;
    println!("==============================================================");
    println!("  VoxelCraft Çok Oyunculu Sunucu + HTTP Web Sunucusu Hazır!");
    println!("  Web Arayüzü : http://localhost:{port}");
    println!("  WebSocket   : ws://localhost:{port}");
    println!("  Dünya Tohumu: {seed}");
    println!("==============================================================");

    let mut server = GameServer::new(seed);
    let tick_dur = Duration::from_millis(50); // 20 Hz
    let mut last_tick = Instant::now();

    loop {
        while let Ok((stream, _addr)) = listener.accept() {
            server.accept_http_or_ws(stream, root_dir);
        }
        let now = Instant::now();
        let elapsed = now.duration_since(last_tick);
        if elapsed >= tick_dur {
            let dt = elapsed.as_secs_f64().min(0.1);
            last_tick = now;
            server.tick(dt);
        } else {
            // Gelen paketleri düşük gecikmeyle işle
            server.poll_clients();
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::{BRICK, FARMLAND, HOE, SEEDS, WHEAT_0};
    use crate::protocol::{Face, PROTOCOL_VERSION};

    /// RFC 6455 Bölüm 1.3'teki resmi el sıkışma anahtarı testi.
    #[test]
    fn rfc6455_websocket_accept_key_matches_specification() {
        let key = "dGhlIHNhbXBsZSBub25jZQ==";
        assert_eq!(ws_accept_key(key), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn websocket_masked_and_unmasked_frames_round_trip() {
        let payload = vec![1, 2, 3, 4, 5, 250, 255];
        let masked = encode_ws_masked_frame(0x02, &payload, [0xAA, 0xBB, 0xCC, 0xDD]);
        let (op, dec, used) = try_decode_ws_frame(&masked).unwrap().unwrap();
        assert_eq!(op, 0x02);
        assert_eq!(dec, payload);
        assert_eq!(used, masked.len());
    }

    /// Test yardımcısı: sunucuya bağlı sahte WebSocket istemcisi.
    struct TestClient {
        id: i32,
        raw_in: Vec<u8>,
        decoder: Decoder,
        conn: Connection,
    }

    impl TestClient {
        fn connect(server: &mut GameServer) -> Self {
            let id = server.add_memory_client();
            Self {
                id,
                raw_in: Vec::new(),
                decoder: Decoder::default(),
                conn: Connection::default(),
            }
        }

        fn send(&mut self, server: &mut GameServer, pkt: Packet) {
            self.conn.handle(&pkt).unwrap();
            let bytes = pkt.encode(None).unwrap();
            let frame = encode_ws_masked_frame(0x02, &bytes, [0x12, 0x34, 0x56, 0x78]);
            server.push_client_ws_bytes(self.id, &frame);
        }

        fn recv_all(&mut self, server: &mut GameServer) -> Vec<Packet> {
            self.raw_in.extend_from_slice(&server.take_client_ws_bytes(self.id));
            while let Ok(Some((opcode, payload, consumed))) = try_decode_ws_frame(&self.raw_in) {
                self.raw_in.drain(..consumed);
                if opcode == 0x02 {
                    self.decoder.push(&payload);
                }
            }
            let mut out = Vec::new();
            while let Ok(Some((id, body))) = self.decoder.next_packet() {
                let pkt = Packet::decode(self.conn.state, Direction::Clientbound, id, &body).unwrap();
                self.conn.handle(&pkt).unwrap();
                out.push(pkt);
            }
            out
        }

        fn login_to_play(&mut self, server: &mut GameServer, name: &str) -> Vec<Packet> {
            self.send(
                server,
                Packet::Handshake {
                    protocol: PROTOCOL_VERSION,
                    address: "localhost".into(),
                    port: 8080,
                    intent: Intent::Login,
                },
            );
            self.send(server, Packet::LoginStart { name: name.into() });
            server.tick(0.01);
            let pkts = self.recv_all(server);
            assert!(pkts.iter().any(|p| matches!(p, Packet::LoginSuccess { .. })));

            self.send(server, Packet::LoginAcknowledged);
            server.tick(0.01);
            let pkts = self.recv_all(server);
            assert!(pkts.contains(&Packet::FinishConfiguration));

            self.send(server, Packet::AcknowledgeFinishConfiguration);
            server.tick(0.01);
            assert_eq!(self.conn.state, State::Play);
            self.recv_all(server)
        }
    }

    #[test]
    fn server_handles_status_ping_and_two_players_sync_blocks_farming_and_chat() {
        let mut server = GameServer::new(20261012);

        // 1. Server List Ping (Status + Ping/Pong)
        let mut slp = TestClient::connect(&mut server);
        slp.send(
            &mut server,
            Packet::Handshake {
                protocol: PROTOCOL_VERSION,
                address: "localhost".into(),
                port: 8080,
                intent: Intent::Status,
            },
        );
        slp.send(&mut server, Packet::StatusRequest);
        slp.send(&mut server, Packet::Ping { payload: 4242 });
        server.tick(0.01);
        let status_pkts = slp.recv_all(&mut server);
        assert!(status_pkts.iter().any(|p| matches!(p, Packet::StatusResponse { json } if json.contains("VoxelCraft"))));
        assert!(status_pkts.contains(&Packet::Pong { payload: 4242 }));

        // 2. İki oyuncu Login -> Configuration -> Play akışıyla bağlanır
        let mut c1 = TestClient::connect(&mut server);
        let c1_init = c1.login_to_play(&mut server, "Taha");
        assert!(c1_init.iter().any(|p| matches!(p, Packet::JoinGame { seed: 20261012, .. })));

        let mut c2 = TestClient::connect(&mut server);
        let c2_init = c2.login_to_play(&mut server, "Elif");
        // Elif bağlandığında Taha'nın SpawnPlayer paketini almalı, Taha da Elif'i görmeli
        assert!(c2_init.iter().any(|p| matches!(p, Packet::SpawnPlayer { name, .. } if name == "Taha")));
        let c1_updates = c1.recv_all(&mut server);
        assert!(c1_updates.iter().any(|p| matches!(p, Packet::SpawnPlayer { name, .. } if name == "Elif")));

        // 3. Taha bir blok koyar -> Elif BlockUpdate paketini alır
        let pos = Position { x: 2, y: 30, z: 2 };
        c1.send(&mut server, Packet::PlayerAction { pos, block: BRICK as i32 });
        server.tick(0.01);
        let c2_updates = c2.recv_all(&mut server);
        assert!(c2_updates.contains(&Packet::BlockUpdate { pos, block: BRICK as i32 }));

        // 4. Elif çapa ve tohum kullanır -> Taha tarla ve buğday filizi güncellemelerini alır
        let farm_pos = Position { x: 0, y: 20, z: 0 };
        server.apply_block_change(0, 20, 0, crate::blocks::GRASS);
        server.apply_block_change(0, 21, 0, crate::blocks::AIR);
        let _ = c1.recv_all(&mut server);
        let _ = c2.recv_all(&mut server);

        c2.send(&mut server, Packet::UseItemOn { pos: farm_pos, face: Face::Top, item_id: HOE as i32 });
        c2.send(&mut server, Packet::UseItemOn { pos: farm_pos, face: Face::Top, item_id: SEEDS as i32 });
        server.tick(0.01);
        let c1_farm = c1.recv_all(&mut server);
        assert!(c1_farm.contains(&Packet::BlockUpdate { pos: farm_pos, block: FARMLAND as i32 }));
        assert!(c1_farm.contains(&Packet::BlockUpdate {
            pos: Position { x: 0, y: 21, z: 0 },
            block: WHEAT_0 as i32,
        }));

        // 5. Sohbet iletisi tüm oyunculara ulaşır
        c1.send(&mut server, Packet::ChatMessage { message: "Selam Elif!".into() });
        server.tick(0.01);
        let c2_chat = c2.recv_all(&mut server);
        assert!(c2_chat.contains(&Packet::SystemChat {
            sender: "Taha".into(),
            message: "Selam Elif!".into(),
        }));
    }
}
