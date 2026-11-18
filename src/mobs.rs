//! Hayvanlar ve yaratıklar: boşta durma, gezinme, koşma (kaçma / kovalama).
//!
//! Saf kurallardır; rastgelelik yapının kendi üretecinden gelir, çizimi `app`
//! kutulara çevirir.
//!
//! * **Koyun, domuz, tavuk** — bir süre durur, bir süre yürür. Hasar alınca
//!   saldırgandan **koşarak kaçar**.
//! * **Zombi** — gece ortaya çıkar, gezinir; oyuncuyu fark edince **koşarak
//!   kovalar** ve yakındaysa vurur. Gündüz yanar.
//! * **Mantar Kafa** — gece ortaya çıkar; oyuncuyla arasında mesafe tutar
//!   (uzaksa yaklaşır, yakınsa koşarak uzaklaşır) ve **zehir tükürür**.
//!   O da gündüz yanar.
//!
//! Aynı türden iki yetişkin yan yana gelince **yavrular**; yavru zamanla
//! büyür. Her canlı ara sıra ses çıkarır (olay olarak bildirilir).

use crate::blocks::{
    is_liquid, is_plant, is_solid, AIR, APPLE, BONE, BREAD, FISH, GRASS, LEAVES, MEAT, MUSHROOM,
    SEEDS, TORCH, WATER, WHEAT,
};
use crate::player::{Player, GRAVITY};
use crate::protocol::{Error, Reader, Result as ProtoResult, Writer};
use crate::world::{ckey, World, WORLD_HEIGHT};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Sheep,
    Pig,
    Chicken,
    Zombie,
    Shroom,
    Cow,
    Spider,
    Skeleton,
    /// Suda yaşayan balık: yüzer, karada çırpınıp boğulur, oyuncudan kaçar.
    Fish,
    /// Köy evlerinde yaşayan köylü: gündüz evinin çevresinde dolaşır, gece
    /// kapısına döner. Saldırgan değildir.
    Villager,
}

/// Canlının çıkardığı ses türü.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sound {
    /// Ara sıra kendi kendine.
    Idle,
    Hurt,
    /// Zombi ya da örümcek oyuncuyu fark etti.
    Alert,
    /// Mantar Kafa zehir tükürdü.
    Spit,
    /// İskelet ok attı.
    Shoot,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Idle,
    Walk,
    Run,
}

pub struct Stats {
    pub name: &'static str,
    pub radius: f64,
    pub height: f64,
    pub health: i32,
    pub walk: f64,
    pub run: f64,
    /// Ölünce düşen ganimet: `(eşya kimliği, adet)`.
    pub drop: Option<(u16, u32)>,
    /// Ölüm parçacıklarının rengi.
    pub color: [u8; 3],
}

impl Kind {
    pub fn stats(self) -> Stats {
        match self {
            Kind::Sheep => Stats { name: "Koyun", radius: 0.42, height: 1.1, health: 8, walk: 1.3, run: 4.6, drop: Some((MEAT, 2)), color: [236, 236, 228] },
            Kind::Pig => Stats { name: "Domuz", radius: 0.42, height: 0.9, health: 10, walk: 1.4, run: 4.9, drop: Some((MEAT, 2)), color: [238, 164, 164] },
            Kind::Chicken => Stats { name: "Tavuk", radius: 0.24, height: 0.6, health: 4, walk: 1.1, run: 3.9, drop: Some((MEAT, 1)), color: [245, 245, 240] },
            Kind::Cow => Stats { name: "İnek", radius: 0.42, height: 1.25, health: 10, walk: 1.35, run: 4.5, drop: Some((MEAT, 3)), color: [110, 78, 56] },
            Kind::Zombie => Stats { name: "Zombi", radius: 0.3, height: 1.8, health: 20, walk: 1.0, run: 3.6, drop: Some((APPLE, 1)), color: [70, 120, 70] },
            Kind::Shroom => Stats { name: "Mantar Kafa", radius: 0.32, height: 1.3, health: 14, walk: 1.2, run: 3.2, drop: Some((MUSHROOM, 1)), color: [200, 44, 40] },
            Kind::Spider => Stats { name: "Örümcek", radius: 0.48, height: 0.85, health: 16, walk: 2.0, run: 4.0, drop: Some((SEEDS, 2)), color: [52, 46, 56] },
            Kind::Skeleton => Stats { name: "İskelet", radius: 0.3, height: 1.85, health: 16, walk: 1.5, run: 3.0, drop: Some((BONE, 2)), color: [224, 220, 208] },
            Kind::Fish => Stats { name: "Balık", radius: 0.18, height: 0.34, health: 3, walk: 1.5, run: 3.2, drop: Some((FISH, 1)), color: [232, 148, 72] },
            Kind::Villager => Stats { name: "Köylü", radius: 0.3, height: 1.8, health: 14, walk: 1.15, run: 3.4, drop: Some((BREAD, 1)), color: [138, 102, 72] },
        }
    }

    pub fn hostile(self) -> bool {
        matches!(self, Kind::Zombie | Kind::Shroom | Kind::Spider | Kind::Skeleton)
    }

    /// Kendiliğinden (gece/gündüz döngüsüyle) doğan tür mü? Balık suda, köylü
    /// evinde doğar; ikisi de yabani hayvan sayımına ve üremeye girmez.
    pub fn wild(self) -> bool {
        !matches!(self, Kind::Fish | Kind::Villager)
    }

    pub fn is_fish(self) -> bool {
        self == Kind::Fish
    }

    pub fn is_villager(self) -> bool {
        self == Kind::Villager
    }

    /// Yemle çiftleşen hayvan mı? (Balık ve köylü bu listeye girmez.)
    pub fn breedable(self) -> bool {
        matches!(self, Kind::Sheep | Kind::Pig | Kind::Chicken | Kind::Cow)
    }

    /// Hayvanın sevdiği yem mi? (Elde tutulunca takip eder, sağ tıklanınca çiftleşir.)
    pub fn favorite_food(self, item: u16) -> bool {
        match self {
            Kind::Sheep | Kind::Cow => item == WHEAT,
            Kind::Chicken => item == SEEDS,
            Kind::Pig => item == APPLE || item == MUSHROOM,
            _ => false,
        }
    }

    /// Ses dosyası adlarında kullanılan kısa anahtar.
    pub fn key(self) -> &'static str {
        match self {
            Kind::Sheep => "sheep",
            Kind::Pig => "pig",
            Kind::Chicken => "chicken",
            Kind::Cow => "cow",
            Kind::Zombie => "zombie",
            Kind::Shroom => "shroom",
            Kind::Spider => "spider",
            Kind::Skeleton => "skeleton",
            Kind::Fish => "fish",
            Kind::Villager => "villager",
        }
    }

    /// Minecraft protokolü (`Spawn Entity`) ile uyumlu varlık kimliği (`Identifier`).
    pub fn identifier(self) -> &'static str {
        match self {
            Kind::Sheep => "minecraft:sheep",
            Kind::Pig => "minecraft:pig",
            Kind::Chicken => "minecraft:chicken",
            Kind::Cow => "minecraft:cow",
            Kind::Zombie => "minecraft:zombie",
            Kind::Shroom => "voxelcraft:shroom",
            Kind::Spider => "minecraft:spider",
            Kind::Skeleton => "minecraft:skeleton",
            Kind::Fish => "minecraft:cod",
            Kind::Villager => "minecraft:villager",
        }
    }

    pub fn from_identifier(s: &str) -> Option<Self> {
        [
            Kind::Sheep,
            Kind::Pig,
            Kind::Chicken,
            Kind::Cow,
            Kind::Zombie,
            Kind::Shroom,
            Kind::Spider,
            Kind::Skeleton,
            Kind::Fish,
            Kind::Villager,
        ]
        .into_iter()
        .find(|k| k.identifier() == s)
    }

    pub fn to_u8(self) -> u8 {
        match self {
            Kind::Sheep => 0,
            Kind::Pig => 1,
            Kind::Chicken => 2,
            Kind::Zombie => 3,
            Kind::Shroom => 4,
            Kind::Cow => 5,
            Kind::Spider => 6,
            Kind::Skeleton => 7,
            Kind::Fish => 8,
            Kind::Villager => 9,
        }
    }

    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0 => Kind::Sheep,
            1 => Kind::Pig,
            2 => Kind::Chicken,
            3 => Kind::Zombie,
            4 => Kind::Shroom,
            5 => Kind::Cow,
            6 => Kind::Spider,
            7 => Kind::Skeleton,
            8 => Kind::Fish,
            9 => Kind::Villager,
            _ => return None,
        })
    }
}

/// Zombinin oyuncuyu fark ettiği yatay mesafe.
pub const CHASE_RANGE: f64 = 18.0;
const ATTACK_RANGE: f64 = 1.3;
pub const ZOMBIE_DAMAGE: i32 = 3;
pub const ARROW_DAMAGE: i32 = 3;
const ATTACK_INTERVAL: f64 = 1.2;
const FLEE_TIME: f64 = 5.0;
const JUMP_SPEED: f64 = 8.6;
/// Kendiliğinden doğma sınırları; üremeyle `BREED_*` sınırına kadar çıkılır.
const MAX_ANIMALS: usize = 8;
const MAX_ZOMBIES: usize = 4;
const BREED_MAX_ANIMALS: usize = 14;
const BREED_MAX_HOSTILE: usize = 6;
/// Suda yüzen balık ve köy evlerinde yaşayan köylü sınırları.
const MAX_FISH: usize = 12;
const MAX_VILLAGERS: usize = 8;
/// Balığın karada boğulmadan önce çırpındığı süre (saniye).
const FISH_BREATH: f64 = 5.0;
/// Eş arama ve çiftleşme mesafeleri.
const LURE_RANGE: f64 = 16.0;
const BREED_RANGE: f64 = 2.6;
const BREED_COOLDOWN: f64 = 60.0;
/// Yavrunun yetişkin olma süresi (saniye).
const GROW_TIME: f64 = 45.0;
/// Mantar Kafa ve İskelet'in menzili, aralığı ve tutmak istediği mesafe.
const SPIT_RANGE: f64 = 12.0;
const ARROW_RANGE: f64 = 14.0;
const SPIT_INTERVAL: f64 = 2.4;
const ARROW_INTERVAL: f64 = 1.8;
const SPIT_SPEED: f64 = 11.0;
const ARROW_SPEED: f64 = 16.0;
const SPIT_GRAVITY: f64 = 9.0;
const KEEP_FAR: f64 = 7.0;
const KEEP_NEAR: f64 = 3.5;
/// Zehirlenmenin süresi (saniye).
pub const POISON_TIME: f64 = 5.0;
const DESPAWN_DIST: f64 = 90.0;
/// Meşalenin düşman yaratık doğmasını engellediği yarıçap (blok).
pub const TORCH_SAFE_RADIUS: f64 = 10.0;

pub struct Mob {
    pub kind: Kind,
    /// Çarpışma gövdesi (`pos` taban merkezidir).
    pub body: Player,
    /// Baktığı yön (oyuncunun `yaw` kuralıyla).
    pub yaw: f64,
    heading: f64,
    pub state: State,
    timer: f64,
    pub health: i32,
    /// Hasar alınca kısa süre kızarır (1 → 0).
    pub hurt: f64,
    /// Bacak salınımının fazı: kat edilen yolla ilerler.
    pub anim: f64,
    /// Boşta animasyonunun zamanı.
    pub idle_time: f64,
    /// Gündüz yanma zamanlayıcısı (`idle_time`'dan bağımsızdır).
    burn_timer: f64,
    /// Son adımdaki gerçek yatay hız (blok/sn).
    pub speed: f64,
    flee: f64,
    flee_from: [f64; 2],
    attack_cd: f64,
    /// Büyüme: 0 yeni doğmuş yavru, 1 yetişkin.
    pub age: f64,
    breed_cd: f64,
    /// Üremeyle çoğalan / kalıcı hayvanlar uzaklaşınca silinmez.
    pub persistent: bool,
    /// Gece mi? (Örümcek gündüz kışkırtılmadıkça saldırmaz.)
    pub night: bool,
    /// Saldırıya uğradı mı? (Gündüz örümceği kışkırtır.)
    pub provoked: bool,
    /// Yakındaki uygun eşin ya da yem tutan oyuncunun yönü.
    lure: Option<f64>,
    voice: f64,
    spit_cd: f64,
    /// Bu adımda çıkarılan ses / atılan tükürük veya ok; `Mobs::tick` toplar.
    pending_sound: Option<Sound>,
    pending_spit: bool,
    /// Ağ varlık kimliği (`Spawn Entity` paketindeki `entity_id`); atanmadıysa `-1`.
    pub net_id: i32,
    /// İstemcide sunucudan gelen son konum ve yön: çizim bu hedefe yumuşakça yaklaşır.
    pub net_target: Option<([f64; 3], f64)>,
    /// Köylünün evi (kapı önü). Gece buraya döner, gündüz çevresinde dolaşır.
    pub home: Option<[f64; 3]>,
    /// Balığın karada kaldığı süre; dolunca boğulur.
    breath: f64,
    /// Balığın hedeflediği dikey hız (su sütununun içinde kalır).
    vy_target: f64,
}

impl Mob {
    pub fn new(kind: Kind, pos: [f64; 3], yaw: f64) -> Self {
        let s = kind.stats();
        Self {
            kind,
            body: Player { pos, radius: s.radius, height: s.height, ..Player::default() },
            yaw,
            heading: yaw,
            state: State::Idle,
            timer: 1.0,
            health: s.health,
            hurt: 0.0,
            anim: 0.0,
            idle_time: 0.0,
            burn_timer: 0.0,
            speed: 0.0,
            flee: 0.0,
            flee_from: [pos[0], pos[2]],
            attack_cd: 0.0,
            age: 1.0,
            breed_cd: 20.0,
            persistent: false,
            night: true,
            provoked: false,
            lure: None,
            voice: 4.0,
            spit_cd: 1.0,
            pending_sound: None,
            pending_spit: false,
            net_id: -1,
            net_target: None,
            home: None,
            breath: 0.0,
            vy_target: 0.0,
        }
    }

