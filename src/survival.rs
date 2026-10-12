//! Hayatta kalma: can, açlık, nefes, hasar kaynakları ve oyun modu.
//!
//! Bu modül bilerek **dünyadan bağımsızdır**: kare başına durum `TickInput`
//! ile dışarıdan verilir. Böylece tüm kurallar tarayıcı olmadan test edilebilir.

use crate::blocks::APPLE;
use crate::inventory::{Inventory, Slots};

pub const MAX_HEALTH: i32 = 20; // 10 kalp
pub const MAX_HUNGER: i32 = 20; // 10 but
pub const MAX_AIR: f64 = 20.0; // 10 baloncuk
pub const AIR_SECONDS: f64 = 15.0;
pub const HUNGER_PER_EXHAUSTION: f64 = 4.0;
/// Bu değerin altındayken koşulamaz.
pub const SPRINT_MIN_HUNGER: i32 = 6;
/// Bu yükseklikten kısa düşüşler hasarsızdır.
const SAFE_FALL: f64 = 3.2;
const REGEN_INTERVAL: f64 = 4.0;
const REGEN_MIN_HUNGER: i32 = 18;
const REGEN_EXHAUSTION_COST: f64 = 6.0;
const STARVE_INTERVAL: f64 = 4.0;
const DROWN_INTERVAL: f64 = 1.8;
const DROWN_DAMAGE: i32 = 2;
const INVULN_TIME: f64 = 0.5;
/// Doğuştan sonra verilen dokunulmazlık.
const SPAWN_INVULN: f64 = 2.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GameMode {
    Survival,
    Creative,
}

impl GameMode {
    pub fn label(self) -> &'static str {
        match self {
            GameMode::Survival => "Hayatta Kalma",
            GameMode::Creative => "Yaratıcı",
        }
    }
}

/// Arayüzün gösterdiği bildirimler.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Event {
    Damaged { amount: i32, cause: &'static str },
    Healed,
    Starving,
    Died,
    Respawned,
    ModeChanged(GameMode),
    Ate,
    NotHungry,
    NoFood,
}

/// Kare başına dış dünyadan gelen bilgiler.
#[derive(Clone, Copy, Default)]
pub struct TickInput {
    pub dt: f64,
    pub player_y: f64,
    pub on_ground: bool,
    pub flying: bool,
    pub feet_in_water: bool,
    pub eye_in_water: bool,
    pub moved_dist: f64,
    pub sprinting: bool,
    pub jumped: bool,
    pub broke_block: bool,
}

pub struct Survival {
    pub mode: GameMode,
    pub health: i32,
    pub hunger: i32,
    pub air: f64,
    pub exhaustion: f64,
    /// Hasar sonrası kısa dokunulmazlık.
    pub invuln: f64,
    pub dead: bool,
    /// Ekran kırmızı vinyetinin opaklığı (1 → 0 sönümlenir).
    pub hurt_flash: f64,
    pub eat_cooldown: f64,
    pub fall_peak: Option<f64>,
    saved_inventory: Option<Slots>,
    regen_timer: f64,
    starve_timer: f64,
    drown_timer: f64,
    /// Uygulamanın boşalttığı olay kuyruğu.
    pub events: Vec<Event>,
}

impl Default for Survival {
    fn default() -> Self {
        Self::new()
    }
}

impl Survival {
    pub fn new() -> Self {
        Self {
            mode: GameMode::Survival,
            health: MAX_HEALTH,
            hunger: MAX_HUNGER,
            air: MAX_AIR,
            exhaustion: 0.0,
            invuln: 0.0,
            dead: false,
            hurt_flash: 0.0,
            eat_cooldown: 0.0,
            fall_peak: None,
            saved_inventory: None,
            regen_timer: REGEN_INTERVAL,
            starve_timer: STARVE_INTERVAL,
            drown_timer: 0.0,
            events: Vec::new(),
        }
    }

