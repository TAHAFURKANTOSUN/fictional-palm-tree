//! Ağ protokolü temeli — Minecraft: Java Edition protokol dokümantasyonundaki
//! tasarımın (veri tipleri, paket çerçeveleme, bağlantı durumları) uyarlaması.
//!
//! Bu modül **yalnızca kodlama katmanıdır**: bayt dizisi üretir ve çözer, hiç
//! soket açmaz. Oyun şu an tek oyunculudur; çok oyunculu eklenirse istemci ve
//! sunucu bu katmanı paylaşır.
//!
//! Minecraft ile aynı olanlar: `VarInt`/`VarLong`, 64 bitlik `Position`,
//! uzunluk önekli `String`, `Identifier` kuralları, büyük-uçlu (big-endian)
//! sayılar, `[uzunluk][kimlik][veri]` çerçevesi ve Handshaking → Status / Login
//! → Configuration → Play durum makinesi. Farklı olanlar: Play paketlerinin
//! kimlikleri ve içerikleri bu oyuna özeldir (blok ve eşya kümesi farklıdır),
//! şifreleme yoktur ve sıkıştırılmış paketler **çözülmez** (zlib bağımlılığı
//! eklenmedi; sıkıştırma eşiğinin altındaki paketler desteklenir).

use std::fmt;

/// Bu oyunun protokol sürümü (Minecraft'ın sürüm numaralarıyla ilgisi yoktur).
pub const PROTOCOL_VERSION: i32 = 1;
/// Bir paketin (uzunluk öneki hariç) en büyük boyutu: 3 baytlık VarInt sınırı.
pub const MAX_PACKET_LEN: usize = 2_097_151;
/// `String` alanlarının üst sınırı (UTF-16 kod birimi).
pub const MAX_STRING_LEN: usize = 32_767;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    /// Veri beklenenden erken bitti.
    UnexpectedEof,
    /// VarInt 5 (VarLong 10) bayttan uzun.
    VarIntTooLong,
    /// Dize ya da paket izin verilen boyutu aşıyor.
    TooLong,
    InvalidUtf8,
    InvalidIdentifier,
    /// Bu durumda / yönde tanımlı olmayan paket kimliği ya da alan değeri.
    UnknownPacket(i32),
    InvalidValue,
    /// Paket zlib ile sıkıştırılmış; bu katman sıkıştırmayı çözmez.
    CompressionUnsupported,
    /// Paket çözüldükten sonra artan bayt kaldı.
    TrailingBytes,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

pub type Result<T> = std::result::Result<T, Error>;

// --- yazma ----------------------------------------------------------------

/// Paket gövdesi yazıcısı. Çok baytlı sayılar büyük-uçludur.
#[derive(Default)]
pub struct Writer {
    pub buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn bool(&mut self, v: bool) {
        self.buf.push(v as u8);
    }

    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    pub fn i64(&mut self, v: i64) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    pub fn f32(&mut self, v: f32) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    pub fn f64(&mut self, v: f64) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    /// Değişken uzunluklu 32 bit tam sayı: bayt başına 7 bit, en düşük
    /// anlamlı grup önce; üst bit "devamı var" demektir. Negatif sayılar
    /// ikiye tümleyen olarak (her zaman 5 bayt) yazılır.
    pub fn var_int(&mut self, v: i32) {
        let mut x = v as u32;
        loop {
            if x & !0x7f == 0 {
                self.buf.push(x as u8);
                return;
            }
            self.buf.push((x & 0x7f) as u8 | 0x80);
            x >>= 7;
        }
    }

    pub fn var_long(&mut self, v: i64) {
        let mut x = v as u64;
        loop {
            if x & !0x7f == 0 {
                self.buf.push(x as u8);
                return;
            }
            self.buf.push((x & 0x7f) as u8 | 0x80);
            x >>= 7;
        }
    }

    /// VarInt bayt uzunluğu + UTF-8 dize.
    pub fn string(&mut self, s: &str) {
        self.var_int(s.len() as i32);
        self.buf.extend_from_slice(s.as_bytes());
    }