    /// İstemci tarafı: yapay zekâ çalıştırmadan, sunucunun bildirdiği konuma
    /// doğru ara değerle ilerler ve yürüme / koşma animasyonunu hızdan türetir.
    pub fn animate_remote(&mut self, dt: f64) {
        self.hurt = (self.hurt - dt * 3.0).max(0.0);
        self.idle_time += dt;
        let sc = self.scale();
        let s = self.kind.stats();
        self.body.radius = s.radius * sc;
        self.body.height = s.height * sc;
        let Some((target, yaw)) = self.net_target else { return };
        let k = (dt * 10.0).min(1.0);
        let start = self.body.pos;
        for a in 0..3 {
            self.body.pos[a] += (target[a] - self.body.pos[a]) * k;
        }
        let mut diff = (yaw - self.yaw).rem_euclid(std::f64::consts::TAU);
        if diff > std::f64::consts::PI {
            diff -= std::f64::consts::TAU;
        }
        self.yaw += diff * (dt * 8.0).min(1.0);
        let moved = (self.body.pos[0] - start[0]).hypot(self.body.pos[2] - start[2]);
        let speed = if dt > 0.0 { moved / dt } else { 0.0 };
        // Ağ titremesini yumuşatmak için hız da süzülür.
        self.speed += (speed - self.speed) * (dt * 8.0).min(1.0);
        self.anim += moved;
        self.state = if self.speed > s.walk * 1.6 {
            State::Run
        } else if self.speed > 0.15 {
            State::Walk
        } else {
            State::Idle
        };
    }

    /// Yarı boyda ve yarı canda yeni doğmuş yavru oluşturur.
    pub fn new_baby(kind: Kind, pos: [f64; 3], yaw: f64) -> Self {
        let s = kind.stats();
        let mut baby = Self::new(kind, pos, yaw);
        baby.age = 0.0;
        baby.health = (s.health / 2).max(1);
        baby.breed_cd = BREED_COOLDOWN;
        baby.persistent = !kind.hostile();
        let sc = baby.scale();
        baby.body.radius = s.radius * sc;
        baby.body.height = s.height * sc;
        baby
    }

    /// Boyut çarpanı: yavrular yarı boydadır.
    pub fn scale(&self) -> f64 {
        0.5 + 0.5 * self.age.min(1.0)
    }

    pub fn adult(&self) -> bool {
        self.age >= 1.0
    }

    pub fn forward(&self) -> [f64; 2] {
        [-self.yaw.sin(), -self.yaw.cos()]
    }

    /// Görüş konisi (frustum) elemesi için kameraya göre kapsayıcı kutu.
    /// Baş, burun ve uzanan kollar fizik kutusundan biraz taşabildiği için
    /// güvenli bir pay bırakılır.
    pub fn aabb(&self, cam: [f64; 3]) -> ([f32; 3], [f32; 3]) {
        let p = self.body.pos;
        let (rx, ry, rz) = ((p[0] - cam[0]) as f32, (p[1] - cam[1]) as f32, (p[2] - cam[2]) as f32);
        let sc = self.scale() as f32;
        let hw = self.body.radius as f32 + 0.65 * sc;
        let h = self.body.height as f32 + 0.30 * sc;
        ([rx - hw, ry - 0.1, rz - hw], [rx + hw, ry + h, rz + hw])
    }

    /// Canlının kutu parçalarını (gövde, baş ve yürüyüşle sallanan uzuvlar) `boxes`'a ekler.
    pub fn push_boxes(&self, boxes: &mut Vec<crate::effects::BoxInst>) {
        use crate::effects::BoxInst;
        let p = self.body.pos;
        let yaw = self.yaw as f32;
        let f = self.forward();
        let right = [-f[1], f[0]];
        let hurt = self.hurt as f32 * 0.6;
        let tint = |c: [u8; 3]| {
            let mix = |a: u8, b: f32| (a as f32 + (b - a as f32) * hurt) as u8;
            [mix(c[0], 255.0), mix(c[1], 60.0), mix(c[2], 60.0)]
        };
        let running = self.state == State::Run;
        let idle = self.state == State::Idle;
        // Bacak salınımı kat edilen yolla ilerler: koşarken daha geniş ve hızlı.
        let stride = if running { 0.95 } else { 0.5 } * (self.speed / 0.6).min(1.0) as f32;
        let swing = (self.anim * if running { 4.2 } else { 6.0 }).sin() as f32 * stride;
        let (sw_sin, sw_cos) = (swing as f64).sin_cos();
        // Boşta: nefes alıp verir, başını sağa sola çevirir.
        let bob = if idle { (self.idle_time * 2.2).sin() * 0.012 } else { (self.anim * 8.0).sin().abs() * 0.03 };
        let look = if idle { ((self.idle_time * 0.8).sin() * 0.6) as f32 } else { 0.0 };
        let lean = if running { -0.14 } else { 0.0 };
        // Yavrular aynı modelin küçültülmüş hâlidir.
        let sc = self.scale();
        let at = |r: f64, u: f64, fw: f64| {
            let (r, u, fw) = (r * sc, u * sc, fw * sc);
            [p[0] + right[0] * r + f[0] * fw, p[1] + u + bob, p[2] + right[1] * r + f[1] * fw]
        };
        let mut add = |center: [f64; 3], half: [f32; 3], yaw: f32, pitch: f32, color: [u8; 3]| {
            let k = sc as f32;
            boxes.push(BoxInst { center, half: [half[0] * k, half[1] * k, half[2] * k], yaw, pitch, color: tint(color), glow: false });
        };
        // Kalçadan sarkan bacak: canlı başına bir kez alınan `sw_sin`/`sw_cos` ile hesaplanır.
        let leg_box = |r: f64, hip: f64, fw: f64, len: f64, w: f32, sign: f32| {
            let s = sw_sin * sign as f64;
            (at(r, hip - sw_cos * len * 0.5, fw + s * len * 0.5), [w, len as f32 * 0.5, w], swing * sign)
        };
        match self.kind {
            Kind::Sheep | Kind::Pig => {
                let sheep = self.kind == Kind::Sheep;
                let (body, head, legs) = if sheep { ([236, 236, 228], [84, 72, 66], [70, 60, 56]) } else { ([238, 164, 164], [228, 148, 150], [214, 134, 138]) };
                let (by, bh, leg) = if sheep { (0.74, 0.3, 0.46) } else { (0.54, 0.24, 0.32) };
                add(at(0.0, by, 0.0), [0.34, bh as f32, 0.52], yaw, lean, body);
                add(at(0.0, by + 0.16, 0.62), [0.2, 0.2, 0.19], yaw + look, lean, head);
                if !sheep {
                    add(at(0.0, by + 0.1, 0.83), [0.1, 0.07, 0.04], yaw + look, 0.0, [200, 110, 120]);
                }
                for (r, fw, sign) in [(-0.2, 0.36, 1.0), (0.2, 0.36, -1.0), (-0.2, -0.36, -1.0), (0.2, -0.36, 1.0)] {
                    let (c, h, a) = leg_box(r, leg, fw, leg, 0.085, sign);
                    add(c, h, yaw, a, legs);
                }
            }
            Kind::Cow => {
                // Kahverengi-beyaz alacalı gövde, boynuzlu baş ve dört bacak.
                let (coat, patch, head, horn) = ([106, 74, 52], [238, 234, 224], [98, 68, 48], [212, 204, 184]);
                add(at(0.0, 0.82, 0.0), [0.38, 0.34, 0.58], yaw, lean, coat);
                add(at(-0.14, 0.90, 0.12), [0.26, 0.22, 0.22], yaw, lean, patch);
                add(at(0.16, 0.78, -0.22), [0.24, 0.20, 0.20], yaw, lean, patch);
                add(at(0.0, 0.98, 0.66), [0.22, 0.22, 0.20], yaw + look, lean, head);
                add(at(0.0, 0.90, 0.86), [0.14, 0.09, 0.05], yaw + look, 0.0, [218, 188, 166]);
                for side in [-0.22, 0.22] {
                    add(at(side, 1.20, 0.64), [0.035, 0.08, 0.035], yaw + look, 0.0, horn);
                }
                for (r, fw, sign) in [(-0.24, 0.40, 1.0), (0.24, 0.40, -1.0), (-0.24, -0.40, -1.0), (0.24, -0.40, 1.0)] {
                    let (c, h, a) = leg_box(r, 0.50, fw, 0.50, 0.095, sign);
                    add(c, h, yaw, a, coat);
                }
            }
            Kind::Chicken => {
                // Boştayken ara sıra yeri gagalar.
                let peck = if idle { ((self.idle_time * 3.0).sin().max(0.0) * 0.9) as f32 } else { 0.0 };
                add(at(0.0, 0.38, 0.0), [0.15, 0.15, 0.22], yaw, lean, [245, 245, 240]);
                add(at(0.0, 0.58 - peck as f64 * 0.16, 0.2 + peck as f64 * 0.06), [0.09, 0.11, 0.09], yaw + look, -peck, [245, 245, 240]);
                add(at(0.0, 0.56 - peck as f64 * 0.2, 0.32 + peck as f64 * 0.06), [0.04, 0.03, 0.05], yaw + look, -peck, [240, 190, 60]);
                add(at(0.0, 0.71 - peck as f64 * 0.14, 0.2), [0.03, 0.04, 0.06], yaw + look, -peck, [210, 50, 50]);
                for (r, sign) in [(-0.08, 1.0), (0.08, -1.0)] {
                    let (c, h, a) = leg_box(r, 0.24, 0.0, 0.24, 0.03, sign);
                    add(c, h, yaw, a, [240, 190, 60]);
                }
            }
            Kind::Spider => {
                // Alçak gövde, iri karın, parlayan kırmızı gözler ve 8 eklemli bacak.
                let (thorax, abdomen, leg_col) = ([48, 42, 52], [38, 34, 44], [58, 50, 62]);
                add(at(0.0, 0.36, 0.12), [0.22, 0.16, 0.22], yaw + look * 0.5, lean, thorax);
                add(at(0.0, 0.42, -0.32), [0.30, 0.22, 0.32], yaw, lean * 0.5, abdomen);
                add(at(0.0, 0.63, -0.30), [0.10, 0.02, 0.18], yaw, 0.0, [168, 36, 36]);
                for (idx, fw) in [-0.26, -0.08, 0.10, 0.26].into_iter().enumerate() {
                    let phase = if idx % 2 == 0 { 1.0 } else { -1.0 };
                    let step = (sw_sin as f32) * phase * 0.25;
                    for side in [-1.0f64, 1.0f64] {
                        let s = side as f32;
                        add(
                            at(side * 0.40, 0.26 + (step * s).abs() as f64 * 0.10, fw + (step * s) as f64 * 0.10),
                            [0.22, 0.04, 0.04],
                            yaw + step * s,
                            0.30 * s,
                            leg_col,
                        );
                    }
                }
                for side in [-0.10, -0.04, 0.04, 0.10] {
                    let k = sc as f32;
                    boxes.push(BoxInst {
                        center: at(side, 0.40, 0.35),
                        half: [0.025 * k, 0.025 * k, 0.015 * k],
                        yaw: yaw + look * 0.5,
                        pitch: 0.0,
                        color: tint([230, 36, 36]),
                        glow: true,
                    });
                }
            }
            Kind::Shroom => {
                // Sap gövde, kısa bacaklar ve beyaz benekli kırmızı şapka.
                let (stalk, cap, dot) = ([226, 214, 186], [200, 44, 40], [246, 246, 240]);
                // Şapka boştayken hafifçe sallanır, tükürmeden hemen önce geriye yatar.
                let wobble = if idle { ((self.idle_time * 1.7).sin() * 0.07) as f32 } else { 0.0 };
                for (r, sign) in [(-0.12, 1.0), (0.12, -1.0)] {
                    let (c, h, a) = leg_box(r, 0.3, 0.0, 0.3, 0.08, sign);
                    add(c, h, yaw, a, [190, 176, 150]);
                }
                add(at(0.0, 0.66, 0.0), [0.2, 0.36, 0.18], yaw, lean, stalk);
                for r in [-0.09, 0.09] {
                    add(at(r, 0.8, 0.185), [0.035, 0.05, 0.01], yaw, 0.0, [30, 30, 34]);
                }
                add(at(0.0, 1.1, 0.0), [0.44, 0.1, 0.44], yaw + look, wobble, cap);
                add(at(0.0, 1.24, 0.0), [0.3, 0.08, 0.3], yaw + look, wobble, cap);
                for (r, fw, u) in [(-0.24, 0.18, 1.21), (0.2, -0.22, 1.21), (0.26, 0.24, 1.21), (-0.08, -0.06, 1.33), (0.1, 0.12, 1.33)] {
                    add(at(r, u, fw), [0.06, 0.012, 0.06], yaw + look, wobble, dot);
                }
            }
            Kind::Skeleton => {
                // Kemik rengi ince iskelet gövdesi ve elinde ahşap yay.
                let (bone, dark, bow_wood) = ([226, 222, 210], [36, 36, 40], [138, 96, 52]);
                add(at(0.0, 1.15, 0.0), [0.20, 0.34, 0.10], yaw, lean, bone);
                add(at(0.0, 0.84, 0.0), [0.18, 0.06, 0.10], yaw, lean, bone);
                add(at(0.0, 1.68, 0.0), [0.20, 0.20, 0.20], yaw + look, 0.0, bone);
                for r in [-0.08, 0.08] {
                    add(at(r, 1.70, 0.205), [0.04, 0.035, 0.01], yaw + look, 0.0, dark);
                }
                for (r, sign) in [(-0.11, 1.0), (0.11, -1.0)] {
                    let (c, h, a) = leg_box(r, 0.76, 0.0, 0.76, 0.06, sign);
                    add(c, h, yaw, a, bone);
                }
                for r in [-0.28, 0.28] {
                    add(at(r, 1.34, 0.28), [0.055, 0.055, 0.30], yaw, 0.15, bone);
                }
                // Yay: sağ elin önünde dikey kavis ve kiriş.
                add(at(0.22, 1.34, 0.56), [0.025, 0.26, 0.03], yaw, 0.0, bow_wood);
                add(at(0.22, 1.34, 0.48), [0.01, 0.25, 0.01], yaw, 0.0, [236, 236, 230]);
            }
            Kind::Zombie => {
                let (skin, shirt, pants) = ([86, 150, 86], [50, 122, 142], [58, 60, 120]);
                add(at(0.0, 1.12, 0.0), [0.25, 0.37, 0.13], yaw, lean, shirt);
                add(at(0.0, 1.69, 0.0), [0.2, 0.2, 0.2], yaw + look, 0.0, skin);
                for (r, sign) in [(-0.13, 1.0), (0.13, -1.0)] {
                    let (c, h, a) = leg_box(r, 0.75, 0.0, 0.75, 0.11, sign);
                    add(c, h, yaw, a, pants);
                }
                // Kovalarken kollar öne uzanır; gezinirken bacakların tersine sallanır.
                let arm_wave = if running { (self.anim * 5.0).sin() as f32 * 0.1 } else { 0.0 };
                for (r, sign) in [(-0.36, -1.0), (0.36, 1.0)] {
                    let angle = if running { 1.45 + arm_wave * sign } else { swing * sign * 0.8 };
                    let (a_sin, a_cos) = (angle as f64).sin_cos();
                    let c = at(r, 1.42 - a_cos * 0.35, a_sin * 0.35);
                    add(c, [0.09, 0.35, 0.09], yaw, angle, skin);
                }
            }
            Kind::Fish => {
                // Küçük gövde, sallanan kuyruk, sırt yüzgeci ve iki göz.
                // Yüzerken dikey hıza göre hafifçe yatar.
                let (body, fin, belly) = ([236, 138, 62], [250, 206, 130], [214, 96, 48]);
                let wag = ((self.anim * 11.0).sin() * 0.5) as f32;
                let tilt = (self.vy_target * 0.35).clamp(-0.5, 0.5) as f32;
                add(at(0.0, 0.16, 0.0), [0.065, 0.1, 0.2], yaw, tilt, body);
                add(at(0.0, 0.1, 0.03), [0.055, 0.05, 0.15], yaw, tilt, belly);
                add(at(0.0, 0.16, -0.25), [0.014, 0.09, 0.08], yaw + wag, tilt, fin);
                add(at(0.0, 0.28, 0.0), [0.01, 0.06, 0.08], yaw, tilt, fin);
                for side in [-0.06, 0.06] {
                    add(at(side, 0.2, 0.15), [0.015, 0.015, 0.014], yaw, tilt, [26, 26, 32]);
                }
            }
            Kind::Villager => {
                // Cübbe, yanda kollar, önde kavuşturulmuş eller, iri burun ve
                // tek kaş. Bacaklar cübbenin altında boşluksuz durur.
                let (robe, robe_dark, skin, nose, hair) =
                    ([124, 84, 56], [100, 66, 44], [230, 182, 138], [206, 148, 104], [54, 38, 26]);
                for (r, sign) in [(-0.11, 1.0), (0.11, -1.0)] {
                    let (c, h, a) = leg_box(r, 0.52, 0.0, 0.52, 0.1, sign);
                    add(c, h, yaw, a, robe_dark);
                }
                add(at(0.0, 0.98, 0.0), [0.27, 0.46, 0.17], yaw, lean, robe);
                for r in [-0.31, 0.31] {
                    add(at(r, 1.2, 0.02), [0.08, 0.24, 0.08], yaw, lean, robe);
                }
                add(at(0.0, 1.06, 0.2), [0.16, 0.07, 0.07], yaw, 0.0, robe_dark);
                add(at(0.0, 1.65, 0.0), [0.2, 0.2, 0.2], yaw + look, 0.0, skin);
                add(at(0.0, 1.6, 0.22), [0.055, 0.075, 0.06], yaw + look, 0.0, nose);
                add(at(0.0, 1.71, 0.202), [0.15, 0.022, 0.01], yaw + look, 0.0, [46, 34, 26]);
                for r in [-0.08, 0.08] {
                    add(at(r, 1.67, 0.202), [0.032, 0.03, 0.01], yaw + look, 0.0, [34, 34, 42]);
                }
                add(at(0.0, 1.83, 0.0), [0.21, 0.05, 0.21], yaw + look, 0.0, hair);
            }
        }
    }