    pub fn is_creative(&self) -> bool {
        self.mode == GameMode::Creative
    }

    /// Yaratıcıda koşma serbest; hayatta kalmada açlık yetmeli.
    pub fn can_sprint(&self) -> bool {
        self.is_creative() || self.hunger > SPRINT_MIN_HUNGER
    }

    /// Açlık barının gösterilmesi gerekiyor mu?
    pub fn vitals_visible(&self) -> bool {
        !self.is_creative()
    }

    fn reset_vitals(&mut self) {
        self.health = MAX_HEALTH;
        self.hunger = MAX_HUNGER;
        self.air = MAX_AIR;
        self.exhaustion = 0.0;
        self.regen_timer = REGEN_INTERVAL;
        self.starve_timer = STARVE_INTERVAL;
        self.drown_timer = 0.0;
    }

    /// Hasar verir. Yaratıcıda, ölüyken ve dokunulmazlık sırasında yok sayılır.
    pub fn damage(&mut self, amount: i32, cause: &'static str) {
        if self.is_creative() || self.dead || self.invuln > 0.0 || amount <= 0 {
            return;
        }
        self.health = (self.health - amount).max(0);
        self.invuln = INVULN_TIME;
        self.hurt_flash = 1.0;
        self.events.push(Event::Damaged { amount, cause });
        if self.health <= 0 {
            self.dead = true;
            self.events.push(Event::Died);
        }
    }

    pub fn heal(&mut self, amount: i32) {
        if self.dead || amount <= 0 {
            return;
        }
        let before = self.health;
        self.health = (self.health + amount).min(MAX_HEALTH);
        if self.health != before {
            self.events.push(Event::Healed);
        }
    }

    /// Ölümden sonra doğuş noktasında sıfırlanır.
    pub fn respawn(&mut self) {
        self.reset_vitals();
        self.invuln = SPAWN_INVULN;
        self.fall_peak = None;
        self.dead = false;
        self.hurt_flash = 0.0;
        self.events.push(Event::Respawned);
    }

    /// Mod değiştirir. Yaratıcıya geçerken envanter yedeklenir ve palet verilir;
    /// hayatta kalmaya dönerken yedek geri yüklenir.
    pub fn set_mode(&mut self, mode: GameMode, inv: &mut Inventory) {
        if mode == self.mode {
            return;
        }
        match mode {
            GameMode::Creative => {
                self.saved_inventory = Some(inv.snapshot());
                inv.give_creative_palette();
                self.reset_vitals();
            }
            GameMode::Survival => {
                if let Some(snap) = self.saved_inventory.take() {
                    inv.restore(snap);
                }
                self.reset_vitals();
                self.invuln = 1.5;
                self.fall_peak = None;
            }
        }
        self.mode = mode;
        self.events.push(Event::ModeChanged(mode));
    }

    pub fn toggle_mode(&mut self, inv: &mut Inventory) {
        let next = if self.is_creative() {
            GameMode::Survival
        } else {
            GameMode::Creative
        };
        self.set_mode(next, inv);
    }

    /// Elmayı yer. `true` dönerse bir adet harcanmıştır.
    pub fn try_eat(&mut self, inv: &mut Inventory) -> bool {
        let Some(stack) = inv.selected_stack() else {
            self.events.push(Event::NoFood);
            return false;
        };
        if stack.id != APPLE {
            self.events.push(Event::NoFood);
            return false;
        }
        if self.eat_cooldown > 0.0 {
            return false;
        }
        if self.hunger >= MAX_HUNGER {
            self.events.push(Event::NotHungry);
            return false;
        }
        inv.consume_selected();
        self.hunger = (self.hunger + 6).min(MAX_HUNGER);
        self.exhaustion = (self.exhaustion - 4.0).max(0.0);
        self.eat_cooldown = 0.6;
        self.events.push(Event::Ate);
        true
    }

