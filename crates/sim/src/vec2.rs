use serde::{Deserialize, Serialize};
use std::ops::{Add, AddAssign, Mul, Neg, Sub, SubAssign};

/// Plain 2-D vector. Coordinates are world units, `y` grows downwards
/// (towards gravity), exactly like the canvas in the browser.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

impl Vec2 {
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };

    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
    pub fn dot(self, o: Vec2) -> f64 {
        self.x * o.x + self.y * o.y
    }
    /// 2-D cross product (z component).
    pub fn cross(self, o: Vec2) -> f64 {
        self.x * o.y - self.y * o.x
    }
    pub fn length(self) -> f64 {
        self.dot(self).sqrt()
    }
    pub fn length_sq(self) -> f64 {
        self.dot(self)
    }
    pub fn normalized(self) -> Vec2 {
        let l = self.length();
        if l > 0.0 {
            self * (1.0 / l)
        } else {
            Vec2::ZERO
        }
    }
    pub fn distance(self, o: Vec2) -> f64 {
        (self - o).length()
    }
    /// Clamp the length of the vector to `max`.
    pub fn clamp_length(self, max: f64) -> Vec2 {
        let l = self.length();
        if l > max && l > 0.0 {
            self * (max / l)
        } else {
            self
        }
    }
}

impl Add for Vec2 {
    type Output = Vec2;
    fn add(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x + o.x, self.y + o.y)
    }
}
impl AddAssign for Vec2 {
    fn add_assign(&mut self, o: Vec2) {
        self.x += o.x;
        self.y += o.y;
    }
}
impl Sub for Vec2 {
    type Output = Vec2;
    fn sub(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x - o.x, self.y - o.y)
    }
}
impl SubAssign for Vec2 {
    fn sub_assign(&mut self, o: Vec2) {
        self.x -= o.x;
        self.y -= o.y;
    }
}
impl Mul<f64> for Vec2 {
    type Output = Vec2;
    fn mul(self, s: f64) -> Vec2 {
        Vec2::new(self.x * s, self.y * s)
    }
}
impl Neg for Vec2 {
    type Output = Vec2;
    fn neg(self) -> Vec2 {
        Vec2::new(-self.x, -self.y)
    }
}

/// Shortest distance between point `p` and segment `a`-`b`.
pub fn point_segment_distance(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let ab = b - a;
    let len_sq = ab.length_sq();
    if len_sq == 0.0 {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Shortest distance between segments `a1`-`a2` and `b1`-`b2`.
/// Returns 0 when they intersect.
pub fn segment_segment_distance(a1: Vec2, a2: Vec2, b1: Vec2, b2: Vec2) -> f64 {
    if segments_intersect(a1, a2, b1, b2) {
        return 0.0;
    }
    point_segment_distance(a1, b1, b2)
        .min(point_segment_distance(a2, b1, b2))
        .min(point_segment_distance(b1, a1, a2))
        .min(point_segment_distance(b2, a1, a2))
}

fn orient(a: Vec2, b: Vec2, c: Vec2) -> f64 {
    (b - a).cross(c - a)
}

fn on_segment(a: Vec2, b: Vec2, p: Vec2) -> bool {
    p.x >= a.x.min(b.x) && p.x <= a.x.max(b.x) && p.y >= a.y.min(b.y) && p.y <= a.y.max(b.y)
}

/// Proper or touching intersection test for two segments.
pub fn segments_intersect(a1: Vec2, a2: Vec2, b1: Vec2, b2: Vec2) -> bool {
    let d1 = orient(b1, b2, a1);
    let d2 = orient(b1, b2, a2);
    let d3 = orient(a1, a2, b1);
    let d4 = orient(a1, a2, b2);
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    {
        return true;
    }
    (d1 == 0.0 && on_segment(b1, b2, a1))
        || (d2 == 0.0 && on_segment(b1, b2, a2))
        || (d3 == 0.0 && on_segment(a1, a2, b1))
        || (d4 == 0.0 && on_segment(a1, a2, b2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_distances() {
        let a = Vec2::new(0.0, 0.0);
        let b = Vec2::new(10.0, 0.0);
        assert!((point_segment_distance(Vec2::new(5.0, 3.0), a, b) - 3.0).abs() < 1e-12);
        assert!((point_segment_distance(Vec2::new(-4.0, 3.0), a, b) - 5.0).abs() < 1e-12);
        assert!(segments_intersect(
            a,
            b,
            Vec2::new(5.0, -1.0),
            Vec2::new(5.0, 1.0)
        ));
        assert!(!segments_intersect(
            a,
            b,
            Vec2::new(5.0, 1.0),
            Vec2::new(5.0, 2.0)
        ));
        assert!(
            (segment_segment_distance(a, b, Vec2::new(5.0, 1.0), Vec2::new(5.0, 2.0)) - 1.0).abs()
                < 1e-12
        );
    }
}