    pub fn position(&mut self, p: Position) {
        self.i64(p.pack());
    }
}

/// Bir `VarInt`'in kaç bayt tuttuğu.
pub fn var_int_len(v: i32) -> usize {
    let mut w = Writer::new();
    w.var_int(v);
    w.buf.len()
}

// --- okuma ----------------------------------------------------------------

pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    /// Tüm veri tüketildiyse `Ok`; paket sonunda artan bayt bir hatadır.
    pub fn finish(&self) -> Result<()> {
        if self.remaining() == 0 { Ok(()) } else { Err(Error::TrailingBytes) }
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.remaining() < n {
            return Err(Error::UnexpectedEof);
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.bytes(N)?);
        Ok(out)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }

    pub fn bool(&mut self) -> Result<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Error::InvalidValue),
        }
    }

    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.array()?))
    }

    pub fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_be_bytes(self.array()?))
    }

    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_be_bytes(self.array()?))
    }

    pub fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_be_bytes(self.array()?))
    }

    pub fn var_int(&mut self) -> Result<i32> {
        let mut value = 0u32;
        for i in 0..5 {
            let b = self.u8()?;
            value |= ((b & 0x7f) as u32) << (7 * i);
            if b & 0x80 == 0 {
                return Ok(value as i32);
            }
        }
        Err(Error::VarIntTooLong)
    }

    pub fn var_long(&mut self) -> Result<i64> {
        let mut value = 0u64;
        for i in 0..10 {
            let b = self.u8()?;
            value |= ((b & 0x7f) as u64) << (7 * i);
            if b & 0x80 == 0 {
                return Ok(value as i64);
            }
        }
        Err(Error::VarIntTooLong)
    }

    /// En çok `max` UTF-16 kod birimi uzunluğunda dize.
    pub fn string(&mut self, max: usize) -> Result<&'a str> {
        let len = self.var_int()?;
        // Bir kod birimi UTF-8'de en çok 3 bayt tutar.
        if len < 0 || len as usize > max * 3 {
            return Err(Error::TooLong);
        }
        let s = std::str::from_utf8(self.bytes(len as usize)?).map_err(|_| Error::InvalidUtf8)?;
        if s.encode_utf16().count() > max {
            return Err(Error::TooLong);
        }
        Ok(s)
    }

    pub fn position(&mut self) -> Result<Position> {
        Ok(Position::unpack(self.i64()?))
    }
}

// --- Position ve Identifier -------------------------------------------------

/// Blok konumu. Ağda tek bir 64 bit sayıya paketlenir: `x` 26 bit, `z` 26 bit,
/// `y` 12 bit (hepsi işaretli).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Position {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl Position {
    pub fn pack(self) -> i64 {
        ((self.x as i64 & 0x3FF_FFFF) << 38) | ((self.z as i64 & 0x3FF_FFFF) << 12) | (self.y as i64 & 0xFFF)
    }

    /// Kaydırmalar işaretlidir: üst bitler işareti kendiliğinden taşır.
    pub fn unpack(v: i64) -> Self {
        Self { x: (v >> 38) as i32, y: (v << 52 >> 52) as i32, z: (v << 26 >> 38) as i32 }
    }

    /// Paketlenebilir aralıkta mı? (x, z: ±33.554.432; y: −2048..=2047)
    pub fn in_range(self) -> bool {
        let h = 1 << 25;
        (-h..h).contains(&self.x) && (-h..h).contains(&self.z) && (-2048..2048).contains(&self.y)
    }
}

/// `ad_alanı:yol` biçiminde kimlik (ör. `minecraft:stone`). Ad alanı
/// verilmezse `minecraft` sayılır.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Identifier {
    pub namespace: String,
    pub path: String,
}

impl Identifier {
    /// Ad alanında `a-z 0-9 . - _`, yolda ayrıca `/` kullanılabilir.
    pub fn parse(s: &str) -> Result<Self> {
        let (namespace, path) = s.split_once(':').unwrap_or(("minecraft", s));
        let ok = |c: char, slash: bool| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_') || (slash && c == '/');
        if namespace.is_empty() || path.is_empty() || !namespace.chars().all(|c| ok(c, false)) || !path.chars().all(|c| ok(c, true)) {
            return Err(Error::InvalidIdentifier);
        }
        Ok(Self { namespace: namespace.to_string(), path: path.to_string() })
    }
}