    /// Kare başına tüm hayatta kalma kuralları.
    pub fn tick(&mut self, input: &TickInput) {
        let dt = input.dt;
        if self.eat_cooldown > 0.0 {
            self.eat_cooldown -= dt;
        }
        if self.invuln > 0.0 {
            self.invuln -= dt;
        }
        if self.hurt_flash > 0.0 {
            self.hurt_flash = (self.hurt_flash - dt * 2.2).max(0.0);
        }

        if self.is_creative() || self.dead {
            if self.is_creative() {
                self.air = MAX_AIR;
            }
            return;
        }

        // --- nefes / boğulma ---
        if input.eye_in_water {
            self.air -= dt * (MAX_AIR / AIR_SECONDS);
            if self.air <= 0.0 {
                self.air = 0.0;
                self.drown_timer -= dt;
                if self.drown_timer <= 0.0 {
                    self.drown_timer = DROWN_INTERVAL;
                    self.damage(DROWN_DAMAGE, "boğulma");
                }
            }
        } else {
            self.air = (self.air + dt * (MAX_AIR / 3.0)).min(MAX_AIR);
            self.drown_timer = 0.0;
        }

        // --- düşme hasarı: havadayken en yüksek nokta izlenir ---
        if input.on_ground {
            if let Some(peak) = self.fall_peak.take() {
                let drop = peak - input.player_y;
                if drop > SAFE_FALL {
                    self.damage((drop - 3.0).floor() as i32, "düşme");
                }
            }
        } else if !input.flying && !input.feet_in_water {
            self.fall_peak = Some(match self.fall_peak {
                Some(p) => p.max(input.player_y),
                None => input.player_y,
            });
        } else {
            self.fall_peak = None;
        }

        // --- yorgunluk birikimi ---
        if input.jumped {
            self.exhaustion += if input.sprinting { 0.4 } else { 0.2 };
        }
        if input.broke_block {
            self.exhaustion += 0.005;
        }
        if input.moved_dist > 0.0 {
            self.exhaustion += if input.flying {
                input.moved_dist * 0.008
            } else if input.feet_in_water {
                input.moved_dist * 0.012
            } else if input.on_ground {
                input.moved_dist * if input.sprinting { 0.1 } else { 0.01 }
            } else {
                0.0
            };
        }

        // --- yorgunluk → açlık ---
        while self.exhaustion >= HUNGER_PER_EXHAUSTION {
            self.exhaustion -= HUNGER_PER_EXHAUSTION;
            if self.hunger > 0 {
                self.hunger -= 1;
            }
        }

        // --- açlık sıfır → yıldız açlığı ---
        if self.hunger <= 0 {
            self.starve_timer -= dt;
            if self.starve_timer <= 0.0 {
                self.starve_timer = STARVE_INTERVAL;
                self.events.push(Event::Starving);
                self.damage(1, "açlık");
            }
        } else {
            self.starve_timer = STARVE_INTERVAL;
        }

        // --- tokken iyileşme (karşılığında açlık harcanır) ---
        if self.hunger >= REGEN_MIN_HUNGER && self.health < MAX_HEALTH {
            self.regen_timer -= dt;
            if self.regen_timer <= 0.0 {
                self.regen_timer = REGEN_INTERVAL;
                self.heal(1);
                self.exhaustion += REGEN_EXHAUSTION_COST;
            }
        } else {
            self.regen_timer = REGEN_INTERVAL;
        }
    }

    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::{Stack, MAX_STACK};

    fn tick_for(s: &mut Survival, seconds: f64, base: TickInput) {
        let steps = (seconds / 0.1).round() as i32;
        for _ in 0..steps {
            let mut i = base;
            i.dt = 0.1;
            s.tick(&i);
        }
    }

    fn grounded() -> TickInput {
        TickInput { on_ground: true, player_y: 30.0, ..Default::default() }
    }