    /// Hasar verir; hayvanlar `from` noktasından koşarak kaçmaya başlar,
    /// örümcek gündüz bile kışkırtılmış olur. Öldüyse `true`.
    pub fn damage(&mut self, amount: i32, from: [f64; 3]) -> bool {
        self.health -= amount;
        self.hurt = 1.0;
        self.provoked = true;
        self.pending_sound = Some(Sound::Hurt);
        // Geri savrulma.
        let (dx, dz) = (self.body.pos[0] - from[0], self.body.pos[2] - from[2]);
        let d = dx.hypot(dz).max(1e-6);
        self.body.pos[1] += 0.02;
        self.body.vel = [dx / d * 4.0, 4.5, dz / d * 4.0];
        if !self.kind.hostile() {
            self.flee = FLEE_TIME;
            self.flee_from = [from[0], from[2]];
        }
        self.health <= 0
    }

    /// Bir adım. Yakın dövüş yaratığı bu adımda oyuncuya vurduysa `true` döner.
    pub fn update(&mut self, world: &World, dt: f64, player: [f64; 3], rand: &mut dyn FnMut() -> f64) -> bool {
        // Balık suda yüzer: yerçekimi ve zemin yürüyüşü yerine kendi hareketi.
        if self.kind.is_fish() {
            return self.update_fish(world, dt, player, rand);
        }
        let s = self.kind.stats();
        self.hurt = (self.hurt - dt * 3.0).max(0.0);
        self.attack_cd = (self.attack_cd - dt).max(0.0);
        self.timer -= dt;
        self.idle_time += dt;
        self.breed_cd = (self.breed_cd - dt).max(0.0);
        self.spit_cd = (self.spit_cd - dt).max(0.0);
        // Büyüme: gövde de boyla birlikte büyür; yetişkinliğe ulaşınca kalan can eklenir.
        if self.age < 1.0 {
            self.age = (self.age + dt / GROW_TIME).min(1.0);
            if self.age >= 1.0 {
                let baby_max = (s.health / 2).max(1);
                self.health = (self.health + (s.health - baby_max)).min(s.health);
            }
        }
        self.body.radius = s.radius * self.scale();
        self.body.height = s.height * self.scale();
        // Ara sıra kendi kendine ses çıkarır.
        self.voice -= dt;
        if self.voice <= 0.0 {
            self.voice = 6.0 + rand() * 9.0;
            self.pending_sound.get_or_insert(Sound::Idle);
        }
        let pos = self.body.pos;
        let (px, pz) = (player[0] - pos[0], player[2] - pos[2]);
        let dist = px.hypot(pz);
        let mut attacked = false;
        let mut hold = false;

        // Örümcek gündüz kışkırtılmadıkça uysaldır; gece veya hasar alınca saldırır.
        let spider_passive = self.kind == Kind::Spider && !self.night && !self.provoked;

        if matches!(self.kind, Kind::Shroom | Kind::Skeleton) {
            let (range, interval, sound) = if self.kind == Kind::Skeleton {
                (ARROW_RANGE, ARROW_INTERVAL, Sound::Shoot)
            } else {
                (SPIT_RANGE, SPIT_INTERVAL, Sound::Spit)
            };
            if dist < CHASE_RANGE && (player[1] - pos[1]).abs() < 8.0 {
                // Mesafeyi korur: uzaksa yaklaşır, fazla yakınsa koşarak uzaklaşır.
                let toward = (-px).atan2(-pz);
                if dist < KEEP_NEAR {
                    self.state = State::Run;
                    self.heading = toward + std::f64::consts::PI;
                } else {
                    self.heading = toward;
                    if dist > KEEP_FAR {
                        self.state = State::Walk;
                    } else {
                        self.state = State::Idle;
                    }
                    self.timer = 1.0;
                    if dist < range && self.spit_cd == 0.0 {
                        self.spit_cd = interval;
                        self.pending_spit = true;
                        self.pending_sound = Some(sound);
                    }
                }
            } else if self.state == State::Run {
                self.state = State::Idle;
                self.timer = 1.0;
            }
        } else if self.kind.hostile() && !spider_passive {
            if dist < CHASE_RANGE && (player[1] - pos[1]).abs() < 8.0 {
                // Oyuncuyu gördü: koşarak kovala.
                if self.state != State::Run {
                    self.pending_sound = Some(Sound::Alert);
                }
                self.state = State::Run;
                self.heading = (-px).atan2(-pz);
                if dist < ATTACK_RANGE {
                    hold = true;
                    if self.attack_cd == 0.0 {
                        self.attack_cd = ATTACK_INTERVAL;
                        attacked = true;
                    }
                }
            } else if self.state == State::Run {
                self.state = State::Idle;
                self.timer = 1.0;
            }
        } else if self.flee > 0.0 {
            // Korktu: tehditten uzağa koş.
            self.flee -= dt;
            self.state = State::Run;
            let (ax, az) = (pos[0] - self.flee_from[0], pos[2] - self.flee_from[1]);
            if ax.hypot(az) > 1e-6 {
                self.heading = (-ax).atan2(-az);
            }
        } else if self.kind.is_villager() {
            // Köylü evine bağlıdır: gece kapısına döner, gündüz evin
            // çevresinden fazla uzaklaşmaz.
            if let Some(home) = self.home {
                let (hx, hz) = (home[0] - pos[0], home[2] - pos[2]);
                let limit = if self.night { 1.4 } else { 11.0 };
                if hx.hypot(hz) > limit {
                    self.state = State::Walk;
                    self.heading = (-hx).atan2(-hz);
                    self.timer = 0.6;
                }
            }
        } else if self.state == State::Run {
            self.state = State::Idle;
            self.timer = 1.5;
        }

        // Gezinme: bir süre dur, bir süre rastgele bir yöne yürü.
        if self.state != State::Run && self.timer <= 0.0 {
            if self.state == State::Idle {
                self.state = State::Walk;
                // Üremeye hazırsa ve yakında eş (ya da yem tutan oyuncu) varsa ona doğru yürür.
                let random = rand() * std::f64::consts::TAU;
                self.heading = self.lure.unwrap_or(random);
                self.timer = 2.0 + rand() * 3.5;
            } else {
                self.state = State::Idle;
                self.timer = 1.5 + rand() * 3.0;
            }
        }

        // Yön yumuşakça hedefe döner.
        let mut diff = (self.heading - self.yaw).rem_euclid(std::f64::consts::TAU);
        if diff > std::f64::consts::PI {
            diff -= std::f64::consts::TAU;
        }
        self.yaw += diff * (dt * 7.0).min(1.0);

        let mut speed = match self.state {
            State::Idle => 0.0,
            State::Walk => s.walk,
            State::Run => s.run,
        };
        if hold {
            speed = 0.0;
        }
        let f = self.forward();

        // Gezinirken suya ve uçuruma yürümez: önü uygun değilse yön değiştirir.
        // (Mantar Kafa ve İskelet oyuncuya yürürken yönünü kendi belirler.)
        if self.state == State::Walk && speed > 0.0 && !matches!(self.kind, Kind::Shroom | Kind::Skeleton) {
            let (ax, az) = ((pos[0] + f[0] * 0.9).floor() as i32, (pos[2] + f[1] * 0.9).floor() as i32);
            let y = pos[1].floor() as i32;
            let water = world.get_block(ax, y, az) == WATER || world.get_block(ax, y - 1, az) == WATER;
            let cliff = (1..=3).all(|d| !is_solid(world.get_block(ax, y - d, az)));
            if water || cliff {
                self.heading += std::f64::consts::PI * (0.6 + rand() * 0.8);
                speed = 0.0;
            }
        }

        // Savrulma hızı (hasar) yatayda söner; yürüyüş hızı üstüne eklenir.
        let damp = (1.0 - dt * 4.0).max(0.0);
        self.body.vel[0] *= damp;
        self.body.vel[2] *= damp;
        let (dx, dz) = ((f[0] * speed + self.body.vel[0]) * dt, (f[1] * speed + self.body.vel[2]) * dt);
        let want = dx.hypot(dz);
        let grounded = self.body.on_ground;
        let start = self.body.pos;
        let knock = [self.body.vel[0], self.body.vel[2]];
        self.body.move_with_collision(world, dx, 0.0, dz);
        // `move_with_collision` çarpınca hızı sıfırlar; savrulma sönümlenerek sürsün.
        if self.body.vel[0] != 0.0 {
            self.body.vel[0] = knock[0];
        }
        if self.body.vel[2] != 0.0 {
            self.body.vel[2] = knock[1];
        }
        let moved = (self.body.pos[0] - start[0]).hypot(self.body.pos[2] - start[2]);

        // Önü kapalıysa: Örümcek duvara tırmanır (havadayken de), diğerleri tek basamak zıplar.
        if speed > 0.0 && want > 1e-6 && moved < want * 0.5 {
            if self.kind == Kind::Spider {
                self.body.vel[1] = 4.2;
            } else if grounded {
                self.body.vel[1] = JUMP_SPEED;
            }
        }
        if self.body.feet_in_water(world) {
            // Suda yüzeyde kalır.
            self.body.vel[1] += (2.2 - self.body.vel[1]) * (dt * 6.0).min(1.0);
        } else {
            self.body.vel[1] = (self.body.vel[1] - GRAVITY * dt).max(-50.0);
        }
        self.body.on_ground = false;
        let vy = self.body.vel[1];
        self.body.move_with_collision(world, 0.0, vy * dt, 0.0);

        self.anim += moved;
        self.speed = if dt > 0.0 { moved / dt } else { 0.0 };
        attacked
    }

