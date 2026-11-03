//! Worker-side billboard/ribbon/triangle projection into the same instance arena.
use super::*;
use crate::spatial::marker_geometry::ColoredPoint;
fn color(c: [f32; 4]) -> Result<(), String> {
    if c.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)) {
        Ok(())
    } else {
        Err("Invalid Marker corner color".into())
    }
}
fn record(
    columns: [[f32; 4]; 4],
    rgba: [f32; 4],
    eye: [f32; 4],
    ray: [f32; 4],
    tag: u32,
) -> ClipInstance {
    ClipInstance {
        columns,
        rgba,
        eye,
        ray,
        shape: [tag, 0, 0, 0],
    }
}
impl Frame {
    pub fn push_point(
        &mut self,
        p: ColoredPoint,
        dimensions: [f64; 2],
        camera: CameraPose,
        size: [u32; 2],
        deadline: Option<u64>,
    ) -> Result<(), String> {
        color(p.rgba)?;
        if dimensions
            .iter()
            .any(|v| !v.is_finite() || *v <= 0. || *v > 1_000_000.)
        {
            return Err("Invalid Marker point dimensions".into());
        }
        let projection = projection::Projection::new(camera, size)?;
        let x = camera.right.map(|v| v as f64 * dimensions[0]);
        let y = camera.up.map(|v| v as f64 * dimensions[1]);
        for (a, b) in [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
            let corner = Point3(std::array::from_fn(|i| {
                p.position.0[i] + x[i] * a + y[i] * b
            }));
            projection.point(corner)?;
        }
        let columns = [
            projection.delta(x, 0.)?,
            projection.delta(y, 0.)?,
            [0.; 4],
            projection.point(p.position)?,
        ];
        self.commit(
            Kind::Point,
            record(columns, p.rgba, [0.; 4], [0.; 4], 0),
            camera,
            projection.size,
            deadline,
        )
    }
    pub fn push_line(
        &mut self,
        a: ColoredPoint,
        b: ColoredPoint,
        width: f64,
        camera: CameraPose,
        size: [u32; 2],
        deadline: Option<u64>,
    ) -> Result<bool, String> {
        color(a.rgba)?;
        color(b.rgba)?;
        if !width.is_finite() || width <= 0. || width > 1_000_000. {
            return Err("Invalid Marker line width".into());
        }
        let projection = projection::Projection::new(camera, size)?;
        projection.point(a.position)?;
        projection.point(b.position)?;
        let along = std::array::from_fn::<_, 3, _>(|i| b.position.0[i] - a.position.0[i]);
        let center = Point3(std::array::from_fn(|i| {
            (a.position.0[i] + b.position.0[i]) * 0.5
        }));
        let view = if camera.orthographic_span.is_some() {
            camera.forward.map(f64::from)
        } else {
            std::array::from_fn(|i| center.0[i] - camera.origin()[i])
        };
        let normal = [
            along[1] * view[2] - along[2] * view[1],
            along[2] * view[0] - along[0] * view[2],
            along[0] * view[1] - along[1] * view[0],
        ];
        let norm = normal.iter().map(|v| v * v).sum::<f64>().sqrt();
        if norm == 0. {
            return Ok(false);
        }
        let across = normal.map(|v| v / norm * width);
        for p in [a.position, b.position] {
            for side in [-0.5, 0.5] {
                projection.point(Point3(std::array::from_fn(|i| p.0[i] + across[i] * side)))?;
            }
        }
        let columns = [
            projection.delta(along, 0.)?,
            projection.delta(across, 0.)?,
            [0.; 4],
            projection.point(center)?,
        ];
        self.commit(
            Kind::Line,
            record(columns, a.rgba, b.rgba, [0.; 4], 2),
            camera,
            projection.size,
            deadline,
        )?;
        Ok(true)
    }
    pub fn push_triangle(
        &mut self,
        points: [ColoredPoint; 3],
        camera: CameraPose,
        size: [u32; 2],
        deadline: Option<u64>,
    ) -> Result<(), String> {
        let projection = projection::Projection::new(camera, size)?;
        for p in points {
            color(p.rgba)?;
        }
        let mut columns = [[0.; 4]; 4];
        for i in 0..3 {
            columns[i] = projection.point(points[i].position)?;
        }
        columns[3] = std::array::from_fn(|i| {
            ((columns[0][i] as f64 + columns[1][i] as f64 + columns[2][i] as f64) / 3.) as f32
        });
        self.commit(
            Kind::Triangle,
            record(columns, points[0].rgba, points[1].rgba, points[2].rgba, 3),
            camera,
            projection.size,
            deadline,
        )
    }
}
