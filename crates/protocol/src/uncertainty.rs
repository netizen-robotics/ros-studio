//! Shared fixed-axis pose covariance math. No serialized contract or ROS IO.
//! Variables are XYZ then rotations about fixed X/Y/Z, in the header frame.
mod serde;

/// Numeric input bound, not a statement of a sensor's physical accuracy.
pub const MAX_ABSOLUTE_COVARIANCE: f64 = 1e12;
const SYMMETRY_RELATIVE: f64 = 1e-9;
const SYMMETRY_ABSOLUTE: f64 = 1e-12;
// Dimensionless eigenvalue tolerance after per-variable diagonal equilibration.
const PSD_ROUNDOFF: f64 = 1e-12;
const EIGEN_RESIDUAL: f64 = 1e-14;

#[derive(Clone, Debug, PartialEq)]
pub struct Covariance {
    values: [f64; 36],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Marginal {
    Position,
    FixedAxisRotation,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrincipalAxes {
    /// Descending marginal variances, clamped only within PSD roundoff.
    pub variances: [f64; 3],
    /// Corresponding unit direction vectors, forming a right-handed basis.
    /// Signs and directions within a repeated eigenspace are not unique.
    pub directions: [[f64; 3]; 3],
}
impl Covariance {
    pub fn new(mut values: [f64; 36]) -> Result<Self, String> {
        if values
            .iter()
            .any(|v| !v.is_finite() || v.abs() > MAX_ABSOLUTE_COVARIANCE)
        {
            return Err("Covariance values must be finite and within the numeric bound".into());
        }
        for i in 0..6 {
            for j in i + 1..6 {
                let (a, b) = (values[i * 6 + j], values[j * 6 + i]);
                if (a - b).abs() > SYMMETRY_ABSOLUTE + SYMMETRY_RELATIVE * a.abs().max(b.abs()) {
                    return Err("Covariance must be symmetric".into());
                }
                values[i * 6 + j] = (a + b) * 0.5;
                values[j * 6 + i] = values[i * 6 + j];
            }
        }
        factor(&values)?;
        Ok(Self { values })
    }
    pub fn row_major(&self) -> &[f64; 36] {
        &self.values
    }
    pub fn marginal(&self, part: Marginal) -> [[f64; 3]; 3] {
        let offset = match part {
            Marginal::Position => 0,
            Marginal::FixedAxisRotation => 3,
        };
        std::array::from_fn(|i| std::array::from_fn(|j| self.values[(i + offset) * 6 + j + offset]))
    }
    pub fn principal_axes(&self, part: Marginal) -> Result<PrincipalAxes, String> {
        let (values, vectors, _) = diagonalize(self.marginal(part))?;
        let mut order = [0, 1, 2];
        order.sort_by(|a, b| values[*b].total_cmp(&values[*a]));
        let variances = order.map(|i| values[i].max(0.));
        let mut directions = order.map(|i| std::array::from_fn(|j| vectors[j][i]));
        let [a, b, c] = directions;
        let determinant = a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
            + a[2] * (b[0] * c[1] - b[1] * c[0]);
        if determinant < 0. {
            directions[2] = directions[2].map(|v| -v);
        }
        Ok(PrincipalAxes {
            variances,
            directions,
        })
    }
    /// Rotate fixed coordinate axes using TF's quaternion XYZW. Translation
    /// changes the pose origin, not this pose covariance's error coordinates.
    /// This is not a twist/wrench adjoint transform or a body-frame conversion.
    pub fn rotated(&self, quaternion: [f64; 4]) -> Result<Self, String> {
        let [x, y, z, w] = quaternion;
        if !crate::mapping::valid_pose(&[0., 0., 0., x, y, z, w]) {
            return Err("Covariance frame rotation requires a finite unit quaternion".into());
        }
        let r = [
            [
                1. - 2. * (y * y + z * z),
                2. * (x * y - z * w),
                2. * (x * z + y * w),
            ],
            [
                2. * (x * y + z * w),
                1. - 2. * (x * x + z * z),
                2. * (y * z - x * w),
            ],
            [
                2. * (x * z - y * w),
                2. * (y * z + x * w),
                1. - 2. * (x * x + y * y),
            ],
        ];
        // Rotate a PSD factor, then form its Gram matrix. Direct R C R^T
        // subtraction can create negative zero-axis variances for rank-one
        // inputs; sums of squared factor components preserve those cases.
        let source = factor(&self.values)?;
        let transformed: [[f64; 6]; 6] = std::array::from_fn(|i| {
            std::array::from_fn(|j| (0..3).map(|k| r[i % 3][k] * source[i / 3 * 3 + k][j]).sum())
        });
        let mut values = [0.; 36];
        for i in 0..6 {
            for j in 0..6 {
                values[i * 6 + j] = transformed[i]
                    .iter()
                    .zip(transformed[j])
                    .map(|(a, b)| a * b)
                    .sum();
            }
        }
        Self::new(values)
    }
}

fn factor(values: &[f64; 36]) -> Result<[[f64; 6]; 6], String> {
    let invalid =
        || "Covariance must be positive semidefinite, including cross correlations".to_string();
    let mut deviations = [0.; 6];
    for i in 0..6 {
        if values[i * 6 + i] < 0. {
            return Err(invalid());
        }
        deviations[i] = values[i * 6 + i].sqrt();
    }
    let mut correlation = [[0.; 6]; 6];
    for i in 0..6 {
        for j in 0..6 {
            let value = values[i * 6 + j];
            if deviations[i] == 0. || deviations[j] == 0. {
                if value != 0. {
                    return Err(invalid());
                }
            } else {
                let scaled = value / deviations[i] / deviations[j];
                if !scaled.is_finite() || scaled.abs() > 1. + PSD_ROUNDOFF {
                    return Err(invalid());
                }
                correlation[i][j] = scaled;
            }
        }
    }
    let (eigenvalues, vectors, _) = diagonalize(correlation)?;
    if eigenvalues.iter().any(|v| *v < -PSD_ROUNDOFF) {
        return Err(invalid());
    }
    Ok(std::array::from_fn(|i| {
        std::array::from_fn(|j| deviations[i] * vectors[i][j] * eigenvalues[j].max(0.).sqrt())
    }))
}

type EigenResult<const N: usize> = ([f64; N], [[f64; N]; N], f64);
/// Bounded maximum-pivot Jacobi solver. All matrices are tiny and symmetric;
/// scale normalization prevents arithmetic range overflow. Full PSD admission
/// uses a dimensionless correlation matrix so mixed units cannot hide errors.
fn diagonalize<const N: usize>(mut a: [[f64; N]; N]) -> Result<EigenResult<N>, String> {
    let scale = a.iter().flatten().fold(0f64, |m, v| m.max(v.abs()));
    let mut vectors =
        std::array::from_fn(|i| std::array::from_fn(|j| if i == j { 1. } else { 0. }));
    if scale == 0. {
        return Ok(([0.; N], vectors, 0.));
    }
    for value in a.iter_mut().flatten() {
        *value /= scale;
    }
    for _ in 0..64 * N * N {
        let (mut p, mut q, mut largest) = (0, 0, 0.);
        for (i, row) in a.iter().enumerate() {
            for (j, value) in row.iter().enumerate().skip(i + 1) {
                if value.abs() > largest {
                    (p, q, largest) = (i, j, value.abs());
                }
            }
        }
        if largest <= EIGEN_RESIDUAL {
            return Ok((std::array::from_fn(|i| a[i][i] * scale), vectors, scale));
        }
        let tau = (a[q][q] - a[p][p]) / (2. * a[p][q]);
        let t = tau.signum() / (tau.abs() + tau.hypot(1.));
        let c = 1. / (1. + t * t).sqrt();
        let s = t * c;
        a[p][p] -= t * a[p][q];
        a[q][q] += t * a[p][q];
        a[p][q] = 0.;
        a[q][p] = 0.;
        for k in 0..N {
            if k != p && k != q {
                let (kp, kq) = (a[k][p], a[k][q]);
                a[k][p] = c * kp - s * kq;
                a[p][k] = a[k][p];
                a[k][q] = s * kp + c * kq;
                a[q][k] = a[k][q];
            }
            let (vp, vq) = (vectors[k][p], vectors[k][q]);
            vectors[k][p] = c * vp - s * vq;
            vectors[k][q] = s * vp + c * vq;
        }
    }
    Err("Covariance eigensolver did not converge within its work bound".into())
}

#[cfg(test)]
mod tests;
