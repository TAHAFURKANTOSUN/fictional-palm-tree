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

use crate::blocks::{is_liquid, is_solid, AIR, GRASS, LEAVES, TORCH, WATER};
use crate::player::{Player, GRAVITY};
use crate::world::{World, WORLD_HEIGHT};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Sheep,
    Pig,
    Chicken,
    Zombie,
    Shroom,
}

/// Canlının çıkardığı ses türü.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sound {
    /// Ara sıra kendi kendine.
    Idle,
    Hurt,
    /// Zombi oyuncuyu fark etti.
    Alert,
    /// Mantar Kafa zehir tükürdü.
    Spit,
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
    /// Ölünce düşen et sayısı.
    pub meat: u32,
}

impl Kind {
    pub fn stats(self) -> Stats {
        match self {
            Kind::Sheep => Stats { name: "Koyun", radius: 0.42, height: 1.1, health: 8, walk: 1.3, run: 4.6, meat: 2 },
            Kind::Pig => Stats { name: "Domuz", radius: 0.42, height: 0.9, health: 10, walk: 1.4, run: 4.9, meat: 2 },
            Kind::Chicken => Stats { name: "Tavuk", radius: 0.24, height: 0.6, health: 4, walk: 1.1, run: 3.9, meat: 1 },
            Kind::Zombie => Stats { name: "Zombi", radius: 0.3, height: 1.8, health: 20, walk: 1.0, run: 3.6, meat: 0 },
            Kind::Shroom => Stats { name: "Mantar Kafa", radius: 0.32, height: 1.3, health: 14, walk: 1.2, run: 3.2, meat: 0 },
        }
    }

    pub fn hostile(self) -> bool {
        matches!(self, Kind::Zombie | Kind::Shroom)
    }

    /// Ses dosyası adlarında kullanılan kısa anahtar.
    pub fn key(self) -> &'static str {
        match self {
            Kind::Sheep => "sheep",
            Kind::Pig => "pig",
            Kind::Chicken => "chicken",
            Kind::Zombie => "zombie",
            Kind::Shroom => "shroom",
        }
    }
}

