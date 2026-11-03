#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point3(pub [f64; 3]);
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RigidTransform {
    translation: [f64; 3],
    rotation: [f64; 4], // x,y,z,w, unit quaternion, right-handed ROS axes.
}
impl RigidTransform {
    pub fn new(translation: [f64; 3], rotation: [f64; 4]) -> Option<Self> {
        let norm = rotation.iter().map(|v| v * v).sum::<f64>();
        (translation
            .iter()
            .chain(rotation.iter())
            .all(|v| v.is_finite())
            && (norm - 1.0).abs() <= 1e-6)
            .then(|| Self {
                translation,
                rotation: Self::normalize(rotation),
            })
    }
    fn normalize(rotation: [f64; 4]) -> [f64; 4] {
        let norm = rotation.iter().map(|v| v * v).sum::<f64>().sqrt();
        rotation.map(|v| v / norm)
    }
    pub fn identity() -> Self {
        Self {
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
        }
    }
    fn rotate(&self, point: Point3) -> Point3 {
        let [x, y, z, w] = self.rotation;
        let [px, py, pz] = point.0;
        let t = [
            2.0 * (y * pz - z * py),
            2.0 * (z * px - x * pz),
            2.0 * (x * py - y * px),
        ];
        Point3([
            px + w * t[0] + y * t[2] - z * t[1],
            py + w * t[1] + z * t[0] - x * t[2],
            pz + w * t[2] + x * t[1] - y * t[0],
        ])
    }
    pub fn apply(&self, point: Point3) -> Point3 {
        let mut p = self.rotate(point).0;
        for (v, t) in p.iter_mut().zip(self.translation) {
            *v += t;
        }
        Point3(p)
    }
    /// `self` after `rhs`; compose target<-middle and middle<-source.
    pub fn compose(&self, rhs: Self) -> Self {
        let [x, y, z, w] = self.rotation;
        let [a, b, c, d] = rhs.rotation;
        Self {
            translation: self.apply(Point3(rhs.translation)).0,
            rotation: Self::normalize([
                w * a + x * d + y * c - z * b,
                w * b - x * c + y * d + z * a,
                w * c + x * b - y * a + z * d,
                w * d - x * a - y * b - z * c,
            ]),
        }
    }
}
