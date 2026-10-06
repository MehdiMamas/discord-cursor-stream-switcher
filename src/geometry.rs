//! Rectangles, letterboxing and rotation math. Pure code, unit tested.

/// Integer rectangle in desktop pixels. `right` and `bottom` are exclusive.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self { left, top, right, bottom }
    }

    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }

    pub fn is_empty(&self) -> bool {
        self.width() <= 0 || self.height() <= 0
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }

    /// The point inside this rectangle closest to `(x, y)`.
    pub fn clamp(&self, x: i32, y: i32) -> (i32, i32) {
        (x.clamp(self.left, self.right - 1), y.clamp(self.top, self.bottom - 1))
    }
}

/// The point inside any of `rects` closest to `(x, y)`, or `None` if there are no usable rects.
pub fn nearest_point(rects: &[Rect], x: i32, y: i32) -> Option<(i32, i32)> {
    rects.iter().filter(|r| !r.is_empty()).map(|r| r.clamp(x, y)).min_by_key(|&(px, py)| {
        let dx = i64::from(px - x);
        let dy = i64::from(py - y);
        dx * dx + dy * dy
    })
}

/// Floating point rectangle in render-target pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RectF {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Largest rectangle with the aspect ratio of `src` that fits centered in `dst` (letterbox or
/// pillarbox). Edges land on whole pixels so the picture stays crisp.
pub fn fit(src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> RectF {
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return RectF::default();
    }
    let scale = (dst_w as f32 / src_w as f32).min(dst_h as f32 / src_h as f32);
    let w = (src_w as f32 * scale).round().min(dst_w as f32);
    let h = (src_h as f32 * scale).round().min(dst_h as f32);
    RectF { x: ((dst_w as f32 - w) / 2.0).floor(), y: ((dst_h as f32 - h) / 2.0).floor(), w, h }
}

/// Converts a pixel rectangle to normalized device coordinates: `[left, top, right, bottom]`.
pub fn to_ndc(r: RectF, target_w: u32, target_h: u32) -> [f32; 4] {
    let tw = target_w.max(1) as f32;
    let th = target_h.max(1) as f32;
    [r.x / tw * 2.0 - 1.0, 1.0 - r.y / th * 2.0, (r.x + r.w) / tw * 2.0 - 1.0, 1.0 - (r.y + r.h) / th * 2.0]
}

/// How a duplicated desktop image is rotated relative to the desktop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Rotation {
    #[default]
    Identity,
    Rotate90,
    Rotate180,
    Rotate270,
}

impl Rotation {
    /// Maps from DXGI_MODE_ROTATION values (1 = identity, 2 = 90, 3 = 180, 4 = 270).
    pub fn from_dxgi(value: i32) -> Self {
        match value {
            2 => Self::Rotate90,
            3 => Self::Rotate180,
            4 => Self::Rotate270,
            _ => Self::Identity,
        }
    }

    /// Affine map from output coordinates `(u, v)` in `[0, 1]` (top-left origin, desktop
    /// orientation) to texture coordinates of the unrotated duplicated image.
    /// `tex.x = a.0*u + a.1*v + a.2`, `tex.y = b.0*u + b.1*v + b.2`.
    pub fn uv_transform(self) -> ([f32; 3], [f32; 3]) {
        match self {
            Self::Identity => ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            Self::Rotate90 => ([0.0, 1.0, 0.0], [-1.0, 0.0, 1.0]),
            Self::Rotate180 => ([-1.0, 0.0, 1.0], [0.0, -1.0, 1.0]),
            Self::Rotate270 => ([0.0, -1.0, 1.0], [1.0, 0.0, 0.0]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_is_half_open() {
        let r = Rect::new(0, 0, 10, 10);
        assert!(r.contains(0, 0));
        assert!(r.contains(9, 9));
        assert!(!r.contains(10, 5));
        assert!(!r.contains(5, -1));
    }

    #[test]
    fn nearest_point_picks_closest_monitor_edge() {
        let left = Rect::new(-1920, 0, 0, 1080);
        let main = Rect::new(0, 0, 2560, 1440);
        // A point on a virtual display to the right of the main monitor.
        assert_eq!(nearest_point(&[left, main], 2600, 500), Some((2559, 500)));
        // A point below the left monitor but near it.
        assert_eq!(nearest_point(&[left, main], -500, 1200), Some((-500, 1079)));
        assert_eq!(nearest_point(&[], 1, 1), None);
        assert_eq!(nearest_point(&[Rect::default()], 1, 1), None);
    }

    #[test]
    fn fit_letterboxes_and_pillarboxes() {
        // 2560x1440 into 1920x1080: exact scale, no bars.
        assert_eq!(fit(2560, 1440, 1920, 1080), RectF { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0 });
        // 4:3 into 16:9: bars left and right.
        assert_eq!(fit(1600, 1200, 1920, 1080), RectF { x: 240.0, y: 0.0, w: 1440.0, h: 1080.0 });
        // Ultrawide into 16:9: bars top and bottom.
        assert_eq!(fit(3440, 1440, 1920, 1080), RectF { x: 0.0, y: 138.0, w: 1920.0, h: 804.0 });
        // Portrait monitor.
        assert_eq!(fit(1440, 2560, 1920, 1080), RectF { x: 656.0, y: 0.0, w: 608.0, h: 1080.0 });
        assert_eq!(fit(0, 10, 10, 10), RectF::default());
    }

    #[test]
    fn ndc_covers_full_target() {
        let r = RectF { x: 0.0, y: 0.0, w: 100.0, h: 50.0 };
        assert_eq!(to_ndc(r, 100, 50), [-1.0, 1.0, 1.0, -1.0]);
        let half = RectF { x: 50.0, y: 0.0, w: 50.0, h: 50.0 };
        assert_eq!(to_ndc(half, 100, 50), [0.0, 1.0, 1.0, -1.0]);
    }

    fn apply(rot: Rotation, u: f32, v: f32) -> (f32, f32) {
        let (a, b) = rot.uv_transform();
        (a[0] * u + a[1] * v + a[2], b[0] * u + b[1] * v + b[2])
    }

    #[test]
    fn rotations_match_dxgi_sample_corners() {
        // Values from the Microsoft DXGI desktop duplication sample's vertex setup.
        assert_eq!(apply(Rotation::Identity, 0.0, 0.0), (0.0, 0.0));
        assert_eq!(apply(Rotation::Rotate90, 0.0, 0.0), (0.0, 1.0));
        assert_eq!(apply(Rotation::Rotate90, 1.0, 0.0), (0.0, 0.0));
        assert_eq!(apply(Rotation::Rotate90, 0.0, 1.0), (1.0, 1.0));
        assert_eq!(apply(Rotation::Rotate180, 0.0, 0.0), (1.0, 1.0));
        assert_eq!(apply(Rotation::Rotate270, 0.0, 0.0), (1.0, 0.0));
        assert_eq!(apply(Rotation::Rotate270, 0.0, 1.0), (0.0, 0.0));
        assert_eq!(Rotation::from_dxgi(3), Rotation::Rotate180);
        assert_eq!(Rotation::from_dxgi(0), Rotation::Identity);
    }
}
