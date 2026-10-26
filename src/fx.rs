//! Motion: easing helpers, the global "animations on/off" switch and the sand
//! effect (grains that crumble off the page edge while the settings scroll).

use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui::{self, Color32, Pos2, Rect, Vec2};

static ENABLED: AtomicBool = AtomicBool::new(true);

/// Turn all animations on or off (the "Анимации" setting).
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

pub fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Progress 0..=1 of an animation that started at `start` (seconds), after
/// `delay`, lasting `duration`. Always 1 when animations are off.
pub fn progress(now: f64, start: f64, delay: f32, duration: f32) -> f32 {
    progress_if(enabled(), now, start, delay, duration)
}

fn progress_if(on: bool, now: f64, start: f64, delay: f32, duration: f32) -> f32 {
    if !on {
        return 1.0;
    }
    (((now - start) as f32 - delay) / duration.max(0.001)).clamp(0.0, 1.0)
}

/// Small deterministic random numbers (xorshift): the same seed gives the same sand.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// 0.0..1.0
    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.unit()
    }

    pub fn index(&mut self, len: usize) -> usize {
        (self.next_u64() % len.max(1) as u64) as usize
    }
}

/// The edge of the page the sand falls from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
}

#[derive(Clone, Debug)]
struct Grain {
    pos: Pos2,
    vel: Vec2,
    age: f32,
    life: f32,
    size: f32,
    drift: f32,
    color: Color32,
}

/// A cloud of sand grains with a little physics: they are thrown off the edge,
/// pulled down by gravity, slowed by the air and fade out.
#[derive(Clone, Debug)]
pub struct Sand {
    grains: Vec<Grain>,
    rng: Rng,
    /// Sideways push in points per second² (the sand drifts with the wind).
    pub wind: f32,
}

impl Sand {
    pub const MAX_GRAINS: usize = 2400;
    const GRAVITY: f32 = 640.0;
    const DRAG: f32 = 1.5;

    pub fn new(seed: u64) -> Self {
        Self { grains: Vec::new(), rng: Rng::new(seed), wind: 0.0 }
    }

    pub fn len(&self) -> usize {
        self.grains.len()
    }

    pub fn is_empty(&self) -> bool {
        self.grains.is_empty()
    }

    pub fn clear(&mut self) {
        self.grains.clear();
    }

    /// Throw `count` grains off the `edge` of `area`. `strength` (about 0.3..2) is how hard.
    pub fn emit(&mut self, area: Rect, edge: Edge, count: usize, strength: f32, palette: &[Color32]) {
        self.emit_deep(area, edge, count, strength, palette, 30.0);
    }

    /// Like [`Self::emit`], but the grains break off anywhere within `depth` points of the edge.
    pub fn emit_deep(&mut self, area: Rect, edge: Edge, count: usize, strength: f32, palette: &[Color32], depth: f32) {
        if palette.is_empty() || area.width() <= 1.0 {
            return;
        }
        let strength = strength.clamp(0.1, 3.0);
        for _ in 0..count {
            if self.grains.len() >= Self::MAX_GRAINS {
                break;
            }
            let x = self.rng.range(area.left(), area.right());
            let (y, vy) = match edge {
                // the content leaves upwards: the grains jump up, then fall
                Edge::Top => (area.top() + self.rng.range(0.0, depth), -self.rng.range(60.0, 420.0) * strength),
                // the content leaves downwards: the grains are carried down
                Edge::Bottom => (area.bottom() - self.rng.range(0.0, depth), self.rng.range(20.0, 300.0) * strength),
            };
            let vx = self.rng.range(-1.0, 1.0) * self.rng.range(30.0, 320.0) * strength;
            self.grains.push(Grain {
                pos: Pos2::new(x, y),
                vel: Vec2::new(vx, vy),
                age: 0.0,
                life: self.rng.range(0.8, 2.0),
                size: self.rng.range(2.0, 5.0),
                drift: self.rng.range(-40.0, 40.0),
                color: palette[self.rng.index(palette.len())],
            });
        }
    }

    /// A round burst of grains flying out of `center` in every direction (sparks).
    pub fn burst(&mut self, center: Pos2, count: usize, speed: f32, palette: &[Color32]) {
        if palette.is_empty() {
            return;
        }
        for _ in 0..count {
            if self.grains.len() >= Self::MAX_GRAINS {
                break;
            }
            let angle = self.rng.range(0.0, std::f32::consts::TAU);
            let v = speed * self.rng.range(0.25, 1.0);
            let r = self.rng.range(0.0, 8.0);
            self.grains.push(Grain {
                pos: center + Vec2::new(angle.cos(), angle.sin()) * r,
                vel: Vec2::new(angle.cos() * v, angle.sin() * v - speed * 0.25),
                age: 0.0,
                life: self.rng.range(0.5, 1.2),
                size: self.rng.range(2.0, 4.5),
                drift: 0.0,
                color: palette[self.rng.index(palette.len())],
            });
        }
    }

