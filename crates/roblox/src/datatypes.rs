//! Roblox datatypes as plain Rust values. Math is done in Rust; values cross into Luau as
//! their components.

use core::ops::{Add, Div, Mul, Neg, Sub};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Vector3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vector3 {
    pub const ZERO: Vector3 = Vector3::new(0.0, 0.0, 0.0);
    pub const ONE: Vector3 = Vector3::new(1.0, 1.0, 1.0);
    pub const X_AXIS: Vector3 = Vector3::new(1.0, 0.0, 0.0);
    pub const Y_AXIS: Vector3 = Vector3::new(0.0, 1.0, 0.0);
    pub const Z_AXIS: Vector3 = Vector3::new(0.0, 0.0, 1.0);
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Vector3 { x, y, z }
    }
    pub fn dot(self, o: Self) -> f64 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    pub fn cross(self, o: Self) -> Self {
        Vector3::new(self.y * o.z - self.z * o.y, self.z * o.x - self.x * o.z, self.x * o.y - self.y * o.x)
    }
    pub fn magnitude(self) -> f64 {
        libm_sqrt(self.dot(self))
    }
    pub fn unit(self) -> Self {
        let m = self.magnitude();
        if m == 0.0 { self } else { self / m }
    }
    pub fn lerp(self, goal: Self, a: f64) -> Self {
        self + (goal - self) * a
    }
}

