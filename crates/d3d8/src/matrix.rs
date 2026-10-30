/// WW3D Matrix3D: 3 rows of (x axis, y axis, z axis, translation).
pub type Matrix = [f32; 12];

/// Blends two transforms: translation and scale linearly, rotation as a normalized quaternion lerp.
/// Returns `b` when either one is mirrored or degenerate.
pub fn blend(a: &Matrix, b: &Matrix, t: f32) -> Matrix {
    let (Some((ra, sa)), Some((rb, sb))) = (split(a), split(b)) else { return *b };
    let qa = to_quat(&ra);
    let mut qb = to_quat(&rb);
    if dot(&qa, &qb) < 0.0 {
        qb = qb.map(|v| -v);
    }
    let q = normalize([0, 1, 2, 3].map(|i| lerp(qa[i], qb[i], t)));
    let r = from_quat(&q);
    let mut m = [0.0; 12];
    for row in 0..3 {
        for col in 0..3 {
            m[row * 4 + col] = r[row * 3 + col] * lerp(sa[col], sb[col], t);
        }
        m[row * 4 + 3] = lerp(a[row * 4 + 3], b[row * 4 + 3], t);
    }
    m
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Splits the 3x3 part into a rotation (row major) and a scale per axis.
fn split(m: &Matrix) -> Option<([f32; 9], [f32; 3])> {
    let mut r = [0.0; 9];
    let mut s = [0.0; 3];
    for col in 0..3 {
        let len = (0..3).map(|row| m[row * 4 + col].powi(2)).sum::<f32>().sqrt();
        if len < 1e-6 {
            return None;
        }
        s[col] = len;
        for row in 0..3 {
            r[row * 3 + col] = m[row * 4 + col] / len;
        }
    }
    let det = r[0] * (r[4] * r[8] - r[5] * r[7]) - r[1] * (r[3] * r[8] - r[5] * r[6])
        + r[2] * (r[3] * r[7] - r[4] * r[6]);
    (det > 0.0).then_some((r, s))
}

/// Quaternion as (x, y, z, w).
fn to_quat(r: &[f32; 9]) -> [f32; 4] {
    let at = |row: usize, col: usize| r[row * 3 + col];
    let (r00, r11, r22) = (at(0, 0), at(1, 1), at(2, 2));
    let trace = r00 + r11 + r22;
    let q = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        [(at(2, 1) - at(1, 2)) / s, (at(0, 2) - at(2, 0)) / s, (at(1, 0) - at(0, 1)) / s, 0.25 * s]
    } else if r00 > r11 && r00 > r22 {
        let s = (1.0 + r00 - r11 - r22).sqrt() * 2.0;
        [0.25 * s, (at(0, 1) + at(1, 0)) / s, (at(0, 2) + at(2, 0)) / s, (at(2, 1) - at(1, 2)) / s]
    } else if r11 > r22 {
        let s = (1.0 + r11 - r00 - r22).sqrt() * 2.0;
        [(at(0, 1) + at(1, 0)) / s, 0.25 * s, (at(1, 2) + at(2, 1)) / s, (at(0, 2) - at(2, 0)) / s]
    } else {
        let s = (1.0 + r22 - r00 - r11).sqrt() * 2.0;
        [(at(0, 2) + at(2, 0)) / s, (at(1, 2) + at(2, 1)) / s, 0.25 * s, (at(1, 0) - at(0, 1)) / s]
    };
    normalize(q)
}

/// Rotation, row major.
fn from_quat(q: &[f32; 4]) -> [f32; 9] {
    let [x, y, z, w] = *q;
    [
        1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - z * w), 2.0 * (x * z + y * w),
        2.0 * (x * y + z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - x * w),
        2.0 * (x * z - y * w), 2.0 * (y * z + x * w), 1.0 - 2.0 * (x * x + y * y),
    ]
}

fn dot(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn normalize(q: [f32; 4]) -> [f32; 4] {
    let len = dot(&q, &q).sqrt();
    q.map(|v| v / len)
}