    pub fn update(&mut self, dt: f32) {
        let dt = dt.clamp(0.0, 0.1);
        let wind = self.wind;
        for g in &mut self.grains {
            g.vel.y += Self::GRAVITY * dt;
            g.vel.x += (g.drift + wind) * dt;
            let slow = (1.0 - Self::DRAG * dt).max(0.0);
            g.vel *= slow;
            g.pos += g.vel * dt;
            g.age += dt;
        }
        self.grains.retain(|g| g.age < g.life);
    }

    /// Opacity of a grain at this age: it thins out towards the end of its life.
    pub fn alpha(age: f32, life: f32) -> f32 {
        (1.0 - age / life.max(0.001)).clamp(0.0, 1.0).powf(1.3)
    }

    /// Paint the grains: squares in the pixel style, dots otherwise.
    pub fn paint(&self, painter: &egui::Painter, square: bool) {
        for g in &self.grains {
            let color = g.color.gamma_multiply(Self::alpha(g.age, g.life));
            if square {
                // snap to whole pixels so it looks like pixel-art sand
                let p = Pos2::new(g.pos.x.round(), g.pos.y.round());
                painter.rect_filled(Rect::from_center_size(p, Vec2::splat(g.size.round().max(2.0))), 0.0, color);
            } else {
                painter.circle_filled(g.pos, g.size * 0.5, color);
            }
        }
    }

    /// Grains of the given area (for tests and fades).
    pub fn positions(&self) -> impl Iterator<Item = Pos2> + '_ {
        self.grains.iter().map(|g| g.pos)
    }
}

/// Pseudo-random number 0..1 for a cell of a grid (the same cell always gives the same number).
pub fn hash_unit(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA77) ^ seed.wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    (h >> 8) as f32 / (1u32 << 24) as f32
}

/// The edge of a page crumbling away: square blocks of the background `color`
/// laid over the content, dense at the `edge` and rarer deeper inside the
/// `band`. `strength` (0..=1) is how much has crumbled. The pattern is fixed
/// (the same blocks stay), so growing `strength` makes more of them appear.
pub fn dissolve_mesh(area: Rect, edge: Edge, strength: f32, band: f32, cell: f32, color: Color32, seed: u32) -> egui::Mesh {
    let mut mesh = egui::Mesh::default();
    let strength = strength.clamp(0.0, 1.0);
    if strength <= 0.0 || cell < 1.0 || band < cell || area.width() < cell {
        return mesh;
    }
    let rows = (band / cell).floor() as i32;
    let cols = (area.width() / cell).ceil() as i32;
    for r in 0..rows {
        // 0 at the edge, 1 at the inner end of the band
        let depth = (r as f32 + 0.5) / rows as f32;
        let chance = strength * (1.0 - depth).powf(1.3);
        for c in 0..cols {
            if hash_unit(c, r, seed) >= chance {
                continue;
            }
            let x0 = area.left() + c as f32 * cell;
            let y0 = match edge {
                Edge::Top => area.top() + r as f32 * cell,
                Edge::Bottom => area.bottom() - (r + 1) as f32 * cell,
            };
            let rect = Rect::from_min_max(Pos2::new(x0, y0), Pos2::new((x0 + cell).min(area.right()), y0 + cell)).intersect(area);
            if rect.is_positive() {
                mesh.add_colored_rect(rect, color);
            }
        }
    }
    mesh
}