    #[test]
    fn starts_full_and_visible() {
        let s = Survival::new();
        assert_eq!(s.health, MAX_HEALTH);
        assert_eq!(s.hunger, MAX_HUNGER);
        assert_eq!(s.air, MAX_AIR);
        assert!(s.vitals_visible());
        assert_eq!(s.mode, GameMode::Survival);
    }

    #[test]
    fn damage_reduces_health_and_reports_the_cause() {
        let mut s = Survival::new();
        s.damage(6, "düşme");
        assert_eq!(s.health, 14);
        let ev = s.take_events();
        assert!(ev.contains(&Event::Damaged { amount: 6, cause: "düşme" }));
        assert!(s.hurt_flash > 0.9);
    }

    #[test]
    fn invulnerability_blocks_consecutive_hits() {
        let mut s = Survival::new();
        s.damage(5, "test");
        s.damage(5, "test");
        assert_eq!(s.health, 15, "dokunulmazlık ikinci hasarı engellemeli");
    }

    #[test]
    fn damage_after_invulnerability_expires() {
        let mut s = Survival::new();
        s.damage(5, "test");
        tick_for(&mut s, 1.0, grounded());
        s.damage(5, "test");
        assert_eq!(s.health, 10);
    }

    #[test]
    fn death_sets_the_flag_and_emits_an_event() {
        let mut s = Survival::new();
        s.health = 3;
        s.damage(10, "test");
        assert_eq!(s.health, 0);
        assert!(s.dead);
        assert!(s.take_events().contains(&Event::Died));
    }

    #[test]
    fn respawn_restores_everything() {
        let mut s = Survival::new();
        s.health = 0;
        s.hunger = 0;
        s.air = 0.0;
        s.dead = true;
        s.respawn();
        assert!(!s.dead);
        assert_eq!(s.health, MAX_HEALTH);
        assert_eq!(s.hunger, MAX_HUNGER);
        assert_eq!(s.air, MAX_AIR);
        assert!(s.invuln > 0.0);
    }

    #[test]
    fn starvation_deals_one_damage_every_four_seconds() {
        let mut s = Survival::new();
        s.hunger = 0;
        tick_for(&mut s, 20.0, grounded());
        // 20 saniyede ~5 vuruş, dokunulmazlık süreleri düşülünce 15 civarı.
        assert_eq!(s.health, 15, "beklenen 15, bulunan {}", s.health);
    }

    #[test]
    fn regeneration_heals_when_well_fed() {
        let mut s = Survival::new();
        s.health = 8;
        s.hunger = 20;
        tick_for(&mut s, 20.0, grounded());
        assert!(s.health > 8, "iyileşme olmadı: {}", s.health);
        assert!(s.health <= 12, "iyileşme çok hızlı: {}", s.health);
    }

    #[test]
    fn regeneration_stops_once_hunger_drops() {
        let mut s = Survival::new();
        s.health = 8;
        s.hunger = 19;
        tick_for(&mut s, 60.0, grounded());
        assert!(s.hunger < REGEN_MIN_HUNGER, "iyileşme açlığı tüketmeli");
        let after = s.health;
        tick_for(&mut s, 40.0, grounded());
        assert!(s.health <= after + 1, "açlık düşünce iyileşme durmalı");
    }

    #[test]
    fn sprinting_requires_food() {
        let mut s = Survival::new();
        assert!(s.can_sprint());
        s.hunger = 6;
        assert!(!s.can_sprint(), "açlık 6 iken koşulmamalı");
        s.hunger = 7;
        assert!(s.can_sprint());
    }

    #[test]
    fn creative_ignores_damage_and_keeps_vitals_hidden() {
        let mut s = Survival::new();
        let mut inv = Inventory::new();
        s.set_mode(GameMode::Creative, &mut inv);
        assert!(s.is_creative());
        assert!(!s.vitals_visible());
        s.damage(20, "test");
        assert_eq!(s.health, MAX_HEALTH, "yaratıcıda hasar işlememeli");
        assert!(!s.dead);
    }