impl fmt::Display for Identifier {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}:{}", self.namespace, self.path)
    }
}

// --- çerçeveleme ------------------------------------------------------------

/// Paketi çerçeveler: `[uzunluk VarInt][kimlik VarInt][veri]`.
///
/// `threshold` verilmişse sıkıştırma etkin biçim kullanılır:
/// `[uzunluk][veri uzunluğu VarInt][kimlik][veri]`; "veri uzunluğu" 0 ise
/// paket sıkıştırılmamıştır. Bu katman sıkıştırma yapmadığından her zaman 0
/// yazar — eşiği aşan paketler için de geçerli bir akıştır, yalnızca daha
/// büyüktür.
pub fn frame(id: i32, body: &[u8], threshold: Option<usize>) -> Result<Vec<u8>> {
    let mut inner = Writer::new();
    if threshold.is_some() {
        inner.var_int(0);
    }
    inner.var_int(id);
    inner.buf.extend_from_slice(body);
    if inner.buf.len() > MAX_PACKET_LEN {
        return Err(Error::TooLong);
    }
    let mut out = Writer::new();
    out.var_int(inner.buf.len() as i32);
    out.buf.extend_from_slice(&inner.buf);
    Ok(out.buf)
}

/// Akıştan gelen baytları biriktirip tam paketleri ayıran çözücü. TCP paket
/// sınırlarını korumaz; bir çerçeve birkaç parçada gelebilir.
#[derive(Default)]
pub struct Decoder {
    buf: Vec<u8>,
    /// Sıkıştırma etkinse (Login sırasında açılır) çerçevede "veri uzunluğu"
    /// alanı da bulunur.
    pub compression: bool,
}

impl Decoder {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Tamponda tam bir paket varsa `(kimlik, gövde)` döndürür; henüz
    /// tamamlanmadıysa `Ok(None)`.
    pub fn next_packet(&mut self) -> Result<Option<(i32, Vec<u8>)>> {
        let mut r = Reader::new(&self.buf);
        let len = match r.var_int() {
            Ok(v) => v,
            // Uzunluk öneki bile tamamlanmadı.
            Err(Error::UnexpectedEof) => return Ok(None),
            Err(e) => return Err(e),
        };
        if len < 0 || len as usize > MAX_PACKET_LEN {
            return Err(Error::TooLong);
        }
        let Ok(frame) = r.bytes(len as usize) else { return Ok(None) };
        let consumed = r.pos;

        let mut r = Reader::new(frame);
        if self.compression && r.var_int()? != 0 {
            return Err(Error::CompressionUnsupported);
        }
        let id = r.var_int()?;
        let body = r.bytes(r.remaining())?.to_vec();
        self.buf.drain(..consumed);
        Ok(Some((id, body)))
    }
}

// --- bağlantı durumları -----------------------------------------------------

/// Bağlantının bulunduğu aşama. Aynı paket kimliği, duruma ve yöne göre
/// tamamen farklı bir paketi gösterir.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Handshaking,
    Status,
    Login,
    Configuration,
    Play,
}

/// Paketin yönü.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    /// İstemciden sunucuya.
    Serverbound,
    /// Sunucudan istemciye.
    Clientbound,
}

/// El sıkışmada istemcinin istediği sonraki durum.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Intent {
    Status = 1,
    Login = 2,
    Transfer = 3,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Packet {
    // Handshaking (serverbound)
    Handshake { protocol: i32, address: String, port: u16, intent: Intent },
    // Status
    StatusRequest,
    /// Sunucu listesi yanıtı (JSON).
    StatusResponse { json: String },
    Ping { payload: i64 },
    Pong { payload: i64 },
    // Login
    LoginStart { name: String },
    LoginSuccess { name: String },
    LoginAcknowledged,
    // Configuration
    FinishConfiguration,
    AcknowledgeFinishConfiguration,
    // Play (bu oyuna özel)
    /// Bir blok değişti; `block` oyunun blok kimliğidir.
    BlockUpdate { pos: Position, block: i32 },
    /// Oyuncunun konumu ve bakışı.
    PlayerPosition { x: f64, y: f64, z: f64, yaw: f32, pitch: f32, on_ground: bool },
}

