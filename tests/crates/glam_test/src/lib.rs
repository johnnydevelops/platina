use glam::{Mat4, Quat, Vec3};

pub fn run() {
    let a = Vec3::new(1.0, 2.0, 3.0);
    let b = Vec3::new(-2.0, 0.5, 4.0);
    println!("{:.4} {:.4?} {:.4}", a.dot(b), a.cross(b), (a - b).length());
    let q = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
    println!("{:.4?}", q * Vec3::X);
    let m = Mat4::from_scale_rotation_translation(Vec3::splat(2.0), q, Vec3::new(1.0, 0.0, -1.0));
    println!("{:.4?}", m.transform_point3(Vec3::ONE));
    println!("{:.4?}", m.inverse().transform_point3(m.transform_point3(a)));
}