/// Zombinin oyuncuyu fark ettiği yatay mesafe.
pub const CHASE_RANGE: f64 = 18.0;
const ATTACK_RANGE: f64 = 1.3;
pub const ZOMBIE_DAMAGE: i32 = 3;
const ATTACK_INTERVAL: f64 = 1.2;
const FLEE_TIME: f64 = 5.0;
const JUMP_SPEED: f64 = 8.6;
/// Kendiliğinden doğma sınırları; üremeyle `BREED_*` sınırına kadar çıkılır.
const MAX_ANIMALS: usize = 8;
const MAX_ZOMBIES: usize = 4;
const BREED_MAX_ANIMALS: usize = 14;
const BREED_MAX_HOSTILE: usize = 6;
/// Eş arama ve çiftleşme mesafeleri.
const LURE_RANGE: f64 = 16.0;
const BREED_RANGE: f64 = 2.6;
const BREED_COOLDOWN: f64 = 60.0;
/// Yavrunun yetişkin olma süresi (saniye).
const GROW_TIME: f64 = 45.0;
/// Mantar Kafa'nın tükürme menzili, aralığı ve tutmak istediği mesafe.
const SPIT_RANGE: f64 = 12.0;
const SPIT_INTERVAL: f64 = 2.4;
const SPIT_SPEED: f64 = 11.0;
const SPIT_GRAVITY: f64 = 9.0;
const KEEP_FAR: f64 = 7.0;
const KEEP_NEAR: f64 = 3.5;
/// Zehirlenmenin süresi (saniye).
pub const POISON_TIME: f64 = 5.0;
const DESPAWN_DIST: f64 = 90.0;

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
    /// Son adımdaki gerçek yatay hız (blok/sn).
    pub speed: f64,
    flee: f64,
    flee_from: [f64; 2],
    attack_cd: f64,
    /// Büyüme: 0 yeni doğmuş yavru, 1 yetişkin.
    pub age: f64,
    breed_cd: f64,
    /// Yakındaki uygun eşin yönü (varsa gezinirken ona doğru yürür).
    lure: Option<f64>,
    voice: f64,
    spit_cd: f64,
    /// Bu adımda çıkarılan ses / atılan tükürük; `Mobs::tick` toplar.
    pending_sound: Option<Sound>,
    pending_spit: bool,
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
            speed: 0.0,
            flee: 0.0,
            flee_from: [pos[0], pos[2]],
            attack_cd: 0.0,
            age: 1.0,
            breed_cd: 20.0,
            lure: None,
            voice: 4.0,
            spit_cd: 1.0,
            pending_sound: None,
            pending_spit: false,
        }
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

    /// Hasar verir; hayvanlar `from` noktasından koşarak kaçmaya başlar.
    /// Öldüyse `true`.
    pub fn damage(&mut self, amount: i32, from: [f64; 3]) -> bool {
        self.health -= amount;
        self.hurt = 1.0;
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

    /// Bir adım. Zombi bu adımda oyuncuya vurduysa `true` döner.
    pub fn update(&mut self, world: &World, dt: f64, player: [f64; 3], rand: &mut dyn FnMut() -> f64) -> bool {
        let s = self.kind.stats();
        self.hurt = (self.hurt - dt * 3.0).max(0.0);
        self.attack_cd = (self.attack_cd - dt).max(0.0);
        self.timer -= dt;
        self.idle_time += dt;
        self.breed_cd = (self.breed_cd - dt).max(0.0);
        self.spit_cd = (self.spit_cd - dt).max(0.0);
        // Büyüme: gövde de boyla birlikte büyür.
        if self.age < 1.0 {
            self.age = (self.age + dt / GROW_TIME).min(1.0);
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

        if self.kind == Kind::Shroom {
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
                    if dist < SPIT_RANGE && self.spit_cd == 0.0 {
                        self.spit_cd = SPIT_INTERVAL;
                        self.pending_spit = true;
                        self.pending_sound = Some(Sound::Spit);
                    }
                }
            } else if self.state == State::Run {
                self.state = State::Idle;
                self.timer = 1.0;
            }
        } else if self.kind.hostile() {
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
        } else if self.state == State::Run {
            self.state = State::Idle;
            self.timer = 1.5;
        }

        // Gezinme: bir süre dur, bir süre rastgele bir yöne yürü.
        if self.state != State::Run && self.timer <= 0.0 {
            if self.state == State::Idle {
                self.state = State::Walk;
                // Üremeye hazırsa ve yakında eş varsa ona doğru yürür.
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
        // (Mantar Kafa oyuncuya yürürken yönünü kendi belirler.)
        if self.state == State::Walk && speed > 0.0 && self.kind != Kind::Shroom {
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

        // Önü tek blokla kapalıysa zıplar.
        if speed > 0.0 && want > 1e-6 && moved < want * 0.5 && grounded {
            self.body.vel[1] = JUMP_SPEED;
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
}

pub enum MobEvent {
    /// Zombi oyuncuya vurdu.
    Attack(i32),
    /// Bir canlı öldü (ganimet ve parçacıklar için).
    Died(Kind, [f64; 3]),
    /// Bir canlı ses çıkardı.
    Sound(Kind, Sound, [f64; 3]),
    /// Bir yavru doğdu.
    Born(Kind, [f64; 3]),
    /// Zehir tükürüğü bir yere çarptı (bulut için).
    SpitHit([f64; 3]),
    /// Zehir tükürüğü oyuncuya isabet etti.
    Poison,
}

/// Havada uçan zehir tükürüğü.
pub struct Spit {
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    life: f64,
}

pub struct Mobs {
    pub list: Vec<Mob>,
    pub spits: Vec<Spit>,
    rng: u32,
    spawn_timer: f64,
    breed_timer: f64,
}

impl Default for Mobs {
    fn default() -> Self {
        Self { list: Vec::new(), spits: Vec::new(), rng: 0x9e37_79b9, spawn_timer: 2.0, breed_timer: 1.0 }
    }
}

/// `(x, z)` sütununun en üst bloğu: `(y, blok)`. Chunk yüklü değilse ya da
/// yüzey su ise `None`.
fn surface(world: &World, x: i32, z: i32) -> Option<(i32, u8)> {
    for y in (1..WORLD_HEIGHT).rev() {
        let b = world.get_block(x, y, z);
        if b == AIR || b == TORCH {
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

    pub fn count(&self, hostile: bool) -> usize {
        self.list.iter().filter(|m| m.kind.hostile() == hostile).count()
    }

    pub fn spawn(&mut self, kind: Kind, pos: [f64; 3]) {
        let yaw = self.rand() * std::f64::consts::TAU;
        self.list.push(Mob::new(kind, pos, yaw));
    }

    /// Oyuncunun çevresinde, görüş alanının biraz uzağında yeni canlı dener.
    fn try_spawn(&mut self, world: &World, player: [f64; 3], night: bool) {
        let hostile = night && self.count(true) < MAX_ZOMBIES && self.rand() < 0.6;
        if !hostile && self.count(false) >= MAX_ANIMALS {
            return;
        }
        let angle = self.rand() * std::f64::consts::TAU;
        let dist = 18.0 + self.rand() * 20.0;
        let (x, z) = ((player[0] + angle.cos() * dist).floor() as i32, (player[2] + angle.sin() * dist).floor() as i32);
        let Some((y, block)) = surface(world, x, z) else { return };
        // Hayvanlar çimende doğar; zombiler ağaç tepesi dışında her yerde.
        if (!hostile && block != GRASS) || block == LEAVES {
            return;
        }
        let kind = if hostile {
            if self.rand() < 0.65 { Kind::Zombie } else { Kind::Shroom }
        } else {
            [Kind::Sheep, Kind::Pig, Kind::Chicken][(self.rand() * 3.0) as usize % 3]
        };
        let mob = Mob::new(kind, [x as f64 + 0.5, y as f64 + 1.0, z as f64 + 0.5], angle);
        if !mob.body.collides(world) {
            self.list.push(mob);
        }
    }

    /// Tüm canlıları bir adım ilerletir; olanları `events`'e yazar.
    pub fn tick(&mut self, world: &World, dt: f64, player: [f64; 3], night: bool, events: &mut Vec<MobEvent>) {
        self.spawn_timer -= dt;
        if self.spawn_timer <= 0.0 {
            self.spawn_timer = 1.5;
            self.try_spawn(world, player, night);
        }

        self.breed_timer -= dt;
        if self.breed_timer <= 0.0 {
            self.breed_timer = 1.0;
            self.breed(night, events);
        }

        let chest = [player[0], player[1] + 0.9, player[2]];
        let mut rng = self.rng;
        let mut rand = move || {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            rng as f64 / 4_294_967_296.0
        };
        for m in self.list.iter_mut() {
            if m.update(world, dt, player, &mut rand) {
                events.push(MobEvent::Attack(ZOMBIE_DAMAGE));
            }
            if let Some(sound) = m.pending_sound.take() {
                events.push(MobEvent::Sound(m.kind, sound, m.body.pos));
            }
            if std::mem::take(&mut m.pending_spit) {
                // Oyuncunun göğsüne nişan alır; düşüşü karşılayacak kadar yukarı atar.
                let from = [m.body.pos[0], m.body.pos[1] + m.body.height * 0.85, m.body.pos[2]];
                let d = [chest[0] - from[0], chest[1] - from[1], chest[2] - from[2]];
                let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
                let t = len / SPIT_SPEED;
                let vel = [d[0] / len * SPIT_SPEED, d[1] / len * SPIT_SPEED + 0.5 * SPIT_GRAVITY * t, d[2] / len * SPIT_SPEED];
                self.spits.push(Spit { pos: from, vel, life: 3.0 });
            }
            // Gece yaratıkları gün ışığında yanar.
            if m.kind.hostile() && !night {
                m.idle_time += dt;
                if (m.idle_time * 2.0).fract() < dt * 2.0 {
                    m.health -= 2;
                    m.hurt = 1.0;
                }
            }
        }
        self.rng = self.rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223) | 1;

        // Tükürükler: küçük adımlarla uçar; bloğa ya da oyuncuya çarpınca biter.
        self.spits.retain_mut(|s| {
            s.life -= dt;
            s.vel[1] -= SPIT_GRAVITY * dt;
            let steps = 4;
            for _ in 0..steps {
                for a in 0..3 {
                    s.pos[a] += s.vel[a] * dt / steps as f64;
                }
                let d2 = (0..3).map(|a| (s.pos[a] - chest[a]).powi(2)).sum::<f64>();
                if d2 < 0.75 * 0.75 {
                    events.push(MobEvent::Poison);
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
                events.push(MobEvent::Died(m.kind, m.body.pos));
                return false;
            }
            let p = m.body.pos;
            (p[0] - player[0]).hypot(p[2] - player[2]) < DESPAWN_DIST && p[1] > -10.0
        });
    }

    /// Üreme: üremeye hazır iki yetişkin yan yanaysa bir yavru doğar; uzaktaysa
    /// birbirlerine doğru çekilirler. Yaratıklar yalnızca gece ürer.
    fn breed(&mut self, night: bool, events: &mut Vec<MobEvent>) {
        let ready = |m: &Mob| m.adult() && m.breed_cd == 0.0 && m.health > 0 && m.state != State::Run;
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
                let pos = [(a[0] + b[0]) * 0.5, a[1].max(b[1]) + 0.1, (a[2] + b[2]) * 0.5];
                let mut baby = Mob::new(kind, pos, self.rand() * std::f64::consts::TAU);
                baby.age = 0.0;
                baby.breed_cd = BREED_COOLDOWN;
                self.list.push(baby);
                events.push(MobEvent::Born(kind, pos));
            } else {
                self.list[i].lure = Some((-(b[0] - a[0])).atan2(-(b[2] - a[2])));
            }
        }
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
        for x in -40..40 {
            for z in -40..40 {
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
        mobs.tick(&w, 0.02, FAR, false, &mut ev);
        assert!(mobs.list.is_empty());
        assert!(ev.iter().any(|e| matches!(e, MobEvent::Died(Kind::Chicken, _))));
        assert_eq!(Kind::Chicken.stats().meat, 1);
        assert_eq!(Kind::Zombie.stats().meat, 0);
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
            mobs.tick(&w, 0.05, player, true, &mut ev);
        }
        assert!(mobs.count(false) > 0 && mobs.count(false) <= BREED_MAX_ANIMALS, "hayvan: {}", mobs.count(false));
        assert!(mobs.count(true) > 0 && mobs.count(true) <= BREED_MAX_HOSTILE, "yaratık: {}", mobs.count(true));
        for m in &mobs.list {
            assert!(m.body.pos[1] > 20.5, "zeminin altında doğmuş: {:?}", m.body.pos);
        }
        for _ in 0..1200 {
            mobs.tick(&w, 0.05, FAR, false, &mut ev);
        }
        assert_eq!(mobs.count(true), 0, "gece yaratıkları gündüz yanmalı");
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
            mobs.tick(&w, 0.05, AWAY, false, &mut ev);
        }
        assert_eq!(mobs.list.len(), 3, "bir yavru doğmalı (ve bekleme süresi ikincisini engellemeli)");
        assert_eq!(ev.iter().filter(|e| matches!(e, MobEvent::Born(Kind::Sheep, _))).count(), 1);
        let baby = &mobs.list[2];
        assert!(!baby.adult() && baby.scale() < 0.6, "yavru küçük doğmalı");
        assert!(baby.body.height < Kind::Sheep.stats().height * 0.6);
        for _ in 0..(GROW_TIME / 0.05) as usize + 10 {
            mobs.tick(&w, 0.05, AWAY, false, &mut ev);
        }
        assert!(mobs.list[2].adult() && mobs.list[2].scale() == 1.0, "yavru büyümeli");
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
            mobs.tick(&w, 0.05, AWAY, false, &mut ev);
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
            mobs.tick(&w, 0.05, AWAY, false, &mut ev);
        }
        assert_eq!(mobs.list.len(), 2, "gündüz üremezler");
        for _ in 0..60 {
            mobs.tick(&w, 0.05, AWAY, true, &mut ev);
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
            mobs.tick(&w, 0.05, player, true, &mut ev);
        }
        let spat = ev.iter().filter(|e| matches!(e, MobEvent::Sound(Kind::Shroom, Sound::Spit, _))).count();
        let poisoned = ev.iter().filter(|e| matches!(e, MobEvent::Poison)).count();
        assert!((3..=5).contains(&spat), "10 saniyede aralıklı tükürmeli: {spat}");
        assert!(poisoned >= 2, "duran oyuncuya isabet etmeli: {poisoned}");
        assert!(mobs.spits.len() <= 1);

        // Oyuncu dibine gelince koşarak uzaklaşır.
        let m = &mobs.list[0];
        let near = [m.body.pos[0] + 1.5, 21.0, m.body.pos[2]];
        let x0 = m.body.pos[0];
        for _ in 0..40 {
            mobs.tick(&w, 0.05, near, true, &mut ev);
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
            mobs.tick(&w, 0.05, [6.5, 21.0, 0.5], true, &mut ev);
        }
        assert!(ev.iter().any(|e| matches!(e, MobEvent::SpitHit(_))), "tükürük duvara çarpmalı");
        assert!(!ev.iter().any(|e| matches!(e, MobEvent::Poison)), "duvarın arkasındaki oyuncu zehirlenmemeli");
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
            mobs.tick(&w, 0.05, AWAY, true, &mut ev);
        }
        let count = |ev: &Vec<MobEvent>, k: Kind, s: Sound| ev.iter().filter(|e| matches!(e, MobEvent::Sound(a, b, _) if *a == k && *b == s)).count();
        assert!((1..=4).contains(&count(&ev, Kind::Sheep, Sound::Idle)), "20 saniyede ara sıra melemeli");
        assert_eq!(count(&ev, Kind::Zombie, Sound::Alert), 0);

        ev.clear();
        mobs.list[0].damage(1, [3.0, 21.0, 0.5]);
        let zp = mobs.list[1].body.pos;
        mobs.tick(&w, 0.05, [zp[0] + 6.0, 21.0, zp[2]], true, &mut ev);
        assert_eq!(count(&ev, Kind::Sheep, Sound::Hurt), 1);
        assert_eq!(count(&ev, Kind::Zombie, Sound::Alert), 1, "oyuncuyu fark edince bir kez hırlamalı");
        mobs.tick(&w, 0.05, [zp[0] + 6.0, 21.0, zp[2]], true, &mut ev);
        assert_eq!(count(&ev, Kind::Zombie, Sound::Alert), 1, "kovalarken tekrar tekrar değil");
    }
}