impl Packet {
    /// Paketin geçerli olduğu `(durum, yön, kimlik)`.
    pub fn route(&self) -> (State, Direction, i32) {
        use Direction::*;
        use State::*;
        match self {
            Packet::Handshake { .. } => (Handshaking, Serverbound, 0x00),
            Packet::StatusRequest => (Status, Serverbound, 0x00),
            Packet::Ping { .. } => (Status, Serverbound, 0x01),
            Packet::StatusResponse { .. } => (Status, Clientbound, 0x00),
            Packet::Pong { .. } => (Status, Clientbound, 0x01),
            Packet::LoginStart { .. } => (Login, Serverbound, 0x00),
            Packet::LoginAcknowledged => (Login, Serverbound, 0x01),
            Packet::LoginSuccess { .. } => (Login, Clientbound, 0x00),
            Packet::AcknowledgeFinishConfiguration => (Configuration, Serverbound, 0x00),
            Packet::FinishConfiguration => (Configuration, Clientbound, 0x00),
            Packet::PlayerPosition { .. } => (Play, Serverbound, 0x00),
            Packet::BlockUpdate { .. } => (Play, Clientbound, 0x00),
        }
    }

    /// Paketi çerçevelenmiş bayt dizisine çevirir.
    pub fn encode(&self, threshold: Option<usize>) -> Result<Vec<u8>> {
        let mut w = Writer::new();
        match self {
            Packet::Handshake { protocol, address, port, intent } => {
                if address.encode_utf16().count() > 255 {
                    return Err(Error::TooLong);
                }
                w.var_int(*protocol);
                w.string(address);
                w.u16(*port);
                w.var_int(*intent as i32);
            }
            Packet::StatusRequest
            | Packet::LoginAcknowledged
            | Packet::FinishConfiguration
            | Packet::AcknowledgeFinishConfiguration => {}
            Packet::StatusResponse { json } => w.string(json),
            Packet::Ping { payload } | Packet::Pong { payload } => w.i64(*payload),
            Packet::LoginStart { name } | Packet::LoginSuccess { name } => {
                if name.is_empty() || name.encode_utf16().count() > 16 {
                    return Err(Error::InvalidValue);
                }
                w.string(name);
            }
            Packet::BlockUpdate { pos, block } => {
                if !pos.in_range() {
                    return Err(Error::InvalidValue);
                }
                w.position(*pos);
                w.var_int(*block);
            }
            Packet::PlayerPosition { x, y, z, yaw, pitch, on_ground } => {
                w.f64(*x);
                w.f64(*y);
                w.f64(*z);
                w.f32(*yaw);
                w.f32(*pitch);
                w.bool(*on_ground);
            }
        }
        frame(self.route().2, &w.buf, threshold)
    }