    #[test]
    fn creative_always_allows_sprinting() {
        let mut s = Survival::new();
        let mut inv = Inventory::new();
        s.set_mode(GameMode::Creative, &mut inv);
        s.hunger = 0;
        assert!(s.can_sprint());
    }

    #[test]
    fn mode_switch_swaps_the_inventory_and_restores_it() {
        let mut s = Survival::new();
        let mut inv = Inventory::new();
        inv.give_starter_kit();
        assert_eq!(inv.count(crate::blocks::GRASS as u16), 16);

        s.set_mode(GameMode::Creative, &mut inv);
        assert_eq!(inv.count(crate::blocks::GRASS as u16), 64, "yaratıcı paleti gelmedi");

        s.set_mode(GameMode::Survival, &mut inv);
        assert_eq!(inv.count(crate::blocks::GRASS as u16), 16, "envanter geri gelmedi");
        assert_eq!(inv.count(APPLE), 3);
        assert!(s.invuln > 0.0, "mod değişiminde kısa dokunulmazlık verilmeli");
    }

    #[test]
    fn fall_damage_only_above_the_safe_height() {
        // 3 blokluk düşüş: güvenli sınırın altında, hasarsız.
        let mut s = Survival::new();
        s.tick(&TickInput { dt: 0.1, player_y: 33.0, on_ground: false, ..Default::default() });
        s.tick(&TickInput { dt: 0.1, player_y: 30.0, on_ground: true, ..Default::default() });
        assert_eq!(s.health, MAX_HEALTH, "kısa düşüş hasar vermemeli");

        // 18 blokluk düşüş: hasarlı.
        let mut s = Survival::new();
        s.tick(&TickInput { dt: 0.1, player_y: 48.0, on_ground: false, ..Default::default() });
        s.tick(&TickInput { dt: 0.1, player_y: 30.0, on_ground: true, ..Default::default() });
        assert!(s.health < MAX_HEALTH, "uzun düşüş hasar vermeli");
        // drop = 18 → hasar = floor(18-3) = 15
        assert_eq!(s.health, MAX_HEALTH - 15);
    }

    #[test]
    fn a_four_block_fall_hurts_exactly_one() {
        let mut s = Survival::new();
        s.tick(&TickInput { dt: 0.1, player_y: 34.0, on_ground: false, ..Default::default() });
        s.tick(&TickInput { dt: 0.1, player_y: 30.0, on_ground: true, ..Default::default() });
        assert_eq!(s.health, MAX_HEALTH - 1);
    }

    #[test]
    fn flying_cancels_fall_damage() {
        let mut s = Survival::new();
        s.tick(&TickInput { dt: 0.1, player_y: 60.0, on_ground: false, flying: true, ..Default::default() });
        s.tick(&TickInput { dt: 0.1, player_y: 30.0, on_ground: true, ..Default::default() });
        assert_eq!(s.health, MAX_HEALTH, "uçarken düşme hasarı olmamalı");
    }

    #[test]
    fn water_cancels_fall_damage() {
        let mut s = Survival::new();
        s.tick(&TickInput { dt: 0.1, player_y: 60.0, on_ground: false, feet_in_water: true, ..Default::default() });
        s.tick(&TickInput { dt: 0.1, player_y: 30.0, on_ground: true, ..Default::default() });
        assert_eq!(s.health, MAX_HEALTH);
    }

    #[test]
    fn drowning_drains_air_then_deals_damage() {
        let mut s = Survival::new();
        let underwater = TickInput { eye_in_water: true, feet_in_water: true, ..Default::default() };
        tick_for(&mut s, 26.0, underwater);
        assert_eq!(s.air, 0.0, "nefes tükenmeli");
        assert!(s.health < MAX_HEALTH, "boğulma hasar vermeli: {}", s.health);
    }

