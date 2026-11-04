//! Tiny f64 vector / matrix helpers (kept local so there is no math-crate version coupling).

pub type V3 = [f64; 3];
/// Row-major 4x4: `m[row][col]`, column vectors (`p' = M * p`).
pub type M4 = [[f64; 4]; 4];

pub const IDENTITY: M4 = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

#[inline]
pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
#[inline]
pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[inline]
pub fn mul(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
#[inline]
pub fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
#[inline]
pub fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
#[inline]
pub fn len(a: V3) -> f64 {
    dot(a, a).sqrt()
}
/// Unit vector, or `a` unchanged when it has no length.
#[inline]
pub fn normalize(a: V3) -> V3 {
    let l = len(a);
    if l > 1e-12 {
        mul(a, 1.0 / l)
    } else {
        a
    }
}

pub fn mat_mul(a: &M4, b: &M4) -> M4 {
    let mut r = [[0.0; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            r[i][j] = (0..4).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    r
}

/// From a column-major array as glTF stores it (`m[col][row]`).
pub fn from_gltf(m: [[f32; 4]; 4]) -> M4 {
    let mut r = [[0.0; 4]; 4];
    for c in 0..4 {
        for row in 0..4 {
            r[row][c] = m[c][row] as f64;
        }
    }
    r
}

#[inline]
pub fn transform_point(m: &M4, p: V3) -> V3 {
    [
        m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2] + m[0][3],
        m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2] + m[1][3],
        m[2][0] * p[0] + m[2][1] * p[1] + m[2][2] * p[2] + m[2][3],
    ]
}

#[inline]
pub fn transform_dir(m: &M4, p: V3) -> V3 {
    [
        m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2],
        m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2],
        m[2][0] * p[0] + m[2][1] * p[1] + m[2][2] * p[2],
    ]
}

pub fn det3(m: &M4) -> f64 {
    let a = [m[0][0], m[0][1], m[0][2]];
    let b = [m[1][0], m[1][1], m[1][2]];
    let c = [m[2][0], m[2][1], m[2][2]];
    dot(a, cross(b, c))
}

/// Inverse transpose of the upper 3x3, as a 3x3 row-major (for transforming normals).
/// Falls back to the plain 3x3 when singular.
pub fn normal_matrix(m: &M4) -> [[f64; 3]; 3] {
    let r0 = [m[0][0], m[0][1], m[0][2]];
    let r1 = [m[1][0], m[1][1], m[1][2]];
    let r2 = [m[2][0], m[2][1], m[2][2]];
    let det = dot(r0, cross(r1, r2));
    if det.abs() < 1e-14 {
        return [r0, r1, r2];
    }
    // inverse transpose = cofactor matrix / det
    let c0 = cross(r1, r2);
    let c1 = cross(r2, r0);
    let c2 = cross(r0, r1);
    [mul(c0, 1.0 / det), mul(c1, 1.0 / det), mul(c2, 1.0 / det)]
}

#[inline]
pub fn mul3(m: &[[f64; 3]; 3], v: V3) -> V3 {
    [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
}

/// Python-style `round()` (ties to even) for parity with the original script.
#[inline]
pub fn round_py(x: f64) -> f64 {
    x.round_ties_even()
}