    /// Verilen durum ve yönde `id` kimlikli paketi çözer.
    pub fn decode(state: State, dir: Direction, id: i32, body: &[u8]) -> Result<Self> {
        use Direction::*;
        use State::*;
        let mut r = Reader::new(body);
        let packet = match (state, dir, id) {
            (Handshaking, Serverbound, 0x00) => {
                let protocol = r.var_int()?;
                let address = r.string(255)?.to_string();
                let port = r.u16()?;
                let intent = match r.var_int()? {
                    1 => Intent::Status,
                    2 => Intent::Login,
                    3 => Intent::Transfer,
                    _ => return Err(Error::InvalidValue),
                };
                Packet::Handshake { protocol, address, port, intent }
            }
            (Status, Serverbound, 0x00) => Packet::StatusRequest,
            (Status, Serverbound, 0x01) => Packet::Ping { payload: r.i64()? },
            (Status, Clientbound, 0x00) => Packet::StatusResponse { json: r.string(MAX_STRING_LEN)?.to_string() },
            (Status, Clientbound, 0x01) => Packet::Pong { payload: r.i64()? },
            (Login, Serverbound, 0x00) => Packet::LoginStart { name: r.string(16)?.to_string() },
            (Login, Serverbound, 0x01) => Packet::LoginAcknowledged,
            (Login, Clientbound, 0x00) => Packet::LoginSuccess { name: r.string(16)?.to_string() },
            (Configuration, Serverbound, 0x00) => Packet::AcknowledgeFinishConfiguration,
            (Configuration, Clientbound, 0x00) => Packet::FinishConfiguration,
            (Play, Serverbound, 0x00) => Packet::PlayerPosition {
                x: r.f64()?,
                y: r.f64()?,
                z: r.f64()?,
                yaw: r.f32()?,
                pitch: r.f32()?,
                on_ground: r.bool()?,
            },
            (Play, Clientbound, 0x00) => Packet::BlockUpdate { pos: r.position()?, block: r.var_int()? },
            _ => return Err(Error::UnknownPacket(id)),
        };
        r.finish()?;
        Ok(packet)
    }
}

/// Bir uçtaki bağlantı durumu: hangi paketin ne zaman geçerli olduğunu ve
/// durum geçişlerini izler.
pub struct Connection {
    pub state: State,
}

impl Default for Connection {
    fn default() -> Self {
        Self { state: State::Handshaking }
    }
}

impl Connection {
    /// Gönderilen ya da alınan bir paketi işler: paket bu durumda geçerli
    /// değilse hata verir, geçiş gerektiriyorsa durumu ilerletir.
    ///
    /// Geçişler: `Handshake` → istenen durum (Status / Login);
    /// `LoginAcknowledged` → Configuration;
    /// `AcknowledgeFinishConfiguration` → Play.
    pub fn handle(&mut self, packet: &Packet) -> Result<()> {
        let (state, _, id) = packet.route();
        if state != self.state {
            return Err(Error::UnknownPacket(id));
        }
        self.state = match packet {
            Packet::Handshake { intent: Intent::Status, .. } => State::Status,
            Packet::Handshake { .. } => State::Login,
            Packet::LoginAcknowledged => State::Configuration,
            Packet::AcknowledgeFinishConfiguration => State::Play,
            _ => self.state,
        };
        Ok(())
    }
}