    /// Balık hareketi: su sütununun içinde yüzer, oyuncu yaklaşınca kaçar,
    /// karaya çıkarsa çırpınıp boğulur.
    fn update_fish(&mut self, world: &World, dt: f64, player: [f64; 3], rand: &mut dyn FnMut() -> f64) -> bool {
        let s = self.kind.stats();
        self.hurt = (self.hurt - dt * 3.0).max(0.0);
        self.timer -= dt;
        self.idle_time += dt;
        self.voice -= dt;
        if self.voice <= 0.0 {
            self.voice = 9.0 + rand() * 12.0;
            self.pending_sound.get_or_insert(Sound::Idle);
        }
        let pos = self.body.pos;
        let (bx, bz) = (pos[0].floor() as i32, pos[2].floor() as i32);
        let by = pos[1].floor() as i32;
        let submerged = world.get_block(bx, by, bz) == WATER;

        // Karada: çırpınır, bir süre sonra boğulur.
        if !submerged {
            self.breath += dt;
            self.vy_target = 0.0;
            if self.breath > FISH_BREATH {
                self.breath = FISH_BREATH * 0.5;
                self.health -= 1;
                self.hurt = 1.0;
            }
            self.state = State::Run;
            let f = self.forward();
            self.body.move_with_collision(world, f[0] * s.walk * dt, 0.0, f[1] * s.walk * dt);
            self.body.vel[1] = (self.body.vel[1] - GRAVITY * dt).max(-50.0);
            let vy = self.body.vel[1];
            self.body.move_with_collision(world, 0.0, vy * dt, 0.0);
            if self.body.on_ground {
                self.body.vel[1] = JUMP_SPEED * 0.4;
                self.heading = rand() * std::f64::consts::TAU;
            }
            self.body.on_ground = false;
            self.yaw = self.heading;
            self.anim += 0.4;
            return false;
        }

        self.breath = 0.0;
        let (px, pz) = (player[0] - pos[0], player[2] - pos[2]);
        // Oyuncu yaklaşınca kaçar; yoksa rastgele yön ve derinlik seçer.
        if px.hypot(pz) < 4.5 && (player[1] - pos[1]).abs() < 3.0 {
            self.state = State::Run;
            self.heading = px.atan2(pz); // oyuncudan uzağa
            self.timer = 0.6;
        } else if self.timer <= 0.0 {
            self.state = if rand() < 0.3 { State::Idle } else { State::Walk };
            self.heading = rand() * std::f64::consts::TAU;
            self.vy_target = (rand() - 0.5) * s.walk;
            self.timer = 1.2 + rand() * 2.4;
        }
        // Su sütununun içinde kal: yüzeye çıkmak üzereyse aşağı, tabana
        // yaklaşıyorsa yukarı yüzer.
        let head = (pos[1] + self.body.height).floor() as i32;
        let foot = (pos[1] - 0.05).floor() as i32;
        if world.get_block(bx, head + 1, bz) != WATER {
            self.vy_target = -s.walk * 0.7;
        } else if world.get_block(bx, foot - 1, bz) != WATER {
            self.vy_target = s.walk * 0.7;
        }
        let mut diff = (self.heading - self.yaw).rem_euclid(std::f64::consts::TAU);
        if diff > std::f64::consts::PI {
            diff -= std::f64::consts::TAU;
        }
        self.yaw += diff * (dt * 6.0).min(1.0);
        let f = self.forward();
        let mut speed = match self.state {
            State::Idle => 0.0,
            State::Walk => s.walk,
            State::Run => s.run,
        };
        // Önündeki hücre su değilse (kıyı) karaya çıkmaz: döner.
        let ahead = [
            (pos[0] + f[0] * 0.45).floor() as i32,
            (pos[1] + self.body.height * 0.5).floor() as i32,
            (pos[2] + f[1] * 0.45).floor() as i32,
        ];
        if world.get_block(ahead[0], ahead[1], ahead[2]) != WATER {
            self.heading += std::f64::consts::PI * (0.5 + rand() * 0.8);
            speed = 0.0;
        }
        let want = (f[0] * speed * dt).hypot(f[1] * speed * dt);
        let start = self.body.pos;
        self.body.move_with_collision(world, f[0] * speed * dt, self.vy_target * dt, f[1] * speed * dt);
        let moved = (self.body.pos[0] - start[0]).hypot(self.body.pos[2] - start[2]);
        // Önü kapalıysa (kaya, kıyı) döner.
        if want > 1e-6 && moved < want * 0.5 {
            self.heading += std::f64::consts::PI * (0.5 + rand() * 0.8);
        }
        self.anim += moved;
        self.speed = if dt > 0.0 { moved / dt } else { 0.0 };
        false
    }
}

pub enum MobEvent {
    /// Yakın dövüş yaratığı `(oyuncu sırası, hasar)` oyuncuya vurdu.
    Attack(usize, i32),
    /// İskelet oku `(oyuncu sırası, hasar)` oyuncuya isabet etti.
    ArrowHit(usize, i32),
    /// Bir canlı öldü: `(tür, konum, ağ kimliği)` (ganimet ve parçacıklar için).
    Died(Kind, [f64; 3], i32),
    /// Bir canlı ses çıkardı.
    Sound(Kind, Sound, [f64; 3]),
    /// Bir yavru doğdu.
    Born(Kind, [f64; 3]),
    /// Zehir tükürüğü ya da ok bir yere çarptı.
    SpitHit([f64; 3]),
    /// Zehir tükürüğü `(oyuncu sırası)` oyuncuya isabet etti.
    Poison(usize),
}

/// Havada uçan zehir tükürüğü veya iskelet oku.
pub struct Spit {
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub arrow: bool,
    life: f64,
}

pub struct Mobs {
    pub list: Vec<Mob>,
    pub spits: Vec<Spit>,
    rng: u32,
    spawn_timer: f64,
    breed_timer: f64,
    /// Balık ve köylü doğurma zamanlayıcıları (ayrı halkalar).
    fish_timer: f64,
    village_timer: f64,
    /// Çok oyunculuda doğmanın denendiği oyuncunun sırası.
    spawn_turn: usize,
    /// Ağ üzerinden senkronize edilen canlılara atanan artan kimlik.
    pub next_id: i32,
}

impl Default for Mobs {
    fn default() -> Self {
        Self {
            list: Vec::new(),
            spits: Vec::new(),
            rng: 0x9e37_79b9,
            spawn_timer: 2.0,
            breed_timer: 1.0,
            fish_timer: 1.5,
            village_timer: 0.5,
            spawn_turn: 0,
            next_id: 1000,
        }
    }
}

/// `(x, z)` sütununun en üst bloğu: `(y, blok)`. Chunk yüklü değilse ya da
/// yüzey su ise `None`. Ot ve çiçeklerin üstünden değil, altındaki zeminden doğar.
fn surface(world: &World, x: i32, z: i32) -> Option<(i32, u8)> {
    for y in (1..WORLD_HEIGHT).rev() {
        let b = world.get_block(x, y, z);
        if b == AIR || b == TORCH || is_plant(b) {
            continue;
        }
        return if is_liquid(b) { None } else { Some((y, b)) };
    }
    None
}