    #[test]
    fn air_refills_when_surfacing() {
        let mut s = Survival::new();
        tick_for(&mut s, 5.0, TickInput { eye_in_water: true, ..Default::default() });
        assert!(s.air < MAX_AIR);
        tick_for(&mut s, 5.0, grounded());
        assert_eq!(s.air, MAX_AIR, "yüzeye çıkınca nefes dolmalı");
    }

    #[test]
    fn running_drains_hunger_but_walking_barely_does() {
        let mut walk = Survival::new();
        tick_for(&mut walk, 60.0, TickInput { dt: 0.1, on_ground: true, moved_dist: 0.52, ..Default::default() });
        let walk_hunger = walk.hunger;

        let mut run = Survival::new();
        tick_for(&mut run, 60.0, TickInput { dt: 0.1, on_ground: true, moved_dist: 0.78, sprinting: true, ..Default::default() });
        assert!(run.hunger < walk_hunger, "koşmak daha çok acıktırmalı");
    }

    #[test]
    fn jumping_costs_exhaustion() {
        let mut s = Survival::new();
        for _ in 0..40 {
            s.tick(&TickInput { dt: 0.1, on_ground: true, jumped: true, ..Default::default() });
        }
        assert!(s.hunger < MAX_HUNGER, "zıplamak açlığı tüketmeli");
    }

    #[test]
    fn eating_restores_hunger_and_consumes_an_apple() {
        let mut s = Survival::new();
        let mut inv = Inventory::new();
        inv.slots[0] = Some(Stack::new(APPLE, 3));
        inv.select(0);
        s.hunger = 6;

        assert!(s.try_eat(&mut inv));
        assert_eq!(s.hunger, 12);
        assert_eq!(inv.count(APPLE), 2);
        assert!(s.take_events().contains(&Event::Ate));
    }

    #[test]
    fn eating_is_refused_when_full_or_holding_something_else() {
        let mut s = Survival::new();
        let mut inv = Inventory::new();
        inv.slots[0] = Some(Stack::new(APPLE, 1));
        inv.select(0);
        assert!(!s.try_eat(&mut inv), "tokken yememeli");
        assert_eq!(inv.count(APPLE), 1);
        assert!(s.take_events().contains(&Event::NotHungry));

        let mut s = Survival::new();
        s.hunger = 5;
        let mut inv = Inventory::new();
        inv.slots[0] = Some(Stack::new(crate::blocks::STONE as u16, 1));
        inv.select(0);
        assert!(!s.try_eat(&mut inv));
        assert!(s.take_events().contains(&Event::NoFood));
    }

    #[test]
    fn eating_has_a_cooldown() {
        let mut s = Survival::new();
        let mut inv = Inventory::new();
        inv.slots[0] = Some(Stack::new(APPLE, 5));
        inv.select(0);
        s.hunger = 2;
        assert!(s.try_eat(&mut inv));
        assert!(!s.try_eat(&mut inv), "bekleme süresi dolmadan yememeli");
        tick_for(&mut s, 1.0, grounded());
        assert!(s.try_eat(&mut inv));
    }

    #[test]
    fn hunger_never_goes_below_zero() {
        let mut s = Survival::new();
        tick_for(&mut s, 400.0, TickInput { dt: 0.1, on_ground: true, moved_dist: 0.78, sprinting: true, ..Default::default() });
        assert!(s.hunger >= 0);
    }

    #[test]
    fn inventory_survives_a_full_creative_round_trip() {
        let mut s = Survival::new();
        let mut inv = Inventory::new();
        inv.give_starter_kit();
        let before = inv.snapshot();

        s.toggle_mode(&mut inv);
        assert!(s.is_creative());
        // Yaratıcıda bir şeyler al ve koy.
        inv.add(crate::blocks::BRICK as u16, MAX_STACK as u32);

        s.toggle_mode(&mut inv);
        assert!(!s.is_creative());
        assert_eq!(inv.snapshot(), before, "yaratıcı turu envanteri bozdu");
    }
}