/// Sunucu listesi (Server List Ping) yanıtının JSON'u.
pub fn status_json(version: &str, online: u32, max: u32, description: &str) -> String {
    let escape = |s: &str| {
        let mut out = String::new();
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                c => out.push(c),
            }
        }
        out
    };
    format!(
        "{{\"version\":{{\"name\":\"{}\",\"protocol\":{}}},\"players\":{{\"max\":{},\"online\":{}}},\"description\":{{\"text\":\"{}\"}}}}",
        escape(version),
        PROTOCOL_VERSION,
        max,
        online,
        escape(description)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn var_int_bytes(v: i32) -> Vec<u8> {
        let mut w = Writer::new();
        w.var_int(v);
        w.buf
    }

    /// Protokol dokümantasyonundaki örnek tablo.
    #[test]
    fn var_int_matches_the_documented_examples() {
        let cases: [(i32, &[u8]); 10] = [
            (0, &[0x00]),
            (1, &[0x01]),
            (2, &[0x02]),
            (127, &[0x7f]),
            (128, &[0x80, 0x01]),
            (255, &[0xff, 0x01]),
            (25565, &[0xdd, 0xc7, 0x01]),
            (2097151, &[0xff, 0xff, 0x7f]),
            (i32::MAX, &[0xff, 0xff, 0xff, 0xff, 0x07]),
            (-1, &[0xff, 0xff, 0xff, 0xff, 0x0f]),
        ];
        for (value, bytes) in cases {
            assert_eq!(var_int_bytes(value), bytes, "{value} kodlaması");
            let mut r = Reader::new(bytes);
            assert_eq!(r.var_int(), Ok(value), "{value} çözümü");
            assert!(r.finish().is_ok());
            assert_eq!(var_int_len(value), bytes.len());
        }
        assert_eq!(var_int_bytes(i32::MIN), [0x80, 0x80, 0x80, 0x80, 0x08]);
    }

    #[test]
    fn var_long_matches_the_documented_examples() {
        let cases: [(i64, &[u8]); 5] = [
            (0, &[0x00]),
            (2147483647, &[0xff, 0xff, 0xff, 0xff, 0x07]),
            (i64::MAX, &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f]),
            (-1, &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01]),
            (i64::MIN, &[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01]),
        ];
        for (value, bytes) in cases {
            let mut w = Writer::new();
            w.var_long(value);
            assert_eq!(w.buf, bytes, "{value} kodlaması");
            assert_eq!(Reader::new(bytes).var_long(), Ok(value));
        }
    }

    #[test]
    fn malformed_var_ints_are_rejected() {
        // 5 bayttan sonra hâlâ "devamı var".
        assert_eq!(Reader::new(&[0xff; 6]).var_int(), Err(Error::VarIntTooLong));
        assert_eq!(Reader::new(&[0xff; 11]).var_long(), Err(Error::VarIntTooLong));
        // Yarıda kesilmiş.
        assert_eq!(Reader::new(&[0x80]).var_int(), Err(Error::UnexpectedEof));
        assert_eq!(Reader::new(&[]).var_int(), Err(Error::UnexpectedEof));
    }

    /// Dokümantasyondaki Position örneği.
    #[test]
    fn position_matches_the_documented_example() {
        let bits = "0100011000000111011000110010110000010101101101001000001100111111";
        let packed = u64::from_str_radix(bits, 2).unwrap() as i64;
        let pos = Position { x: 18357644, y: 831, z: -20882616 };
        assert_eq!(Position::unpack(packed), pos);
        assert_eq!(pos.pack(), packed);
    }

    #[test]
    fn position_round_trips_including_negative_and_extreme_values() {
        let h = (1 << 25) - 1;
        for pos in [
            Position { x: 0, y: 0, z: 0 },
            Position { x: -1, y: -1, z: -1 },
            Position { x: h, y: 2047, z: h },
            Position { x: -h - 1, y: -2048, z: -h - 1 },
            Position { x: 1_000_000, y: 63, z: -1_000_000 },
        ] {
            assert!(pos.in_range());
            assert_eq!(Position::unpack(pos.pack()), pos, "{pos:?}");
        }
        assert!(!Position { x: 1 << 25, y: 0, z: 0 }.in_range());
        assert!(!Position { x: 0, y: 2048, z: 0 }.in_range());
    }

    #[test]
    fn strings_are_length_prefixed_utf8_with_a_limit() {
        let mut w = Writer::new();
        w.string("Çim Bloğu");
        assert_eq!(w.buf[0] as usize, "Çim Bloğu".len(), "önek bayt sayısıdır, harf sayısı değil");
        assert_eq!(Reader::new(&w.buf).string(32).unwrap(), "Çim Bloğu");
        assert_eq!(Reader::new(&w.buf).string(4), Err(Error::TooLong));
        assert_eq!(Reader::new(&[2, 0xff, 0xfe]).string(8), Err(Error::InvalidUtf8));
        assert_eq!(Reader::new(&[5, b'a']).string(8), Err(Error::UnexpectedEof));
    }

    #[test]
    fn identifiers_follow_the_namespace_rules() {
        let id = Identifier::parse("minecraft:stone").unwrap();
        assert_eq!((id.namespace.as_str(), id.path.as_str()), ("minecraft", "stone"));
        assert_eq!(Identifier::parse("oak_log").unwrap().to_string(), "minecraft:oak_log", "ad alanı varsayılanı");
        assert!(Identifier::parse("voxelcraft:vehicle/car").is_ok(), "yolda / olabilir");
        for bad in ["", "minecraft:", ":stone", "Minecraft:stone", "minecraft:Stone", "mine/craft:stone", "a:b c"] {
            assert_eq!(Identifier::parse(bad), Err(Error::InvalidIdentifier), "{bad:?} reddedilmeli");
        }
    }

    #[test]
    fn frames_carry_length_id_and_body() {
        // Handshake: sürüm 1, "localhost", port 25565, sonraki durum Login.
        let p = Packet::Handshake { protocol: 1, address: "localhost".into(), port: 25565, intent: Intent::Login };
        let bytes = p.encode(None).unwrap();
        let mut expected = vec![15, 0x00, 0x01, 9];
        expected.extend_from_slice(b"localhost");
        expected.extend_from_slice(&[0x63, 0xdd, 0x02]);
        assert_eq!(bytes, expected);
        // Sıkıştırma etkin biçimde araya "veri uzunluğu = 0" girer.
        let compressed = p.encode(Some(256)).unwrap();
        assert_eq!(compressed[..3], [16, 0x00, 0x00]);
        assert_eq!(compressed[3..], expected[2..]);
    }

    #[test]
    fn decoder_reassembles_packets_split_across_reads() {
        let a = Packet::Ping { payload: 0x0102_0304_0506_0708 }.encode(None).unwrap();
        let b = Packet::StatusRequest.encode(None).unwrap();
        let stream: Vec<u8> = a.iter().chain(b.iter()).copied().collect();

        let mut d = Decoder::default();
        // Bayt bayt beslenir: son bayt gelene kadar paket çıkmamalı.
        let mut out = Vec::new();
        for byte in &stream {
            d.push(&[*byte]);
            while let Some(p) = d.next_packet().unwrap() {
                out.push(p);
            }
        }
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], (0x01, vec![1, 2, 3, 4, 5, 6, 7, 8]), "sayılar büyük-uçlu olmalı");
        assert_eq!(out[1], (0x00, vec![]));
        assert_eq!(d.next_packet(), Ok(None));
    }

    #[test]
    fn compressed_payloads_are_reported_not_misread() {
        let mut d = Decoder { compression: true, ..Decoder::default() };
        d.push(&Packet::StatusRequest.encode(Some(64)).unwrap());
        assert_eq!(d.next_packet(), Ok(Some((0x00, vec![]))));
        // Veri uzunluğu ≠ 0: gövde zlib ile sıkıştırılmış demektir.
        d.push(&[3, 0x05, 0x78, 0x9c]);
        assert_eq!(d.next_packet(), Err(Error::CompressionUnsupported));
    }

    #[test]
    fn every_packet_round_trips_in_its_own_state() {
        let packets = [
            Packet::Handshake { protocol: PROTOCOL_VERSION, address: "örnek.sunucu".into(), port: 8080, intent: Intent::Status },
            Packet::StatusRequest,
            Packet::StatusResponse { json: status_json("VoxelCraft", 2, 20, "Hoş geldin") },
            Packet::Ping { payload: -42 },
            Packet::Pong { payload: i64::MAX },
            Packet::LoginStart { name: "Taha".into() },
            Packet::LoginSuccess { name: "Taha".into() },
            Packet::LoginAcknowledged,
            Packet::FinishConfiguration,
            Packet::AcknowledgeFinishConfiguration,
            Packet::BlockUpdate { pos: Position { x: -17, y: 40, z: 123_456 }, block: 9 },
            Packet::PlayerPosition { x: 1e6 + 0.5, y: 29.02, z: -1e6 - 0.5, yaw: 1.5, pitch: -0.25, on_ground: true },
        ];
        for p in packets {
            let (state, dir, id) = p.route();
            let mut d = Decoder::default();
            d.push(&p.encode(None).unwrap());
            let (got_id, body) = d.next_packet().unwrap().expect("tam paket");
            assert_eq!(got_id, id);
            assert_eq!(Packet::decode(state, dir, got_id, &body).unwrap(), p);
        }
    }

    /// Aynı kimlik (0x00) duruma ve yöne göre farklı paketlerdir.
    #[test]
    fn the_same_id_means_different_packets_per_state_and_direction() {
        let body = Packet::Ping { payload: 7 }.encode(None).unwrap()[2..].to_vec();
        assert!(matches!(Packet::decode(State::Status, Direction::Serverbound, 0x01, &body), Ok(Packet::Ping { payload: 7 })));
        assert!(matches!(Packet::decode(State::Status, Direction::Clientbound, 0x01, &body), Ok(Packet::Pong { payload: 7 })));
        assert_eq!(Packet::decode(State::Handshaking, Direction::Serverbound, 0x01, &body), Err(Error::UnknownPacket(1)));
        assert_eq!(Packet::decode(State::Play, Direction::Serverbound, 0x7f, &[]), Err(Error::UnknownPacket(0x7f)));
        // Artan bayt ve geçersiz alan değerleri de hatadır.
        assert_eq!(Packet::decode(State::Status, Direction::Serverbound, 0x00, &[1]), Err(Error::TrailingBytes));
        let bad_intent = [1, 1, b'a', 0, 80, 9];
        assert_eq!(Packet::decode(State::Handshaking, Direction::Serverbound, 0, &bad_intent), Err(Error::InvalidValue));
    }

    #[test]
    fn connection_walks_the_login_flow_and_rejects_out_of_order_packets() {
        let mut c = Connection::default();
        assert_eq!(c.state, State::Handshaking);
        // El sıkışmadan önce başka paket olmaz.
        assert!(c.handle(&Packet::LoginStart { name: "Taha".into() }).is_err());

        let hs = |intent| Packet::Handshake { protocol: PROTOCOL_VERSION, address: "a".into(), port: 1, intent };
        c.handle(&hs(Intent::Login)).unwrap();
        assert_eq!(c.state, State::Login);
        c.handle(&Packet::LoginStart { name: "Taha".into() }).unwrap();
        c.handle(&Packet::LoginSuccess { name: "Taha".into() }).unwrap();
        assert_eq!(c.state, State::Login, "istemci onaylayana kadar Login'de kalınır");
        assert!(c.handle(&Packet::BlockUpdate { pos: Position { x: 0, y: 0, z: 0 }, block: 1 }).is_err(), "Play paketi henüz geçersiz");
        c.handle(&Packet::LoginAcknowledged).unwrap();
        assert_eq!(c.state, State::Configuration);
        c.handle(&Packet::FinishConfiguration).unwrap();
        c.handle(&Packet::AcknowledgeFinishConfiguration).unwrap();
        assert_eq!(c.state, State::Play);
        c.handle(&Packet::BlockUpdate { pos: Position { x: 0, y: 0, z: 0 }, block: 1 }).unwrap();

        // Sunucu listesi sorgusu ayrı bir yoldur ve Play'e hiç ulaşmaz.
        let mut s = Connection::default();
        s.handle(&hs(Intent::Status)).unwrap();
        assert_eq!(s.state, State::Status);
        s.handle(&Packet::StatusRequest).unwrap();
        s.handle(&Packet::Ping { payload: 1 }).unwrap();
        assert!(s.handle(&Packet::LoginStart { name: "Taha".into() }).is_err());
    }

    #[test]
    fn status_json_escapes_text() {
        let json = status_json("VoxelCraft", 3, 20, "Merhaba \"dünya\"\n\\o/");
        assert!(json.contains("\"protocol\":1") && json.contains("\"online\":3") && json.contains("\"max\":20"));
        assert!(json.contains("Merhaba \\\"dünya\\\"\\n\\\\o/"), "{json}");
    }

    #[test]
    fn invalid_packets_cannot_be_encoded() {
        assert_eq!(Packet::LoginStart { name: String::new() }.encode(None), Err(Error::InvalidValue));
        assert_eq!(Packet::LoginStart { name: "a".repeat(17) }.encode(None), Err(Error::InvalidValue));
        let far = Packet::BlockUpdate { pos: Position { x: 1 << 25, y: 0, z: 0 }, block: 1 };
        assert_eq!(far.encode(None), Err(Error::InvalidValue), "paketlenemeyen konum sessizce sarmamalı");
        assert_eq!(frame(0, &vec![0; MAX_PACKET_LEN], None), Err(Error::TooLong));
    }
}