impl Mobs {
    fn rand(&mut self) -> f64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        self.rng as f64 / 4_294_967_296.0
    }

    fn alloc_id(&mut self) -> i32 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1000);
        id
    }

    pub fn count(&self, hostile: bool) -> usize {
        self.list
            .iter()
            .filter(|m| m.kind.wild() && m.kind.hostile() == hostile)
            .count()
    }

    pub fn count_fish(&self) -> usize {
        self.list.iter().filter(|m| m.kind.is_fish()).count()
    }

    pub fn count_villagers(&self) -> usize {
        self.list.iter().filter(|m| m.kind.is_villager()).count()
    }

    pub fn spawn(&mut self, kind: Kind, pos: [f64; 3]) {
        let yaw = self.rand() * std::f64::consts::TAU;
        let mut mob = Mob::new(kind, pos, yaw);
        mob.net_id = self.alloc_id();
        self.list.push(mob);
    }

    /// Oyuncunun çevresinde, görüş alanının biraz uzağında yeni canlı dener.
    fn try_spawn(&mut self, world: &World, player: [f64; 3], night: bool, torches: &[[i32; 3]]) {
        let hostile = night && self.count(true) < MAX_ZOMBIES && self.rand() < 0.6;
        let near_animals = self
            .list
            .iter()
            .filter(|m| m.kind.wild() && !m.kind.hostile() && (m.body.pos[0] - player[0]).hypot(m.body.pos[2] - player[2]) < DESPAWN_DIST)
            .count();
        if !hostile && near_animals >= MAX_ANIMALS {
            return;
        }
        let angle = self.rand() * std::f64::consts::TAU;
        let dist = 18.0 + self.rand() * 20.0;
        let (x, z) = ((player[0] + angle.cos() * dist).floor() as i32, (player[2] + angle.sin() * dist).floor() as i32);
        let Some((y, block)) = surface(world, x, z) else { return };
        // Hayvanlar çimende doğar; yaratıklar ağaç tepesi dışında her yerde.
        if (!hostile && block != GRASS) || block == LEAVES {
            return;
        }
        let pos = [x as f64 + 0.5, y as f64 + 1.0, z as f64 + 0.5];
        // Düşman yaratıklar meşalelerin aydınlattığı güvenli yarıçapta doğmaz.
        if hostile
            && torches.iter().any(|t| {
                let dx = t[0] as f64 + 0.5 - pos[0];
                let dy = t[1] as f64 + 0.5 - pos[1];
                let dz = t[2] as f64 + 0.5 - pos[2];
                dx * dx + dy * dy + dz * dz < TORCH_SAFE_RADIUS * TORCH_SAFE_RADIUS
            })
        {
            return;
        }
        let kind = if hostile {
            [Kind::Zombie, Kind::Shroom, Kind::Spider, Kind::Skeleton][(self.rand() * 4.0) as usize % 4]
        } else {
            [Kind::Sheep, Kind::Pig, Kind::Chicken, Kind::Cow][(self.rand() * 4.0) as usize % 4]
        };
        let mut mob = Mob::new(kind, pos, angle);
        if !mob.body.collides(world) {
            mob.net_id = self.alloc_id();
            self.list.push(mob);
        }
    }

    /// Oyuncunun çevresindeki su sütunlarında balık doğurur. Su yoksa (ya da
    /// sütun bir bloktan sığsa) hiçbir şey yapmaz.
    fn try_spawn_fish(&mut self, world: &World, player: [f64; 3]) {
        if self.count_fish() >= MAX_FISH {
            return;
        }
        let angle = self.rand() * std::f64::consts::TAU;
        let dist = 8.0 + self.rand() * 26.0;
        let (x, z) = ((player[0] + angle.cos() * dist).floor() as i32, (player[2] + angle.sin() * dist).floor() as i32);
        // Yukarıdan aşağı: ilk su bloğu yüzey, ilk katı blok taban.
        let mut top = None;
        for y in (1..WORLD_HEIGHT).rev() {
            let b = world.get_block(x, y, z);
            if b == WATER {
                top = Some(y);
                break;
            }
            if is_solid(b) {
                return;
            }
        }
        let Some(top) = top else { return };
        let mut bottom = top;
        while bottom > 1 && world.get_block(x, bottom - 1, z) == WATER {
            bottom -= 1;
        }
        if top - bottom < 1 {
            return;
        }
        let y = bottom + (self.rand() * (top - bottom) as f64) as i32;
        let pos = [x as f64 + 0.5, y as f64 + 0.5, z as f64 + 0.5];
        let mut mob = Mob::new(Kind::Fish, pos, angle);
        if !mob.body.collides(world) {
            mob.net_id = self.alloc_id();
            self.list.push(mob);
        }
    }

    /// Yakındaki bir köy evinin kapısında köylü doğurur. Aynı evde zaten bir
    /// köylü varsa yenisi gelmez; köylüler kalıcıdır (uzaklaşınca silinmez).
    fn try_spawn_villager(&mut self, world: &World, player: [f64; 3]) {
        if self.count_villagers() >= MAX_VILLAGERS {
            return;
        }
        let homes = world.village_homes(player[0] as i32, player[2] as i32, 64);
        if homes.is_empty() {
            return;
        }
        let home = homes[(self.rand() * homes.len() as f64) as usize % homes.len()];
        if self.list.iter().any(|m| {
            m.kind.is_villager() && (m.body.pos[0] - home[0]).hypot(m.body.pos[2] - home[2]) < 6.0
        }) {
            return;
        }
        let mut mob = Mob::new(Kind::Villager, home, self.rand() * std::f64::consts::TAU);
        mob.home = Some(home);
        mob.persistent = true;
        if !mob.body.collides(world) {
            mob.net_id = self.alloc_id();
            self.list.push(mob);
        }
    }

    /// Oyuncu elinde yem tutuyorsa yakın hayvanları kendine doğru çeker.
    pub fn lure_with_item(&mut self, player: [f64; 3], held_item: Option<u16>) {
        let Some(item) = held_item else { return };
        for m in self.list.iter_mut() {
            if m.kind.hostile() || !m.kind.favorite_food(item) {
                continue;
            }
            let (dx, dz) = (player[0] - m.body.pos[0], player[2] - m.body.pos[2]);
            let dist = dx.hypot(dz);
            if dist < LURE_RANGE && dist > 1.8 {
                let toward = (-dx).atan2(-dz);
                m.lure = Some(toward);
                if m.state == State::Idle {
                    m.state = State::Walk;
                    m.heading = toward;
                    m.timer = 1.2;
                }
            }
        }
    }

    /// `i` sırasındaki hayvanı `item` ile besler: canını yeniler, kalıcı yapar ve
    /// üreme bekleme süresini sıfırlar (yavruysa büyümesini hızlandırır).
    pub fn feed(&mut self, i: usize, item: u16) -> bool {
        let Some(m) = self.list.get_mut(i) else { return false };
        if m.kind.hostile() || !m.kind.favorite_food(item) {
            return false;
        }
        let max_hp = m.kind.stats().health;
        m.health = (m.health + 2).min(max_hp);
        m.persistent = true;
        if m.adult() {
            m.breed_cd = 0.0;
        } else {
            m.age = (m.age + 0.35).min(1.0);
        }
        true
    }

    /// Tüm canlıları tek oyunculu oyunda bir adım ilerletir; olanları `events`'e yazar.
    pub fn tick(
        &mut self,
        world: &World,
        dt: f64,
        player: [f64; 3],
        night: bool,
        torches: &[[i32; 3]],
        events: &mut Vec<MobEvent>,
    ) {
        self.tick_multi(world, dt, &[player], night, torches, events);
    }

    /// Çok oyunculu adım: her canlı **en yakın** oyuncuyu hedefler, doğma
    /// sırayla oyuncuların çevresinde denenir ve canlı ancak tüm oyunculardan
    /// uzaklaşınca silinir. Olaylardaki oyuncu sırası `players` dizisindeki sıradır.
    pub fn tick_multi(
        &mut self,
        world: &World,
        dt: f64,
        players: &[[f64; 3]],
        night: bool,
        torches: &[[i32; 3]],
        events: &mut Vec<MobEvent>,
    ) {
        if players.is_empty() {
            return;
        }
        self.spawn_timer -= dt;
        if self.spawn_timer <= 0.0 {
            self.spawn_timer = 1.5 / players.len() as f64;
            self.spawn_turn = (self.spawn_turn + 1) % players.len();
            self.try_spawn(world, players[self.spawn_turn], night, torches);
        }

        // Balıklar suda, köylüler evlerinde doğar; ikisi de yabani hayvan
        // sınırından bağımsızdır.
        self.fish_timer -= dt;
        if self.fish_timer <= 0.0 {
            self.fish_timer = 2.5;
            self.try_spawn_fish(world, players[self.spawn_turn]);
        }
        self.village_timer -= dt;
        if self.village_timer <= 0.0 {
            self.village_timer = 3.0;
            self.try_spawn_villager(world, players[self.spawn_turn]);
        }

        self.breed_timer -= dt;
        if self.breed_timer <= 0.0 {
            self.breed_timer = 1.0;
            self.breed(night, events);
        }

        let nearest = |p: [f64; 3]| -> usize {
            (0..players.len())
                .min_by(|&a, &b| {
                    let d = |i: usize| (players[i][0] - p[0]).powi(2) + (players[i][2] - p[2]).powi(2);
                    d(a).total_cmp(&d(b))
                })
                .unwrap_or(0)
        };
        let chests: Vec<[f64; 3]> = players.iter().map(|p| [p[0], p[1] + 0.9, p[2]]).collect();
        let mut rng = self.rng;
        let mut rand = move || {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            rng as f64 / 4_294_967_296.0
        };
        for m in self.list.iter_mut() {
            // Uzaktaki kalıcı hayvanın chunk'ı yüklü değilse fiziği dondurulur
            // (yüklü olmayan zemin `AIR` döndürdüğü için boşluğa düşmesin).
            let (cx, cz) = ((m.body.pos[0].floor() as i32) >> 4, (m.body.pos[2].floor() as i32) >> 4);
            if !world.chunks.contains_key(&ckey(cx, cz)) {
                continue;
            }
            m.night = night;
            let target = nearest(m.body.pos);
            let player = players[target];
            let chest = chests[target];
            if m.update(world, dt, player, &mut rand) {
                events.push(MobEvent::Attack(target, ZOMBIE_DAMAGE));
            }
            if let Some(sound) = m.pending_sound.take() {
                events.push(MobEvent::Sound(m.kind, sound, m.body.pos));
            }
            if std::mem::take(&mut m.pending_spit) {
                // Oyuncunun göğsüne nişan alır; düşüşü karşılayacak kadar yukarı atar.
                let arrow = m.kind == Kind::Skeleton;
                let spd = if arrow { ARROW_SPEED } else { SPIT_SPEED };
                let from = [m.body.pos[0], m.body.pos[1] + m.body.height * 0.85, m.body.pos[2]];
                let d = [chest[0] - from[0], chest[1] - from[1], chest[2] - from[2]];
                let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
                let t = len / spd;
                let vel = [d[0] / len * spd, d[1] / len * spd + 0.5 * SPIT_GRAVITY * t, d[2] / len * spd];
                self.spits.push(Spit { pos: from, vel, arrow, life: 3.0 });
            }
            // Gece yaratıkları (örümcek hariç) gün ışığında yanar.
            if m.kind.hostile() && m.kind != Kind::Spider && !night {
                m.burn_timer += dt;
                while m.burn_timer >= 0.5 {
                    m.burn_timer -= 0.5;
                    m.health -= 2;
                    m.hurt = 1.0;
                }
            } else {
                m.burn_timer = 0.0;
            }
        }
        self.rng = self.rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223) | 1;

        // Tükürükler ve oklar: küçük adımlarla uçar; bloğa ya da oyuncuya çarpınca biter.
        self.spits.retain_mut(|s| {
            s.life -= dt;
            s.vel[1] -= SPIT_GRAVITY * dt;
            let steps = 4;
            for _ in 0..steps {
                for a in 0..3 {
                    s.pos[a] += s.vel[a] * dt / steps as f64;
                }
                let hit = chests.iter().position(|c| (0..3).map(|a| (s.pos[a] - c[a]).powi(2)).sum::<f64>() < 0.75 * 0.75);
                if let Some(target) = hit {
                    if s.arrow {
                        events.push(MobEvent::ArrowHit(target, ARROW_DAMAGE));
                    } else {
                        events.push(MobEvent::Poison(target));
                    }
                    events.push(MobEvent::SpitHit(s.pos));
                    return false;
                }
                if is_solid(world.get_block(s.pos[0].floor() as i32, s.pos[1].floor() as i32, s.pos[2].floor() as i32)) {
                    events.push(MobEvent::SpitHit(s.pos));
                    return false;
                }
            }
            s.life > 0.0
        });

        self.list.retain(|m| {
            if m.health <= 0 {
                events.push(MobEvent::Died(m.kind, m.body.pos, m.net_id));
                return false;
            }
            let p = m.body.pos;
            let near = players.iter().any(|pl| (p[0] - pl[0]).hypot(p[2] - pl[2]) < DESPAWN_DIST);
            p[1] > -10.0 && (m.persistent || near)
        });
    }

    /// Üreme: üremeye hazır iki yetişkin yan yanaysa bir yavru doğar; uzaktaysa
    /// birbirlerine doğru çekilirler. Yaratıklar yalnızca gece ürer.
    fn breed(&mut self, night: bool, events: &mut Vec<MobEvent>) {
        let ready = |m: &Mob| m.kind.breedable() && m.adult() && m.breed_cd == 0.0 && m.health > 0 && m.state != State::Run;
        for m in self.list.iter_mut() {
            m.lure = None;
        }
        for i in 0..self.list.len() {
            if !ready(&self.list[i]) {
                continue;
            }
            let kind = self.list[i].kind;
            let hostile = kind.hostile();
            let cap = if hostile { BREED_MAX_HOSTILE } else { BREED_MAX_ANIMALS };
            if (hostile && !night) || self.count(hostile) >= cap {
                continue;
            }
            let a = self.list[i].body.pos;
            // En yakın uygun eş.
            let mate = (0..self.list.len())
                .filter(|&j| j != i && self.list[j].kind == kind && ready(&self.list[j]))
                .map(|j| (j, (self.list[j].body.pos[0] - a[0]).hypot(self.list[j].body.pos[2] - a[2])))
                .filter(|&(_, d)| d < LURE_RANGE)
                .min_by(|x, y| x.1.total_cmp(&y.1));
            let Some((j, dist)) = mate else { continue };
            let b = self.list[j].body.pos;
            if dist < BREED_RANGE {
                let cd = BREED_COOLDOWN + self.rand() * 30.0;
                self.list[i].breed_cd = cd;
                self.list[j].breed_cd = cd;
                if !hostile {
                    self.list[i].persistent = true;
                    self.list[j].persistent = true;
                }
                let pos = [(a[0] + b[0]) * 0.5, a[1].max(b[1]) + 0.1, (a[2] + b[2]) * 0.5];
                let mut baby = Mob::new_baby(kind, pos, self.rand() * std::f64::consts::TAU);
                baby.net_id = self.alloc_id();
                self.list.push(baby);
                events.push(MobEvent::Born(kind, pos));
            } else {
                self.list[i].lure = Some((-(b[0] - a[0])).atan2(-(b[2] - a[2])));
            }
        }
    }

    /// Canlı listesini `protocol::Writer` ile ikili biçimde yazar.
    pub fn encode(&self, w: &mut Writer) {
        w.var_int(self.list.len() as i32);
        for m in &self.list {
            w.u8(m.kind.to_u8());
            w.f64(m.body.pos[0]);
            w.f64(m.body.pos[1]);
            w.f64(m.body.pos[2]);
            w.f32(m.yaw as f32);
            w.var_int(m.health);
            w.f32(m.age as f32);
            w.f32(m.breed_cd as f32);
            w.bool(m.persistent);
        }
    }

    /// `protocol::Reader` üzerinden canlı listesini okur.
    pub fn decode(&mut self, r: &mut Reader) -> ProtoResult<()> {
        let len = r.var_int()?;
        if !(0..=1024).contains(&len) {
            return Err(Error::InvalidValue);
        }
        let mut list = Vec::with_capacity(len as usize);
        for _ in 0..len {
            let kind = Kind::from_u8(r.u8()?).ok_or(Error::InvalidValue)?;
            let pos = [r.f64()?, r.f64()?, r.f64()?];
            let yaw = r.f32()? as f64;
            let health = r.var_int()?;
            let age = (r.f32()? as f64).clamp(0.0, 1.0);
            let breed_cd = (r.f32()? as f64).max(0.0);
            let persistent = r.bool()?;
            if health <= 0 || !pos.iter().all(|v| v.is_finite()) || !yaw.is_finite() {
                return Err(Error::InvalidValue);
            }
            let mut m = Mob::new(kind, pos, yaw);
            m.net_id = self.alloc_id();
            m.health = health.min(kind.stats().health);
            m.age = age;
            m.breed_cd = breed_cd;
            m.persistent = persistent;
            // Köylü kayıttan dönerken evini kaybetmesin: kaydedildiği yeri
            // yeni evi sayar.
            if kind.is_villager() {
                m.home = Some(pos);
            }
            let sc = m.scale();
            m.body.radius = kind.stats().radius * sc;
            m.body.height = kind.stats().height * sc;
            list.push(m);
        }
        self.list = list;
        self.spits.clear();
        Ok(())
    }

    /// Tüm canlıları `protocol::Writer` ile bayt dizisine çevirir.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new();
        self.encode(&mut w);
        w.buf
    }

    /// `to_bytes` çıktısından canlı listesini geri yükler.
    pub fn load_bytes(&mut self, bytes: &[u8]) -> ProtoResult<()> {
        let mut r = Reader::new(bytes);
        self.decode(&mut r)?;
        r.finish()
    }

    /// Işının çarptığı en yakın canlı: `(sıra, uzaklık)`.
    pub fn ray_hit(&self, origin: [f64; 3], dir: [f64; 3], max: f64) -> Option<(usize, f64)> {
        let mut best: Option<(usize, f64)> = None;
        for (i, m) in self.list.iter().enumerate() {
            let p = m.body.pos;
            let (r, h) = (m.body.radius, m.body.height);
            let (min, max_c) = ([p[0] - r, p[1], p[2] - r], [p[0] + r, p[1] + h, p[2] + r]);
            let (mut t0, mut t1) = (0.0f64, max);
            let mut hit = true;
            for a in 0..3 {
                if dir[a].abs() < 1e-9 {
                    if origin[a] < min[a] || origin[a] > max_c[a] {
                        hit = false;
                        break;
                    }
                } else {
                    let (mut a0, mut a1) = ((min[a] - origin[a]) / dir[a], (max_c[a] - origin[a]) / dir[a]);
                    if a0 > a1 {
                        std::mem::swap(&mut a0, &mut a1);
                    }
                    t0 = t0.max(a0);
                    t1 = t1.min(a1);
                    if t0 > t1 {
                        hit = false;
                        break;
                    }
                }
            }
            if hit && best.is_none_or(|(_, t)| t0 < t) {
                best = Some((i, t0));
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::STONE;

    /// Zemin yüzeyi `y = 21` olan düz, çimenli dünya.
    fn flat() -> World {
        let mut w = World::new(20261012);
        for dz in -3..=3 {
            for dx in -3..=3 {
                w.generate_chunk(dx, dz);
            }
        }
        // Üretilen bütün chunk'ları (16×3 → -48..47) düzleştir: kenarda kalan
        // doğal okyanus balık doğurma testlerini bozmasın.
        for x in -48..48 {
            for z in -48..48 {
                for y in 0..20 {
                    w.set_block_raw(x, y, z, STONE);
                }
                w.set_block_raw(x, 20, z, GRASS);
                for y in 21..50 {
                    w.set_block_raw(x, y, z, AIR);
                }
            }
        }
        w
    }

    const FAR: [f64; 3] = [500.0, 21.0, 500.0];
    /// Zombinin fark edemeyeceği kadar uzak ama canlıların silinmeyeceği kadar yakın.
    const AWAY: [f64; 3] = [40.5, 21.0, 40.5];

    fn run(m: &mut Mob, w: &World, seconds: f64, player: [f64; 3]) -> u32 {
        let mut s = 12345u32;
        let mut rand = move || {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            s as f64 / 4_294_967_296.0
        };
        let mut attacks = 0;
        for _ in 0..(seconds / 0.02) as usize {
            attacks += m.update(w, 0.02, player, &mut rand) as u32;
        }
        attacks
    }

    #[test]
    fn animals_alternate_between_idle_and_walking() {
        let w = flat();
        let mut m = Mob::new(Kind::Sheep, [0.5, 21.0, 0.5], 0.0);
        let (mut idle, mut walk, mut fastest) = (0, 0, 0.0f64);
        let mut s = 7u32;
        let mut rand = move || {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            s as f64 / 4_294_967_296.0
        };
        for _ in 0..3000 {
            m.update(&w, 0.02, FAR, &mut rand);
            match m.state {
                State::Idle => idle += 1,
                State::Walk => walk += 1,
                State::Run => panic!("tehdit yokken koşmamalı"),
            }
            fastest = fastest.max(m.speed);
        }
        assert!(idle > 300 && walk > 300, "hem durmalı hem yürümeli: {idle}/{walk}");
        assert!(fastest > 1.0 && fastest < 2.0, "yürüyüş hızı: {fastest}");
        assert!((m.body.pos[1] - 21.0).abs() < 0.1, "zeminde kalmalı");
        assert!(m.anim > 5.0, "yürürken bacak animasyonu ilerlemeli");
    }

    #[test]
    fn hurt_animals_run_away_then_calm_down() {
        let w = flat();
        let mut m = Mob::new(Kind::Pig, [0.5, 21.0, 0.5], 0.0);
        let attacker = [3.5, 21.0, 0.5];
        assert!(!m.damage(2, attacker));
        assert_eq!(m.health, 8);
        run(&mut m, &w, 2.0, attacker);
        assert_eq!(m.state, State::Run);
        assert!(m.speed > 3.5, "koşma hızı yürüyüşten yüksek olmalı: {}", m.speed);
        assert!(m.body.pos[0] < -4.0, "saldırgandan uzaklaşmalı: x={}", m.body.pos[0]);
        run(&mut m, &w, 5.0, attacker);
        assert_ne!(m.state, State::Run, "bir süre sonra sakinleşmeli");
    }

    #[test]
    fn zombie_chases_and_attacks_only_when_the_player_is_near() {
        let w = flat();
        let mut z = Mob::new(Kind::Zombie, [0.5, 21.0, 0.5], 0.0);
        assert_eq!(run(&mut z, &w, 4.0, FAR), 0);
        assert_ne!(z.state, State::Run, "oyuncu uzaktayken kovalamamalı");

        let player = [10.5, 21.0, 0.5];
        let attacks = run(&mut z, &w, 6.0, player);
        assert_eq!(z.state, State::Run);
        assert!((z.body.pos[0] - 10.5).abs() < 1.5, "oyuncuya ulaşmalı: x={}", z.body.pos[0]);
        assert!((1..=4).contains(&attacks), "aralıklı vurmalı: {attacks}");
    }

    #[test]
    fn mobs_jump_single_steps() {
        let mut w = flat();
        for z in -40..40 {
            w.set_block_raw(6, 21, z, STONE);
            w.set_block_raw(7, 21, z, STONE);
        }
        let mut z = Mob::new(Kind::Zombie, [0.5, 21.0, 0.5], 0.0);
        run(&mut z, &w, 6.0, [14.5, 21.0, 0.5]);
        assert!(z.body.pos[0] > 9.0, "basamağı aşamadı: x={}", z.body.pos[0]);
    }

    #[test]
    fn dead_mobs_are_removed_and_reported() {
        let w = flat();
        let mut mobs = Mobs::default();
        mobs.spawn(Kind::Chicken, [0.5, 21.0, 0.5]);
        assert!(mobs.list[0].damage(10, [2.0, 21.0, 0.5]));
        let mut ev = Vec::new();
        mobs.tick(&w, 0.02, AWAY, false, &[], &mut ev);
        assert!(mobs.list.is_empty());
        assert!(ev.iter().any(|e| matches!(e, MobEvent::Died(Kind::Chicken, _, _))));
        assert_eq!(Kind::Chicken.stats().drop, Some((MEAT, 1)));
        assert_eq!(Kind::Zombie.stats().drop, Some((APPLE, 1)));
        assert_eq!(Kind::Shroom.stats().drop, Some((MUSHROOM, 1)));
        assert_ne!(Kind::Shroom.stats().color, Kind::Sheep.stats().color, "Mantar Kafa kendi renginde parçacık saçmalı");
    }

    #[test]
    fn ray_hits_the_nearest_mob() {
        let mut mobs = Mobs::default();
        mobs.spawn(Kind::Sheep, [8.5, 21.0, 0.5]);
        mobs.spawn(Kind::Pig, [4.5, 21.0, 0.5]);
        let (i, t) = mobs.ray_hit([0.5, 21.5, 0.5], [1.0, 0.0, 0.0], 50.0).expect("ıskaladı");
        assert_eq!(mobs.list[i].kind, Kind::Pig);
        assert!((t - 3.58).abs() < 0.01, "t={t}");
        assert!(mobs.ray_hit([0.5, 21.5, 0.5], [0.0, 1.0, 0.0], 50.0).is_none());
        assert!(mobs.ray_hit([0.5, 21.5, 0.5], [1.0, 0.0, 0.0], 2.0).is_none(), "menzil dışı");
    }

    #[test]
    fn spawning_respects_caps_and_zombies_burn_by_day() {
        let w = flat();
        let mut mobs = Mobs::default();
        let mut ev = Vec::new();
        let player = [0.5, 21.0, 0.5];
        for _ in 0..6000 {
            mobs.tick(&w, 0.05, player, true, &[], &mut ev);
        }
        assert!(mobs.count(false) > 0 && mobs.count(false) <= BREED_MAX_ANIMALS, "hayvan: {}", mobs.count(false));
        assert!(mobs.count(true) > 0 && mobs.count(true) <= BREED_MAX_HOSTILE, "yaratık: {}", mobs.count(true));
        for m in &mobs.list {
            assert!(m.body.pos[1] > 20.5, "zeminin altında doğmuş: {:?}", m.body.pos);
        }
        for _ in 0..1200 {
            mobs.tick(&w, 0.05, AWAY, false, &[], &mut ev);
        }
        let non_spiders = mobs.list.iter().filter(|m| m.kind.hostile() && m.kind != Kind::Spider).count();
        assert_eq!(non_spiders, 0, "güneşte yanan gece yaratıkları gündüz yok olmalı");
    }

    #[test]
    fn torches_prevent_hostile_spawning_nearby() {
        let w = flat();
        let mut mobs = Mobs::default();
        let player = [0.5, 21.0, 0.5];
        // Oyuncunun etrafındaki doğma halkasının pozitif X yarısını meşalelerle aydınlat.
        let mut torches = Vec::new();
        for x in 10..=40 {
            for z in (-40..=40).step_by(6) {
                torches.push([x, 21, z]);
            }
        }
        let mut spawned_hostiles = 0;
        for _ in 0..500 {
            let prev_len = mobs.list.len();
            mobs.try_spawn(&w, player, true, &torches);
            if mobs.list.len() > prev_len {
                let m = mobs.list.pop().unwrap();
                if m.kind.hostile() {
                    spawned_hostiles += 1;
                    let p = m.body.pos;
                    for t in &torches {
                        let d = ((t[0] as f64 + 0.5 - p[0]).powi(2)
                            + (t[1] as f64 + 0.5 - p[1]).powi(2)
                            + (t[2] as f64 + 0.5 - p[2]).powi(2))
                        .sqrt();
                        assert!(d >= TORCH_SAFE_RADIUS, "meşaleye fazla yakın doğmuş: d={d}");
                    }
                }
            }
        }
        assert!(spawned_hostiles > 0, "karanlık tarafta yaratık doğabilmeli");
    }

    #[test]
    fn burning_uses_its_own_timer_without_speeding_up_idle_animation() {
        let w = flat();
        let mut mobs = Mobs::default();
        mobs.spawn(Kind::Zombie, [0.5, 21.0, 0.5]);
        mobs.spawn_timer = 1e9;
        let mut ev = Vec::new();
        for _ in 0..20 {
            mobs.tick(&w, 0.05, AWAY, false, &[], &mut ev);
        }
        assert!((mobs.list[0].idle_time - 1.0).abs() < 1e-6, "idle_time iki kat hızlı ilerlememeli: {}", mobs.list[0].idle_time);
        assert!(mobs.list[0].health < Kind::Zombie.stats().health, "gündüz yanarak hasar almalı");
    }

    #[test]
    fn two_adults_breed_and_the_baby_grows_up() {
        let w = flat();
        let mut mobs = Mobs::default();
        mobs.spawn(Kind::Sheep, [0.5, 21.0, 0.5]);
        mobs.spawn(Kind::Sheep, [1.9, 21.0, 0.5]);
        for m in mobs.list.iter_mut() {
            m.breed_cd = 0.0;
            m.state = State::Idle;
            m.timer = 100.0; // yerlerinde dursunlar
        }
        let mut ev = Vec::new();
        mobs.spawn_timer = 1e9;
        for _ in 0..80 {
            mobs.tick(&w, 0.05, AWAY, false, &[], &mut ev);
        }
        assert_eq!(mobs.list.len(), 3, "bir yavru doğmalı (ve bekleme süresi ikincisini engellemeli)");
        assert_eq!(ev.iter().filter(|e| matches!(e, MobEvent::Born(Kind::Sheep, _))).count(), 1);
        assert!(mobs.list.iter().all(|m| m.persistent), "üreyen ebeveynler ve yavru kalıcı olmalı");
        let baby = &mobs.list[2];
        assert!(!baby.adult() && baby.scale() < 0.6, "yavru küçük doğmalı");
        assert_eq!(baby.health, Kind::Sheep.stats().health / 2, "yavru yarı canla doğmalı");
        assert!(baby.body.height < Kind::Sheep.stats().height * 0.6);
        for _ in 0..(GROW_TIME / 0.05) as usize + 10 {
            mobs.tick(&w, 0.05, AWAY, false, &[], &mut ev);
        }
        assert!(mobs.list[2].adult() && mobs.list[2].scale() == 1.0, "yavru büyümeli");
        assert_eq!(mobs.list[2].health, Kind::Sheep.stats().health, "büyüyünce tam cana ulaşmalı");
    }

    #[test]
    fn persistent_animals_survive_despawn_and_round_trip_protocol_save() {
        let w = flat();
        let mut mobs = Mobs::default();
        mobs.spawn(Kind::Sheep, [0.5, 21.0, 0.5]);
        mobs.spawn(Kind::Pig, [2.5, 21.0, 0.5]);
        mobs.list[0].persistent = true;
        mobs.list[0].health = 6;
        mobs.list[0].age = 0.5;
        mobs.spawn_timer = 1e9;
        let mut ev = Vec::new();
        // Oyuncu 500 blok uzaktayken (`FAR`) kalıcı olmayan domuz silinir, kalıcı koyun kalır.
        mobs.tick(&w, 0.05, FAR, false, &[], &mut ev);
        assert_eq!(mobs.list.len(), 1);
        assert_eq!(mobs.list[0].kind, Kind::Sheep);

        let bytes = mobs.to_bytes();
        let mut restored = Mobs::default();
        restored.load_bytes(&bytes).expect("kayıt çözülebilmeli");
        assert_eq!(restored.list.len(), 1);
        let s = &restored.list[0];
        assert_eq!(s.kind, Kind::Sheep);
        assert_eq!(s.health, 6);
        assert!(s.persistent);
        assert!((s.age - 0.5).abs() < 0.05);
    }

    #[test]
    fn distant_mates_walk_towards_each_other() {
        let w = flat();
        let mut mobs = Mobs::default();
        mobs.spawn(Kind::Pig, [-5.5, 21.0, 0.5]);
        mobs.spawn(Kind::Pig, [5.5, 21.0, 0.5]);
        for m in mobs.list.iter_mut() {
            m.breed_cd = 0.0;
        }
        mobs.spawn_timer = 1e9;
        let mut ev = Vec::new();
        for _ in 0..1600 {
            mobs.tick(&w, 0.05, AWAY, false, &[], &mut ev);
            if mobs.list.len() > 2 {
                break;
            }
        }
        assert!(mobs.list.len() > 2, "uzaktaki eşler buluşup yavrulamalı");
    }

    #[test]
    fn creatures_only_breed_at_night() {
        let w = flat();
        let mut mobs = Mobs::default();
        mobs.spawn(Kind::Shroom, [0.5, 21.0, 0.5]);
        mobs.spawn(Kind::Shroom, [1.9, 21.0, 0.5]);
        for m in mobs.list.iter_mut() {
            m.breed_cd = 0.0;
            m.timer = 100.0;
        }
        mobs.spawn_timer = 1e9;
        let mut ev = Vec::new();
        for _ in 0..60 {
            mobs.tick(&w, 0.05, AWAY, false, &[], &mut ev);
        }
        assert_eq!(mobs.list.len(), 2, "gündüz üremezler");
        for _ in 0..60 {
            mobs.tick(&w, 0.05, AWAY, true, &[], &mut ev);
        }
        assert_eq!(mobs.list.len(), 3, "gece ürerler");
    }

    #[test]
    fn mushroom_head_keeps_its_distance_and_spits_poison() {
        let w = flat();
        let mut mobs = Mobs::default();
        mobs.spawn(Kind::Shroom, [0.5, 21.0, 0.5]);
        mobs.spawn_timer = 1e9;
        let player = [6.5, 21.0, 0.5];
        let mut ev = Vec::new();
        for _ in 0..200 {
            mobs.tick(&w, 0.05, player, true, &[], &mut ev);
        }
        let spat = ev.iter().filter(|e| matches!(e, MobEvent::Sound(Kind::Shroom, Sound::Spit, _))).count();
        let poisoned = ev.iter().filter(|e| matches!(e, MobEvent::Poison(_))).count();
        assert!((3..=5).contains(&spat), "10 saniyede aralıklı tükürmeli: {spat}");
        assert!(poisoned >= 2, "duran oyuncuya isabet etmeli: {poisoned}");
        assert!(mobs.spits.len() <= 1);

        // Oyuncu dibine gelince koşarak uzaklaşır.
        let m = &mobs.list[0];
        let near = [m.body.pos[0] + 1.5, 21.0, m.body.pos[2]];
        let x0 = m.body.pos[0];
        for _ in 0..40 {
            mobs.tick(&w, 0.05, near, true, &[], &mut ev);
        }
        assert!(mobs.list[0].body.pos[0] < x0 - 1.5, "yakındaki oyuncudan uzaklaşmalı");
    }

    #[test]
    fn spit_stops_at_walls() {
        let mut w = flat();
        for y in 21..26 {
            for z in -3..4 {
                w.set_block_raw(3, y, z, STONE);
            }
        }
        let mut mobs = Mobs::default();
        mobs.spawn(Kind::Shroom, [0.5, 21.0, 0.5]);
        mobs.spawn_timer = 1e9;
        let mut ev = Vec::new();
        for _ in 0..200 {
            mobs.tick(&w, 0.05, [6.5, 21.0, 0.5], true, &[], &mut ev);
        }
        assert!(ev.iter().any(|e| matches!(e, MobEvent::SpitHit(_))), "tükürük duvara çarpmalı");
        assert!(!ev.iter().any(|e| matches!(e, MobEvent::Poison(_))), "duvarın arkasındaki oyuncu zehirlenmemeli");
    }

    #[test]
    fn mobs_make_idle_hurt_and_alert_sounds() {
        let w = flat();
        let mut mobs = Mobs::default();
        mobs.spawn(Kind::Sheep, [0.5, 21.0, 0.5]);
        mobs.spawn(Kind::Zombie, [20.5, 21.0, 20.5]);
        mobs.spawn_timer = 1e9;
        let mut ev = Vec::new();
        for _ in 0..400 {
            mobs.tick(&w, 0.05, AWAY, true, &[], &mut ev);
        }
        let count = |ev: &Vec<MobEvent>, k: Kind, s: Sound| ev.iter().filter(|e| matches!(e, MobEvent::Sound(a, b, _) if *a == k && *b == s)).count();
        assert!((1..=4).contains(&count(&ev, Kind::Sheep, Sound::Idle)), "20 saniyede ara sıra melemeli");
        assert_eq!(count(&ev, Kind::Zombie, Sound::Alert), 0);

        ev.clear();
        mobs.list[0].damage(1, [3.0, 21.0, 0.5]);
        let zp = mobs.list[1].body.pos;
        mobs.tick(&w, 0.05, [zp[0] + 6.0, 21.0, zp[2]], true, &[], &mut ev);
        assert_eq!(count(&ev, Kind::Sheep, Sound::Hurt), 1);
        assert_eq!(count(&ev, Kind::Zombie, Sound::Alert), 1, "oyuncuyu fark edince bir kez hırlamalı");
        mobs.tick(&w, 0.05, [zp[0] + 6.0, 21.0, zp[2]], true, &[], &mut ev);
        assert_eq!(count(&ev, Kind::Zombie, Sound::Alert), 1, "kovalarken tekrar tekrar değil");
    }

    #[test]
    fn mob_aabb_encloses_all_parts_and_frustum_culls_behind_camera() {
        let cam = [0.0, 21.0, 0.0];
        for kind in [
            Kind::Sheep,
            Kind::Pig,
            Kind::Chicken,
            Kind::Cow,
            Kind::Zombie,
            Kind::Shroom,
            Kind::Spider,
            Kind::Skeleton,
            Kind::Fish,
            Kind::Villager,
        ] {
            let mut m = Mob::new(kind, [0.0, 21.0, -10.0], 0.7);
            m.state = State::Run;
            m.speed = 3.0;
            m.anim = 1.2;
            let (min, max) = m.aabb(cam);
            let mut boxes = Vec::new();
            m.push_boxes(&mut boxes);
            assert!(!boxes.is_empty());
            for b in &boxes {
                let rel = [
                    (b.center[0] - cam[0]) as f32,
                    (b.center[1] - cam[1]) as f32,
                    (b.center[2] - cam[2]) as f32,
                ];
                for a in 0..3 {
                    assert!(
                        rel[a] >= min[a] && rel[a] <= max[a],
                        "{:?} parçası AABB dışında: eksen {a}, rel={}, aralık=[{}, {}]",
                        kind,
                        rel[a],
                        min[a],
                        max[a]
                    );
                }
            }
        }

        let vp = crate::mat4::mul(
            &crate::mat4::perspective(76.0_f32.to_radians(), 16.0 / 9.0, 0.05, 192.0),
            &crate::mat4::view_rotation(0.0, 0.0),
        );
        let frustum = crate::mat4::Frustum::from_view_proj(&vp);
        let front = Mob::new(Kind::Sheep, [0.0, 21.0, -12.0], 0.0);
        let behind = Mob::new(Kind::Sheep, [0.0, 21.0, 12.0], 0.0);
        let (fmin, fmax) = front.aabb(cam);
        let (bmin, bmax) = behind.aabb(cam);
        assert!(frustum.aabb_visible(fmin, fmax), "öndeki canlı görünür olmalı");
        assert!(!frustum.aabb_visible(bmin, bmax), "arkadaki canlı görüş konisiyle elenmeli");
    }

    #[test]
    fn spider_is_neutral_by_day_attacks_when_provoked_and_climbs_walls() {
        let mut w = flat();
        // 3 blok yüksekliğinde duvar.
        for z in -10..10 {
            for y in 21..=23 {
                w.set_block_raw(5, y, z, STONE);
            }
        }
        let mut sp = Mob::new(Kind::Spider, [0.5, 21.0, 0.5], 0.0);
        sp.night = false;
        let player = [8.5, 21.0, 0.5];
        assert_eq!(run(&mut sp, &w, 2.0, player), 0, "gündüz kışkırtılmadan saldırmamalı");
        assert_ne!(sp.state, State::Run);

        // Hasar alınca gündüz de kışkırtılır ve 3 blokluk duvarı tırmanarak aşar.
        sp.damage(1, player);
        let attacks = run(&mut sp, &w, 6.0, player);
        assert!(sp.body.pos[0] > 6.0, "örümcek 3 blokluk duvarı tırmanıp aşmalı: x={}", sp.body.pos[0]);
        assert!(attacks >= 1, "duvarı aşıp oyuncuya vurmalı");
    }

    #[test]
    fn skeleton_shoots_arrows_that_deal_direct_damage() {
        let w = flat();
        let mut mobs = Mobs::default();
        mobs.spawn(Kind::Skeleton, [0.5, 21.0, 0.5]);
        mobs.spawn_timer = 1e9;
        let player = [7.5, 21.0, 0.5];
        let mut ev = Vec::new();
        for _ in 0..160 {
            mobs.tick(&w, 0.05, player, true, &[], &mut ev);
        }
        let shots = ev.iter().filter(|e| matches!(e, MobEvent::Sound(Kind::Skeleton, Sound::Shoot, _))).count();
        let hits = ev.iter().filter(|e| matches!(e, MobEvent::ArrowHit(0, ARROW_DAMAGE))).count();
        assert!(shots >= 3, "iskelet aralıklı ok atmalı: {shots}");
        assert!(hits >= 2, "ok oyuncuya isabet etmeli: {hits}");
        assert_eq!(Kind::Skeleton.stats().drop, Some((BONE, 2)));
    }

    /// Ortası su dolu düz dünya: 41×41 göl, iki blok derinliğinde.
    fn lake() -> World {
        let mut w = flat();
        for x in -20..=20 {
            for z in -20..=20 {
                w.set_block_raw(x, 19, z, WATER);
                w.set_block_raw(x, 20, z, WATER);
            }
        }
        w
    }

    fn fish_rand(seed: u32) -> impl FnMut() -> f64 {
        let mut s = seed;
        move || {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            s as f64 / 4_294_967_296.0
        }
    }

    #[test]
    fn fish_swim_inside_water_and_never_beach_themselves() {
        let w = lake();
        let mut m = Mob::new(Kind::Fish, [0.5, 19.4, 0.5], 0.0);
        let mut rand = fish_rand(4242);
        let mut travelled = 0.0;
        let mut last = m.body.pos;
        for _ in 0..800 {
            m.update(&w, 0.02, [40.5, 21.0, 40.5], &mut rand);
            travelled += (m.body.pos[0] - last[0]).hypot(m.body.pos[2] - last[2]);
            last = m.body.pos;
            let b = w.get_block(m.body.pos[0].floor() as i32, m.body.pos[1].floor() as i32, m.body.pos[2].floor() as i32);
            assert_eq!(b, WATER, "balık sudan çıktı: {:?}", m.body.pos);
            assert!((18.9..21.0).contains(&m.body.pos[1]), "dikey aralık: {}", m.body.pos[1]);
        }
        assert_eq!(m.health, Kind::Fish.stats().health, "suda can kaybetmemeli");
        assert!(travelled > 1.5, "balık yüzmeli: {travelled}");
    }

    #[test]
    fn fish_flee_from_the_player_underwater() {
        let w = lake();
        let mut m = Mob::new(Kind::Fish, [0.5, 19.5, 0.5], 0.0);
        let mut rand = fish_rand(7);
        // Oyuncu dibinde: balık koşarak uzaklaşır.
        for _ in 0..40 {
            m.update(&w, 0.02, [1.2, 21.0, 0.5], &mut rand);
        }
        assert_eq!(m.state, State::Run);
        assert!(m.body.pos[0] < -1.0, "oyuncudan uzaklaşmalı: x={}", m.body.pos[0]);
    }

    #[test]
    fn a_beached_fish_flops_and_suffocates() {
        let w = flat();
        let mut m = Mob::new(Kind::Fish, [0.5, 21.0, 0.5], 0.0);
        let mut rand = fish_rand(11);
        for _ in 0..400 {
            m.update(&w, 0.02, [40.5, 21.0, 40.5], &mut rand);
        }
        assert!(m.health < Kind::Fish.stats().health, "karada boğulmalı: {}", m.health);
        assert!(m.body.pos[1] > 20.0, "zeminde çırpınmalı");
    }

    #[test]
    fn villagers_keep_to_their_home_and_return_at_night() {
        let w = flat();
        let home = [0.5, 21.0, 0.5];
        let mut m = Mob::new(Kind::Villager, [15.5, 21.0, 0.5], 0.0);
        m.home = Some(home);
        m.night = false;
        let mut rand = fish_rand(21);
        // Gündüz: evin çevresinde kalır (11 blok + bir adım payı).
        for _ in 0..600 {
            m.update(&w, 0.02, [40.5, 21.0, 40.5], &mut rand);
        }
        let d = (m.body.pos[0] - home[0]).hypot(m.body.pos[2] - home[2]);
        assert!(d <= 13.0, "gündüz evinden uzaklaştı: {d}");

        // Gece: kapısına döner.
        m.night = true;
        m.body.pos = [15.5, 21.0, 0.5];
        for _ in 0..900 {
            m.update(&w, 0.02, [40.5, 21.0, 40.5], &mut rand);
        }
        let d = (m.body.pos[0] - home[0]).hypot(m.body.pos[2] - home[2]);
        assert!(d < 2.5, "gece eve dönmedi: {d}");
    }

    #[test]
    fn fish_spawn_only_in_water_and_villagers_only_at_village_doors() {
        let mut mobs = Mobs::default();
        let dry = flat();
        for _ in 0..120 {
            mobs.try_spawn_fish(&dry, [0.5, 21.0, 0.5]);
        }
        assert_eq!(mobs.count_fish(), 0, "karada balık doğmamalı");

        let wet = lake();
        for _ in 0..200 {
            mobs.try_spawn_fish(&wet, [0.5, 21.0, 0.5]);
        }
        assert!(mobs.count_fish() > 0, "gölde balık doğmalı");
        assert!(mobs.count_fish() <= MAX_FISH);
        for m in mobs.list.iter().filter(|m| m.kind.is_fish()) {
            assert!((19.0..21.0).contains(&m.body.pos[1]), "su dışında doğdu: {}", m.body.pos[1]);
        }

        // Gerçek dünyada bir köy bulup kapısında köylü doğur.
        mobs.list.clear();
        let mut w = World::new(20261012);
        let v = (0..400)
            .find_map(|i| w.village_at(i % 20 - 10, i / 20 - 20))
            .expect("köy bulunamadı");
        for cz in ((v.cz - 30) >> 4)..=((v.cz + 30) >> 4) {
            for cx in ((v.cx - 30) >> 4)..=((v.cx + 30) >> 4) {
                w.generate_chunk(cx, cz);
            }
        }
        let p = [v.cx as f64, v.base as f64, v.cz as f64];
        for _ in 0..60 {
            mobs.try_spawn_villager(&w, p);
        }
        assert!(mobs.count_villagers() > 0, "köyde köylü doğmalı");
        assert!(mobs.count_villagers() <= MAX_VILLAGERS);
        assert!(
            mobs.list.iter().filter(|m| m.kind.is_villager()).all(|m| m.persistent && m.home.is_some()),
            "köylüler kalıcı olmalı ve evlerini bilmeli"
        );
        assert_eq!(mobs.count(false), 0, "köylü yabani hayvan sayılmamalı");
    }

    #[test]
    fn feeding_animals_heals_lures_and_readies_breeding() {
        let mut mobs = Mobs::default();
        mobs.spawn(Kind::Cow, [0.5, 21.0, 0.5]);
        mobs.list[0].health = 6;
        mobs.list[0].breed_cd = 60.0;
        assert!(!mobs.feed(0, BONE), "inek kemik yemez");
        assert!(mobs.feed(0, WHEAT), "inek buğdayla beslenir");
        assert_eq!(mobs.list[0].health, 8);
        assert_eq!(mobs.list[0].breed_cd, 0.0);
        assert!(mobs.list[0].persistent);

        mobs.list[0].state = State::Idle;
        mobs.lure_with_item([6.5, 21.0, 0.5], Some(WHEAT));
        assert_eq!(mobs.list[0].state, State::Walk, "buğday tutan oyuncuya doğru yürümeli");
        assert!(mobs.list[0].lure.is_some());
    }

    #[test]
    fn benchmark_before_vs_after_effects_and_instancing() {
        use crate::effects::{pack_box_instances, BoxInst};
        use crate::mat4::{mul, perspective, view_rotation, Frustum};
        use crate::mesh::FACES;
        use crate::vehicle::Car;
        use std::time::Instant;

        // Eski CPU köşe açılımı (kutu başına 36 köşe × 16 bayt = 576 bayt + sin/cos).
        fn legacy_cpu_expand(boxes: &[BoxInst], cam_pos: [f64; 3], bytes: &mut Vec<u8>) {
            bytes.clear();
            for b in boxes {
                let (sin, cos) = b.yaw.sin_cos();
                let (ps, pc) = b.pitch.sin_cos();
                let rel = [
                    (b.center[0] - cam_pos[0]) as f32,
                    (b.center[1] - cam_pos[1]) as f32,
                    (b.center[2] - cam_pos[2]) as f32,
                ];
                for face in FACES.iter() {
                    let col = [
                        (b.color[0] as f32 * face.shade) as u8,
                        (b.color[1] as f32 * face.shade) as u8,
                        (b.color[2] as f32 * face.shade) as u8,
                        if b.glow { 255 } else { 0 },
                    ];
                    for corner in [0usize, 1, 2, 0, 2, 3] {
                        let v = face.verts[corner];
                        let lx = (v[0] as f32 * 2.0 - 1.0) * b.half[0];
                        let y0 = (v[1] as f32 * 2.0 - 1.0) * b.half[1];
                        let z0 = (v[2] as f32 * 2.0 - 1.0) * b.half[2];
                        let ly = y0 * pc - z0 * ps;
                        let lz = y0 * ps + z0 * pc;
                        let p = [rel[0] + lx * cos + lz * sin, rel[1] + ly, rel[2] - lx * sin + lz * cos];
                        for c in p {
                            bytes.extend_from_slice(&c.to_le_bytes());
                        }
                        bytes.extend_from_slice(&col);
                    }
                }
            }
        }

        let cam = [0.0, 22.62, 0.0];
        let vp = mul(&perspective(76.0_f32.to_radians(), 16.0 / 9.0, 0.05, 192.0), &view_rotation(0.4, -0.1));
        let frustum = Frustum::from_view_proj(&vp);

        // 28 meşale, 6 araba, 50 canlı (oyuncunun etrafındaki çemberde).
        let torches: Vec<[i32; 3]> = (0..28)
            .map(|i| {
                let a = i as f64 * std::f64::consts::TAU / 28.0;
                [(a.cos() * 18.0) as i32, 21, (a.sin() * 18.0) as i32]
            })
            .collect();
        let cars: Vec<Car> = (0..6)
            .map(|i| {
                let a = i as f64 * std::f64::consts::TAU / 6.0;
                Car::new([a.cos() * 22.0, 21.0, a.sin() * 22.0], a)
            })
            .collect();
        let kinds = [Kind::Sheep, Kind::Pig, Kind::Chicken, Kind::Zombie, Kind::Shroom];
        let mobs: Vec<Mob> = (0..50)
            .map(|i| {
                let a = i as f64 * std::f64::consts::TAU / 50.0;
                let mut m = Mob::new(kinds[i % 5], [a.cos() * 28.0, 21.0, a.sin() * 28.0], a);
                m.state = State::Walk;
                m.speed = 1.6;
                m.anim = i as f64 * 0.3;
                m
            })
            .collect();

        let frames = 2000;
        let mut boxes = Vec::with_capacity(600);
        let mut bytes = Vec::with_capacity(400_000);

        // --- ÖNCE (görüş konisi elemesi yok + CPU 36 köşe açılımı) ---
        let t0 = Instant::now();
        let mut before_boxes = 0;
        let mut before_bytes = 0;
        for f in 0..frames {
            let time = f as f32 * 0.016;
            boxes.clear();
            for t in &torches {
                let (x, y, z) = (t[0] as f64 + 0.5, t[1] as f64, t[2] as f64 + 0.5);
                let flicker = ((time * 9.0 + (t[0] * 7 + t[2] * 13) as f32).sin() * 0.5 + 0.5) * 0.25;
                boxes.push(BoxInst { center: [x, y + 0.28, z], half: [0.06, 0.28, 0.06], yaw: 0.0, pitch: 0.0, color: [128, 92, 52], glow: false });
                let s = 0.085 + flicker * 0.05;
                let fx = x + (time * 3.1 + t[0] as f32).sin() as f64 * 0.018;
                boxes.push(BoxInst { center: [fx, y + 0.62 + s as f64 * 0.3, z], half: [s, s, s], yaw: 0.7, pitch: 0.0, color: [255, 190, 70], glow: true });
                boxes.push(BoxInst { center: [fx, y + 0.62, z], half: [0.05, 0.07, 0.05], yaw: 0.0, pitch: 0.0, color: [255, 245, 190], glow: true });
            }
            for car in &cars {
                let p = car.body.pos;
                let yaw = car.yaw as f32;
                let f = car.forward();
                let right = [-f[1], f[0]];
                let at = |r: f64, u: f64, fw: f64| [p[0] + right[0] * r + f[0] * fw, p[1] + u, p[2] + right[1] * r + f[1] * fw];
                boxes.push(BoxInst { center: at(0.0, 0.52, 0.0), half: [0.62, 0.24, 1.0], yaw, pitch: 0.0, color: [200, 56, 44], glow: false });
                boxes.push(BoxInst { center: at(0.0, 0.96, -0.12), half: [0.54, 0.2, 0.5], yaw, pitch: 0.0, color: [176, 220, 240], glow: false });
                boxes.push(BoxInst { center: at(0.0, 1.18, -0.12), half: [0.56, 0.03, 0.52], yaw, pitch: 0.0, color: [150, 40, 32], glow: false });
                for side in [-1.0, 1.0] {
                    boxes.push(BoxInst { center: at(0.4 * side, 0.56, 1.0), half: [0.12, 0.08, 0.02], yaw, pitch: 0.0, color: [255, 244, 190], glow: true });
                    for end in [-0.62, 0.62] {
                        boxes.push(BoxInst { center: at(0.6 * side, 0.26, end), half: [0.1, 0.25, 0.25], yaw, pitch: 0.0, color: [28, 28, 30], glow: false });
                        boxes.push(BoxInst { center: at(0.66 * side, 0.26, end), half: [0.05, 0.2, 0.05], yaw, pitch: 0.0, color: [190, 190, 196], glow: false });
                    }
                }
            }
            for m in &mobs {
                m.push_boxes(&mut boxes);
            }
            legacy_cpu_expand(&boxes, cam, &mut bytes);
            before_boxes = boxes.len();
            before_bytes = bytes.len();
            std::hint::black_box(&bytes);
        }
        let before_ms = t0.elapsed().as_secs_f64() * 1000.0 / frames as f64;

        // --- SONRA (Frustum::aabb_visible elemesi + GPU Instancing 36 bayt/kutu) ---
        let t1 = Instant::now();
        let mut after_boxes = 0;
        let mut after_bytes = 0;
        for f in 0..frames {
            let time = f as f32 * 0.016;
            boxes.clear();
            for t in &torches {
                let (x, y, z) = (t[0] as f64 + 0.5, t[1] as f64, t[2] as f64 + 0.5);
                let (rx, ry, rz) = ((x - cam[0]) as f32, (y - cam[1]) as f32, (z - cam[2]) as f32);
                if !frustum.aabb_visible([rx - 0.2, ry, rz - 0.2], [rx + 0.2, ry + 0.8, rz + 0.2]) {
                    continue;
                }
                let flicker = ((time * 9.0 + (t[0] * 7 + t[2] * 13) as f32).sin() * 0.5 + 0.5) * 0.25;
                let fx = x + (time * 3.1 + t[0] as f32).sin() as f64 * 0.018;
                boxes.push(BoxInst { center: [x, y + 0.28, z], half: [0.06, 0.28, 0.06], yaw: 0.0, pitch: 0.0, color: [128, 92, 52], glow: false });
                let s = 0.085 + flicker * 0.05;
                boxes.push(BoxInst { center: [fx, y + 0.62 + s as f64 * 0.3, z], half: [s, s, s], yaw: 0.7, pitch: 0.0, color: [255, 190, 70], glow: true });
                boxes.push(BoxInst { center: [fx, y + 0.62, z], half: [0.05, 0.07, 0.05], yaw: 0.0, pitch: 0.0, color: [255, 245, 190], glow: true });
            }
            for car in &cars {
                let p = car.body.pos;
                let (rx, ry, rz) = ((p[0] - cam[0]) as f32, (p[1] - cam[1]) as f32, (p[2] - cam[2]) as f32);
                if !frustum.aabb_visible([rx - 1.3, ry - 0.1, rz - 1.3], [rx + 1.3, ry + 1.4, rz + 1.3]) {
                    continue;
                }
                let yaw = car.yaw as f32;
                let f = car.forward();
                let right = [-f[1], f[0]];
                let at = |r: f64, u: f64, fw: f64| [p[0] + right[0] * r + f[0] * fw, p[1] + u, p[2] + right[1] * r + f[1] * fw];
                boxes.push(BoxInst { center: at(0.0, 0.52, 0.0), half: [0.62, 0.24, 1.0], yaw, pitch: 0.0, color: [200, 56, 44], glow: false });
                boxes.push(BoxInst { center: at(0.0, 0.96, -0.12), half: [0.54, 0.2, 0.5], yaw, pitch: 0.0, color: [176, 220, 240], glow: false });
                boxes.push(BoxInst { center: at(0.0, 1.18, -0.12), half: [0.56, 0.03, 0.52], yaw, pitch: 0.0, color: [150, 40, 32], glow: false });
                for side in [-1.0, 1.0] {
                    boxes.push(BoxInst { center: at(0.4 * side, 0.56, 1.0), half: [0.12, 0.08, 0.02], yaw, pitch: 0.0, color: [255, 244, 190], glow: true });
                    for end in [-0.62, 0.62] {
                        boxes.push(BoxInst { center: at(0.6 * side, 0.26, end), half: [0.1, 0.25, 0.25], yaw, pitch: 0.0, color: [28, 28, 30], glow: false });
                        boxes.push(BoxInst { center: at(0.66 * side, 0.26, end), half: [0.05, 0.2, 0.05], yaw, pitch: 0.0, color: [190, 190, 196], glow: false });
                    }
                }
            }
            for m in &mobs {
                let (min, max) = m.aabb(cam);
                if frustum.aabb_visible(min, max) {
                    m.push_boxes(&mut boxes);
                }
            }
            pack_box_instances(&boxes, cam, &mut bytes);
            after_boxes = boxes.len();
            after_bytes = bytes.len();
            std::hint::black_box(&bytes);
        }
        let after_ms = t1.elapsed().as_secs_f64() * 1000.0 / frames as f64;

        println!(
            "BENCHMARK (50 canlı + 28 meşale + 6 araba): ÖNCE: {:.4} ms/kare ({} kutu, {} bayt) -> SONRA: {:.4} ms/kare ({} kutu, {} bayt), Hızlanma: {:.1}x, Veri küçülmesi: {:.1}x",
            before_ms,
            before_boxes,
            before_bytes,
            after_ms,
            after_boxes,
            after_bytes,
            before_ms / after_ms.max(1e-9),
            before_bytes as f64 / after_bytes.max(1) as f64
        );
        assert!(after_boxes < before_boxes / 2, "görüş konisi kutuların yarısından fazlasını elemeli");
        assert!(after_bytes * 25 < before_bytes, "instancing + frustum GPU tampon boyutunu >25x küçültmeli");
    }
}