impl Add for Vector3 {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Vector3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl Sub for Vector3 {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Vector3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl Mul<f64> for Vector3 {
    type Output = Self;
    fn mul(self, s: f64) -> Self {
        Vector3::new(self.x * s, self.y * s, self.z * s)
    }
}
impl Mul for Vector3 {
    type Output = Self;
    fn mul(self, o: Self) -> Self {
        Vector3::new(self.x * o.x, self.y * o.y, self.z * o.z)
    }
}
impl Div<f64> for Vector3 {
    type Output = Self;
    fn div(self, s: f64) -> Self {
        Vector3::new(self.x / s, self.y / s, self.z / s)
    }
}
impl Neg for Vector3 {
    type Output = Self;
    fn neg(self) -> Self {
        Vector3::new(-self.x, -self.y, -self.z)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Vector2 {
    pub x: f64,
    pub y: f64,
}

impl Vector2 {
    pub const ZERO: Vector2 = Vector2::new(0.0, 0.0);
    pub const ONE: Vector2 = Vector2::new(1.0, 1.0);
    pub const fn new(x: f64, y: f64) -> Self {
        Vector2 { x, y }
    }
    pub fn magnitude(self) -> f64 {
        libm_sqrt(self.x * self.x + self.y * self.y)
    }
}
impl Add for Vector2 {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Vector2::new(self.x + o.x, self.y + o.y)
    }
}
impl Sub for Vector2 {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Vector2::new(self.x - o.x, self.y - o.y)
    }
}
impl Mul<f64> for Vector2 {
    type Output = Self;
    fn mul(self, s: f64) -> Self {
        Vector2::new(self.x * s, self.y * s)
    }
}

/// A coordinate frame: position plus a rotation matrix (rows `r0`, `r1`, `r2`), laid out like
/// `CFrame:GetComponents()`.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct CFrame {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub r00: f64,
    pub r01: f64,
    pub r02: f64,
    pub r10: f64,
    pub r11: f64,
    pub r12: f64,
    pub r20: f64,
    pub r21: f64,
    pub r22: f64,
}

impl Default for CFrame {
    fn default() -> Self {
        CFrame::IDENTITY
    }
}

impl CFrame {
    pub const IDENTITY: CFrame = CFrame::new(0.0, 0.0, 0.0);
    /// A translation.
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        CFrame { x, y, z, r00: 1.0, r01: 0.0, r02: 0.0, r10: 0.0, r11: 1.0, r12: 0.0, r20: 0.0, r21: 0.0, r22: 1.0 }
    }
    pub fn from_position(p: Vector3) -> Self {
        CFrame::new(p.x, p.y, p.z)
    }
    pub fn position(&self) -> Vector3 {
        Vector3::new(self.x, self.y, self.z)
    }
    pub fn right_vector(&self) -> Vector3 {
        Vector3::new(self.r00, self.r10, self.r20)
    }
    pub fn up_vector(&self) -> Vector3 {
        Vector3::new(self.r01, self.r11, self.r21)
    }
    pub fn look_vector(&self) -> Vector3 {
        Vector3::new(-self.r02, -self.r12, -self.r22)
    }
    /// Rotation from Euler angles in radians, applied in Z, Y, X order (`CFrame.Angles`).
    pub fn angles(rx: f64, ry: f64, rz: f64) -> Self {
        let (sx, cx) = (libm_sin(rx), libm_cos(rx));
        let (sy, cy) = (libm_sin(ry), libm_cos(ry));
        let (sz, cz) = (libm_sin(rz), libm_cos(rz));
        CFrame {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            r00: cy * cz,
            r01: -cy * sz,
            r02: sy,
            r10: cx * sz + sx * sy * cz,
            r11: cx * cz - sx * sy * sz,
            r12: -sx * cy,
            r20: sx * sz - cx * sy * cz,
            r21: sx * cz + cx * sy * sz,
            r22: cx * cy,
        }
    }
    /// A frame at `at` facing `target` (`CFrame.lookAt`, up = +Y).
    pub fn look_at(at: Vector3, target: Vector3) -> Self {
        let look = (target - at).unit();
        let mut right = look.cross(Vector3::Y_AXIS);
        if right.magnitude() < 1e-9 {
            right = Vector3::X_AXIS;
        }
        let right = right.unit();
        let up = right.cross(look);
        CFrame {
            x: at.x,
            y: at.y,
            z: at.z,
            r00: right.x,
            r01: up.x,
            r02: -look.x,
            r10: right.y,
            r11: up.y,
            r12: -look.y,
            r20: right.z,
            r21: up.z,
            r22: -look.z,
        }
    }
    pub fn inverse(&self) -> Self {
        // rotation is orthonormal: transpose it, and rotate the negated position
        let t = CFrame {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            r00: self.r00,
            r01: self.r10,
            r02: self.r20,
            r10: self.r01,
            r11: self.r11,
            r12: self.r21,
            r20: self.r02,
            r21: self.r12,
            r22: self.r22,
        };
        let p = t.rotate(-self.position());
        CFrame { x: p.x, y: p.y, z: p.z, ..t }
    }
    fn rotate(&self, v: Vector3) -> Vector3 {
        Vector3::new(
            self.r00 * v.x + self.r01 * v.y + self.r02 * v.z,
            self.r10 * v.x + self.r11 * v.y + self.r12 * v.z,
            self.r20 * v.x + self.r21 * v.y + self.r22 * v.z,
        )
    }
    /// Transform a point from this frame's space to world space.
    pub fn point_to_world_space(&self, v: Vector3) -> Vector3 {
        self.rotate(v) + self.position()
    }
}

impl Mul for CFrame {
    type Output = CFrame;
    fn mul(self, o: CFrame) -> CFrame {
        let c = |r: [f64; 3], col: [f64; 3]| r[0] * col[0] + r[1] * col[1] + r[2] * col[2];
        let (a0, a1, a2) = ([self.r00, self.r01, self.r02], [self.r10, self.r11, self.r12], [self.r20, self.r21, self.r22]);
        let (b0, b1, b2) = ([o.r00, o.r10, o.r20], [o.r01, o.r11, o.r21], [o.r02, o.r12, o.r22]);
        let p = self.point_to_world_space(o.position());
        CFrame {
            x: p.x,
            y: p.y,
            z: p.z,
            r00: c(a0, b0),
            r01: c(a0, b1),
            r02: c(a0, b2),
            r10: c(a1, b0),
            r11: c(a1, b1),
            r12: c(a1, b2),
            r20: c(a2, b0),
            r21: c(a2, b1),
            r22: c(a2, b2),
        }
    }
}
impl Mul<Vector3> for CFrame {
    type Output = Vector3;
    fn mul(self, v: Vector3) -> Vector3 {
        self.point_to_world_space(v)
    }
}
impl Add<Vector3> for CFrame {
    type Output = CFrame;
    fn add(self, v: Vector3) -> CFrame {
        CFrame { x: self.x + v.x, y: self.y + v.y, z: self.z + v.z, ..self }
    }
}

