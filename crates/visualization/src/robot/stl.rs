//! Strict bounded STL decoding; normals are validated but geometry owns winding.
use super::model::*;
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) fn cancelled(cancel: &AtomicBool) -> Result<(), RobotError> {
    if cancel.load(Ordering::Acquire) {
        Err(RobotError::new(
            RobotErrorKind::Cancelled,
            "Robot load cancelled",
        ))
    } else {
        Ok(())
    }
}
fn invalid(message: &str) -> RobotError {
    RobotError::new(RobotErrorKind::Asset, message)
}
fn triangle(points: [[f64; 3]; 3]) -> Result<Triangle, RobotError> {
    if points
        .iter()
        .flatten()
        .any(|v| !v.is_finite() || v.abs() > MAX_COORDINATE)
    {
        return Err(invalid(
            "STL vertices must be finite and within coordinate bounds",
        ));
    }
    let a = std::array::from_fn::<_, 3, _>(|i| points[1][i] - points[0][i]);
    let b = std::array::from_fn::<_, 3, _>(|i| points[2][i] - points[0][i]);
    let cross = [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ];
    if !cross.iter().any(|v| *v != 0.0) {
        return Err(invalid("Degenerate STL triangle"));
    }
    Ok(Triangle(points))
}
pub(super) fn decode(bytes: &[u8], cancel: &AtomicBool) -> Result<Vec<Triangle>, RobotError> {
    cancelled(cancel)?;
    if bytes.len() > MAX_MESH_BYTES {
        return Err(RobotError::new(
            RobotErrorKind::Limit,
            "STL byte limit exceeded",
        ));
    }
    // The binary header is arbitrary, including headers beginning with `solid`.
    if bytes.len() >= 84 {
        let count = u32::from_le_bytes(bytes[80..84].try_into().unwrap()) as usize;
        if count.checked_mul(50).and_then(|n| n.checked_add(84)) == Some(bytes.len()) {
            if count == 0 || count > MAX_ROBOT_TRIANGLES {
                return Err(RobotError::new(
                    RobotErrorKind::Limit,
                    "STL triangle count outside bounds",
                ));
            }
            let mut out = Vec::with_capacity(count);
            for record in bytes[84..].chunks_exact(50) {
                cancelled(cancel)?;
                let mut values = [0.0; 12];
                for (v, raw) in values.iter_mut().zip(record[..48].chunks_exact(4)) {
                    *v = f32::from_le_bytes(raw.try_into().unwrap()) as f64;
                    if !v.is_finite() {
                        return Err(invalid("Nonfinite binary STL normal or vertex"));
                    }
                }
                out.push(triangle(std::array::from_fn(|i| {
                    std::array::from_fn(|j| values[3 + i * 3 + j])
                }))?);
            }
            return Ok(out);
        }
    }
    let source = std::str::from_utf8(bytes)
        .map_err(|_| invalid("Invalid binary STL length or non-UTF-8 ASCII STL"))?;
    let source = source.trim();
    let (header, body) = source
        .split_once('\n')
        .ok_or_else(|| invalid("ASCII STL requires solid and endsolid lines"))?;
    if header.split_whitespace().next() != Some("solid") {
        return Err(invalid("Unsupported STL header"));
    }
    let (body, footer) = body
        .rsplit_once('\n')
        .ok_or_else(|| invalid("ASCII STL missing endsolid"))?;
    if footer.split_whitespace().next() != Some("endsolid") {
        return Err(invalid("ASCII STL missing endsolid"));
    }
    let mut tokens = body.split_whitespace();
    fn expect<'a>(
        tokens: &mut impl Iterator<Item = &'a str>,
        expected: &str,
    ) -> Result<(), RobotError> {
        if tokens.next() == Some(expected) {
            Ok(())
        } else {
            Err(invalid("Malformed ASCII STL structure"))
        }
    }
    fn number<'a>(tokens: &mut impl Iterator<Item = &'a str>) -> Result<f64, RobotError> {
        tokens
            .next()
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite())
            .ok_or_else(|| invalid("Invalid ASCII STL number"))
    }
    let mut out = Vec::new();
    while let Some(token) = tokens.next() {
        cancelled(cancel)?;
        if token != "facet" {
            return Err(invalid("Unexpected ASCII STL content"));
        }
        if out.len() >= MAX_ROBOT_TRIANGLES {
            return Err(RobotError::new(
                RobotErrorKind::Limit,
                "STL triangle limit exceeded",
            ));
        }
        expect(&mut tokens, "normal")?;
        for _ in 0..3 {
            number(&mut tokens)?;
        }
        expect(&mut tokens, "outer")?;
        expect(&mut tokens, "loop")?;
        let mut points = [[0.0; 3]; 3];
        for p in &mut points {
            expect(&mut tokens, "vertex")?;
            for v in p {
                *v = number(&mut tokens)?;
            }
        }
        expect(&mut tokens, "endloop")?;
        expect(&mut tokens, "endfacet")?;
        out.push(triangle(points)?);
    }
    if out.is_empty() {
        return Err(invalid("Empty STL mesh"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn binary_solid_header_is_not_misclassified() {
        let mut bytes = vec![0; 84];
        bytes[..5].copy_from_slice(b"solid");
        bytes[80..84].copy_from_slice(&1u32.to_le_bytes());
        for v in [0f32, 0., 1., 0., 0., 0., 1., 0., 0., 0., 1., 0.] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        bytes.extend_from_slice(&[0, 0]);
        assert_eq!(decode(&bytes, &AtomicBool::new(false)).unwrap().len(), 1);
        bytes[84 + 12..84 + 48].fill(0);
        assert!(decode(&bytes, &AtomicBool::new(false)).is_err());
    }
    #[test]
    fn strict_ascii_and_cancel() {
        let source=b"solid sample\nfacet normal 0 0 1 outer loop vertex 0 0 0 vertex 1 0 0 vertex 0 1 0 endloop endfacet\nendsolid sample";
        assert_eq!(decode(source, &AtomicBool::new(false)).unwrap().len(), 1);
        assert_eq!(
            decode(source, &AtomicBool::new(true)).unwrap_err().kind,
            RobotErrorKind::Cancelled
        );
        assert!(decode(
            b"solid sample\nfacet normal NaN 0 1\nendsolid",
            &AtomicBool::new(false)
        )
        .is_err());
    }
}
