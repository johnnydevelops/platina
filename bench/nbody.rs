// simplified n-body (f64 heavy)
#[derive(Clone, Copy)]
struct Body { x: f64, y: f64, z: f64, vx: f64, vy: f64, vz: f64, m: f64 }

pub fn bench(steps: u32) -> u32 {
    let mut bodies = [
        Body { x: 0.0, y: 0.0, z: 0.0, vx: 0.0, vy: 0.0, vz: 0.0, m: 39.47 },
        Body { x: 4.84, y: -1.16, z: -0.10, vx: 0.606, vy: 2.81, vz: -0.02, m: 0.037 },
        Body { x: 8.34, y: 4.12, z: -0.40, vx: -1.01, vy: 1.82, vz: 0.008, m: 0.011 },
        Body { x: 12.89, y: -15.11, z: -0.22, vx: 1.08, vy: 0.868, vz: -0.01, m: 0.0017 },
        Body { x: 15.37, y: -25.91, z: 0.179, vx: 0.979, vy: 0.594, vz: -0.034, m: 0.002 },
    ];
    let dt = 0.01;
    for _ in 0..steps {
        for i in 0..5 {
            for j in (i + 1)..5 {
                let dx = bodies[i].x - bodies[j].x;
                let dy = bodies[i].y - bodies[j].y;
                let dz = bodies[i].z - bodies[j].z;
                let d2 = dx * dx + dy * dy + dz * dz;
                let mag = dt / (d2 * unsafe { core::intrinsics::sqrtf64(d2) });
                let mj = bodies[j].m;
                let mi = bodies[i].m;
                bodies[i].vx -= dx * mj * mag;
                bodies[i].vy -= dy * mj * mag;
                bodies[i].vz -= dz * mj * mag;
                bodies[j].vx += dx * mi * mag;
                bodies[j].vy += dy * mi * mag;
                bodies[j].vz += dz * mi * mag;
            }
        }
        for b in bodies.iter_mut() {
            b.x += dt * b.vx;
            b.y += dt * b.vy;
            b.z += dt * b.vz;
        }
    }
    let mut e = 0.0;
    for b in &bodies { e += b.m * (b.vx * b.vx + b.vy * b.vy + b.vz * b.vz); }
    (e * 1e6) as u32
}