/// An RGB color with components in 0..=1.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Color3 {
    pub r: f64,
    pub g: f64,
    pub b: f64,
}

impl Color3 {
    pub const fn new(r: f64, g: f64, b: f64) -> Self {
        Color3 { r, g, b }
    }
    pub fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        Color3::new(r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0)
    }
    /// From hue, saturation, value in 0..=1.
    pub fn from_hsv(h: f64, s: f64, v: f64) -> Self {
        let h6 = (h - libm_floor(h)) * 6.0;
        let i = libm_floor(h6);
        let f = h6 - i;
        let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
        let (r, g, b) = match i as i32 {
            0 => (v, t, p),
            1 => (q, v, p),
            2 => (p, v, t),
            3 => (p, q, v),
            4 => (t, p, v),
            _ => (v, p, q),
        };
        Color3::new(r, g, b)
    }
    pub fn lerp(self, goal: Self, a: f64) -> Self {
        Color3::new(self.r + (goal.r - self.r) * a, self.g + (goal.g - self.g) * a, self.b + (goal.b - self.b) * a)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct UDim {
    pub scale: f64,
    pub offset: f64,
}

impl UDim {
    pub const fn new(scale: f64, offset: f64) -> Self {
        UDim { scale, offset }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct UDim2 {
    pub x: UDim,
    pub y: UDim,
}

impl UDim2 {
    pub const fn new(x_scale: f64, x_offset: f64, y_scale: f64, y_offset: f64) -> Self {
        UDim2 { x: UDim::new(x_scale, x_offset), y: UDim::new(y_scale, y_offset) }
    }
    pub const fn from_scale(x: f64, y: f64) -> Self {
        UDim2::new(x, 0.0, y, 0.0)
    }
    pub const fn from_offset(x: f64, y: f64) -> Self {
        UDim2::new(0.0, x, 0.0, y)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Rect {
    pub min: Vector2,
    pub max: Vector2,
}

impl Rect {
    pub const fn new(min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> Self {
        Rect { min: Vector2::new(min_x, min_y), max: Vector2::new(max_x, max_y) }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct NumberRange {
    pub min: f64,
    pub max: f64,
}

impl NumberRange {
    pub const fn new(min: f64, max: f64) -> Self {
        NumberRange { min, max }
    }
}

/// A BrickColor, by number (`BrickColor.new(n).Number`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct BrickColor(pub u32);

// Float math through Luau's `math` library, so no libm is linked (this crate is `no_std`).
mod m {
    crate::rt::luau_fn! {
        pub fn sqrt(x: f64) -> f64 = "math.sqrt";
        pub fn sin(x: f64) -> f64 = "math.sin";
        pub fn cos(x: f64) -> f64 = "math.cos";
        pub fn floor(x: f64) -> f64 = "math.floor";
    }
}
fn libm_sqrt(x: f64) -> f64 {
    unsafe { m::sqrt(x) }
}
fn libm_sin(x: f64) -> f64 {
    unsafe { m::sin(x) }
}
fn libm_cos(x: f64) -> f64 {
    unsafe { m::cos(x) }
}
fn libm_floor(x: f64) -> f64 {
    unsafe { m::floor(x) }
}