/// Colors of the sand: warm sand tones, a few grains of the text color and of the accent.
pub fn palette(accent: Color32, text: Color32) -> [Color32; 8] {
    [
        Color32::from_rgb(0xe8, 0xd2, 0x9a),
        Color32::from_rgb(0xd6, 0xb8, 0x7a),
        Color32::from_rgb(0xf2, 0xe2, 0xb8),
        Color32::from_rgb(0xb8, 0x9a, 0x62),
        Color32::from_rgb(0xe0, 0xc6, 0x8c),
        Color32::from_rgb(0xc9, 0xa8, 0x6a),
        accent,
        text.gamma_multiply(0.8),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> Rect {
        Rect::from_min_max(Pos2::new(100.0, 100.0), Pos2::new(500.0, 400.0))
    }

    #[test]
    fn easing_stays_between_zero_and_one() {
        for k in 0..=20 {
            let t = k as f32 / 20.0;
            for f in [ease_out_cubic, ease_in_out] {
                let v = f(t);
                assert!((0.0..=1.0).contains(&v));
            }
        }
        assert_eq!((ease_out_cubic(0.0), ease_out_cubic(1.0)), (0.0, 1.0));
        assert!(ease_out_cubic(0.3) > 0.3, "fast start");
        assert!(ease_in_out(0.5) > 0.49 && ease_in_out(0.5) < 0.51);
        assert_eq!(ease_out_cubic(5.0), 1.0, "out of range is clamped");
    }

    #[test]
    fn progress_follows_time_and_the_switch() {
        // (the global switch is not touched: other tests run at the same time)
        assert_eq!(progress_if(true, 10.0, 10.0, 0.0, 0.5), 0.0);
        assert!((progress_if(true, 10.25, 10.0, 0.0, 0.5) - 0.5).abs() < 1e-3);
        assert_eq!(progress_if(true, 11.0, 10.0, 0.0, 0.5), 1.0);
        assert_eq!(progress_if(true, 10.1, 10.0, 0.2, 0.5), 0.0, "delayed");
        assert_eq!(progress_if(false, 10.0, 10.0, 0.0, 0.5), 1.0, "no animation: already done");
    }

    #[test]
    fn random_numbers_are_repeatable_and_in_range() {
        let (mut a, mut b) = (Rng::new(7), Rng::new(7));
        for _ in 0..200 {
            let (x, y) = (a.unit(), b.unit());
            assert_eq!(x, y);
            assert!((0.0..1.0).contains(&x));
        }
        let mut r = Rng::new(1);
        assert!((0..100).all(|_| (3.0..5.0).contains(&r.range(3.0, 5.0))));
        assert!((0..100).all(|_| r.index(4) < 4));
        assert_ne!(Rng::new(1).next_u64(), Rng::new(2).next_u64());
        assert!(Rng::new(0).unit() >= 0.0, "a zero seed still works");
    }

    #[test]
    fn grains_start_on_the_edge_and_leave_in_the_right_direction() {
        let pal = palette(Color32::GREEN, Color32::WHITE);
        let mut top = Sand::new(3);
        top.emit(area(), Edge::Top, 200, 1.0, &pal);
        assert_eq!(top.len(), 200);
        assert!(top.positions().all(|p| (100.0..=130.5).contains(&p.y) && (100.0..=500.0).contains(&p.x)));
        let y0: f32 = top.positions().map(|p| p.y).sum::<f32>() / 200.0;
        top.update(0.05);
        let y1: f32 = top.positions().map(|p| p.y).sum::<f32>() / 200.0;
        assert!(y1 < y0, "grains thrown off the top go up first");

        let mut bottom = Sand::new(3);
        bottom.emit(area(), Edge::Bottom, 200, 1.0, &pal);
        assert!(bottom.positions().all(|p| (369.5..=400.0).contains(&p.y)));
        let b0: f32 = bottom.positions().map(|p| p.y).sum::<f32>() / 200.0;
        bottom.update(0.05);
        let b1: f32 = bottom.positions().map(|p| p.y).sum::<f32>() / 200.0;
        assert!(b1 > b0, "grains leaving the bottom go down");
    }

    #[test]
    fn grains_fall_and_disappear() {
        let pal = palette(Color32::GREEN, Color32::WHITE);
        let mut s = Sand::new(9);
        s.emit(area(), Edge::Top, 300, 1.5, &pal);
        // gravity: after a while the average grain is lower than where it started
        for _ in 0..30 {
            s.update(0.03);
        }
        let avg: f32 = s.positions().map(|p| p.y).sum::<f32>() / s.len().max(1) as f32;
        assert!(avg > 100.0, "{avg}");
        for _ in 0..100 {
            s.update(0.03);
        }
        assert!(s.is_empty(), "all grains live only a couple of seconds, {} left", s.len());
    }

    #[test]
    fn sand_never_grows_without_limit_and_is_repeatable() {
        let pal = palette(Color32::GREEN, Color32::WHITE);
        let mut s = Sand::new(1);
        for _ in 0..100 {
            s.emit(area(), Edge::Bottom, 500, 1.0, &pal);
        }
        assert_eq!(s.len(), Sand::MAX_GRAINS);
        s.clear();
        assert!(s.is_empty());
        s.emit(Rect::NOTHING, Edge::Top, 10, 1.0, &pal);
        s.emit(area(), Edge::Top, 10, 1.0, &[]);
        assert!(s.is_empty(), "no area or no colors: no sand");

        let (mut a, mut b) = (Sand::new(5), Sand::new(5));
        a.emit(area(), Edge::Top, 50, 1.0, &pal);
        b.emit(area(), Edge::Top, 50, 1.0, &pal);
        a.update(0.02);
        b.update(0.02);
        assert!(a.positions().zip(b.positions()).all(|(p, q)| p == q));
    }

    #[test]
    fn the_wind_blows_sideways() {
        let pal = palette(Color32::GREEN, Color32::WHITE);
        let mut s = Sand::new(4);
        s.wind = 400.0;
        s.emit(area(), Edge::Top, 300, 0.3, &pal);
        let x0: f32 = s.positions().map(|p| p.x).sum::<f32>() / 300.0;
        for _ in 0..12 {
            s.update(0.03);
        }
        let x1: f32 = s.positions().map(|p| p.x).sum::<f32>() / s.len() as f32;
        assert!(x1 > x0 + 10.0, "sand drifts with the wind: {x0} -> {x1}");
    }

    #[test]
    fn a_burst_flies_in_every_direction() {
        let pal = palette(Color32::GREEN, Color32::WHITE);
        let mut s = Sand::new(6);
        let c = Pos2::new(300.0, 300.0);
        s.burst(c, 200, 300.0, &pal);
        assert_eq!(s.len(), 200);
        s.update(0.1);
        let (mut left, mut right, mut up, mut down) = (0, 0, 0, 0);
        for p in s.positions() {
            if p.x < c.x - 5.0 { left += 1 }
            if p.x > c.x + 5.0 { right += 1 }
            if p.y < c.y - 5.0 { up += 1 }
            if p.y > c.y + 5.0 { down += 1 }
        }
        assert!(left > 30 && right > 30 && up > 30 && down > 5, "{left} {right} {up} {down}");
        s.burst(c, 10, 100.0, &[]);
        assert_eq!(s.len(), 200, "no colors: no sparks");
    }

    #[test]
    fn deep_emission_breaks_off_lower_than_the_edge() {
        let pal = palette(Color32::GREEN, Color32::WHITE);
        let mut s = Sand::new(8);
        s.emit_deep(area(), Edge::Top, 400, 1.0, &pal, 90.0);
        assert!(s.positions().all(|p| (100.0..=190.5).contains(&p.y)));
        assert!(s.positions().any(|p| p.y > 150.0), "some grains come from deep inside");
    }

    #[test]
    fn the_crumbling_edge_is_dense_at_the_edge_and_grows_with_strength() {
        let count = |m: &egui::Mesh| m.vertices.len() / 4;
        let none = dissolve_mesh(area(), Edge::Top, 0.0, 100.0, 6.0, Color32::BLACK, 1);
        let half = dissolve_mesh(area(), Edge::Top, 0.5, 100.0, 6.0, Color32::BLACK, 1);
        let full = dissolve_mesh(area(), Edge::Top, 1.0, 100.0, 6.0, Color32::BLACK, 1);
        assert_eq!(count(&none), 0);
        assert!(count(&half) > 20 && count(&full) > count(&half), "{} {}", count(&half), count(&full));
        // blocks lie inside the area, more of them near the edge than deep inside
        let (mut near, mut far) = (0, 0);
        for q in full.vertices.chunks(4) {
            assert!(q.iter().all(|v| area().expand(0.5).contains(v.pos)));
            if q[0].pos.y < 130.0 { near += 1 } else if q[0].pos.y > 170.0 { far += 1 }
        }
        assert!(near > far * 2, "near {near}, far {far}");
        // same pattern for the same strength; the bottom edge mirrors it
        assert_eq!(full.vertices.len(), dissolve_mesh(area(), Edge::Top, 1.0, 100.0, 6.0, Color32::BLACK, 1).vertices.len());
        let bottom = dissolve_mesh(area(), Edge::Bottom, 1.0, 100.0, 6.0, Color32::BLACK, 1);
        assert!(bottom.vertices.iter().all(|v| v.pos.y >= 300.0 - 0.5 && v.pos.y <= 400.0 + 0.5));
        // nonsense sizes give nothing instead of panicking
        assert_eq!(dissolve_mesh(Rect::NOTHING, Edge::Top, 1.0, 100.0, 6.0, Color32::BLACK, 1).vertices.len(), 0);
        assert_eq!(dissolve_mesh(area(), Edge::Top, 1.0, 2.0, 6.0, Color32::BLACK, 1).vertices.len(), 0);
    }

    #[test]
    fn cell_hash_is_stable_and_spread_out() {
        assert_eq!(hash_unit(3, 4, 5), hash_unit(3, 4, 5));
        assert_ne!(hash_unit(3, 4, 5), hash_unit(4, 3, 5));
        let below: usize = (0..1000).filter(|i| hash_unit(*i, 7, 1) < 0.5).count();
        assert!((400..600).contains(&below), "about half: {below}");
    }

    #[test]
    fn grains_thin_out_while_they_age() {
        assert_eq!(Sand::alpha(0.0, 1.0), 1.0);
        assert!(Sand::alpha(0.5, 1.0) < 0.5 + 1e-3 && Sand::alpha(0.5, 1.0) > 0.3);
        assert_eq!(Sand::alpha(1.0, 1.0), 0.0);
        assert_eq!(Sand::alpha(2.0, 1.0), 0.0);
    }
}
