use super::*;
fn diagonal(values: [f64; 6]) -> [f64; 36] {
    std::array::from_fn(|i| if i / 6 == i % 6 { values[i / 6] } else { 0. })
}
fn near(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() <= 1e-10 * expected.abs().max(1.),
        "{actual} != {expected}"
    );
}
fn check_axes(covariance: &Covariance, part: Marginal) {
    let axes = covariance.principal_axes(part).unwrap();
    let matrix = covariance.marginal(part);
    for (i, row) in matrix.iter().enumerate() {
        for (j, expected) in row.iter().enumerate() {
            let rebuilt: f64 = axes
                .variances
                .iter()
                .zip(axes.directions)
                .map(|(v, d)| v * d[i] * d[j])
                .sum();
            near(rebuilt, *expected);
        }
    }
    for (i, direction) in axes.directions.iter().enumerate() {
        near(direction.iter().map(|v| v * v).sum(), 1.);
        for other in &axes.directions[i + 1..] {
            near(direction.iter().zip(other).map(|(a, b)| a * b).sum(), 0.);
        }
    }
    let [a, b, c] = axes.directions;
    near(
        a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
            + a[2] * (b[0] * c[1] - b[1] * c[0]),
        1.,
    );
    assert!(axes.variances.windows(2).all(|p| p[0] >= p[1]));
}
#[test]
fn correlated_position_axes_reconstruct_and_rotate_in_the_header_frame() {
    let source = Covariance::new(diagonal([4., 1., 0., 0.01, 0.04, 0.09])).unwrap();
    let angle = std::f64::consts::FRAC_PI_4;
    let rotated = source
        .rotated([0., 0., (angle / 2.).sin(), (angle / 2.).cos()])
        .unwrap();
    let position = rotated.marginal(Marginal::Position);
    near(position[0][0], 2.5);
    near(position[1][1], 2.5);
    near(position[0][1], 1.5);
    let axes = rotated.principal_axes(Marginal::Position).unwrap();
    for (actual, expected) in axes.variances.into_iter().zip([4., 1., 0.]) {
        near(actual, expected);
    }
    check_axes(&rotated, Marginal::Position);
    check_axes(&rotated, Marginal::FixedAxisRotation);
    let restored = rotated
        .rotated([0., 0., -(angle / 2.).sin(), (angle / 2.).cos()])
        .unwrap();
    for (a, b) in restored.row_major().iter().zip(source.row_major()) {
        near(*a, *b);
    }
}
#[test]
fn full_joint_cross_correlations_are_validated_and_rotated() {
    // Both marginals are PSD, but the joint matrix has eigenvalue -1.
    let mut impossible = diagonal([1.; 6]);
    impossible[3] = 2.;
    impossible[18] = 2.;
    assert!(Covariance::new(impossible)
        .unwrap_err()
        .contains("positive semidefinite"));
    let v = [1., 2., 3., 0.1, 0.2, 0.3];
    let full = Covariance::new(std::array::from_fn(|i| v[i / 6] * v[i % 6])).unwrap();
    let angle = std::f64::consts::FRAC_PI_2;
    let rotated = full
        .rotated([0., 0., (angle / 2.).sin(), (angle / 2.).cos()])
        .unwrap();
    let expected = [-2., 1., 3., -0.2, 0.1, 0.3];
    for (i, value) in rotated.row_major().iter().enumerate() {
        near(*value, expected[i / 6] * expected[i % 6]);
    }
    check_axes(&rotated, Marginal::Position);
    check_axes(&rotated, Marginal::FixedAxisRotation);
}
#[test]
fn zero_planar_rank_deficient_and_repeated_eigenspaces_are_valid() {
    for values in [
        [0.; 6],
        [1., 1., 0., 0., 0., 0.1],
        [1.; 6],
        [1e12, 1e-12, 0., 0.01, 0.01, 0.01],
    ] {
        let covariance = Covariance::new(diagonal(values)).unwrap();
        check_axes(&covariance, Marginal::Position);
        check_axes(&covariance, Marginal::FixedAxisRotation);
    }
    let zero = Covariance::new([0.; 36]).unwrap();
    assert_eq!(zero.row_major(), &[0.; 36]);
    assert_eq!(
        zero.principal_axes(Marginal::Position).unwrap().variances,
        [0.; 3]
    );
}
#[test]
fn invalid_values_symmetry_negative_variance_and_frame_rotation_fail_explicitly() {
    for bad in [
        f64::NAN,
        f64::INFINITY,
        -f64::INFINITY,
        MAX_ABSOLUTE_COVARIANCE + 1.,
    ] {
        let mut values = diagonal([1.; 6]);
        values[2] = bad;
        assert!(Covariance::new(values).is_err());
    }
    let mut values = diagonal([1.; 6]);
    values[1] = 0.5;
    values[6] = 0.6;
    assert!(Covariance::new(values).unwrap_err().contains("symmetric"));
    assert!(Covariance::new(diagonal([1., 1., -1., 1., 1., 1.])).is_err());
    let valid = Covariance::new(diagonal([1.; 6])).unwrap();
    for q in [[0.; 4], [1.; 4], [0., 0., f64::NAN, 1.], [0., 0., 0., 1.01]] {
        assert!(valid.rotated(q).is_err());
    }
}
#[test]
fn declared_roundoff_tolerance_is_bounded_and_symmetrized() {
    let mut values = diagonal([1.; 6]);
    values[1] = 0.5;
    values[6] = 0.5 + 1e-12;
    let valid = Covariance::new(values).unwrap();
    near(valid.row_major()[1], valid.row_major()[6]);
    values[1] = 1. + 5e-13;
    values[6] = values[1];
    assert!(Covariance::new(values).is_ok());
    values[1] = 1. + 2e-12;
    values[6] = values[1];
    assert!(Covariance::new(values).is_err());
}

#[test]
fn mixed_units_cannot_hide_a_negative_marginal_or_invalid_cross_correlation() {
    assert!(Covariance::new(diagonal([1e12, 1., 1., -0.01, 1., 1.])).is_err());
    let mut values = diagonal([1e12, 1., 1., 1e-12, 1., 1.]);
    values[3] = 2.;
    values[18] = 2.;
    assert!(Covariance::new(values).is_err());
    values[3] = 0.5;
    values[18] = 0.5;
    assert!(Covariance::new(values).is_ok());
}

#[test]
fn factor_rotation_preserves_rank_one_cancellation_and_tiny_variances() {
    let v = [1., 1., 0., 1e-6, 1e-6, 0.];
    let source = Covariance::new(std::array::from_fn(|i| v[i / 6] * v[i % 6])).unwrap();
    let angle = std::f64::consts::FRAC_PI_4;
    let rotated = source
        .rotated([0., 0., (angle / 2.).sin(), (angle / 2.).cos()])
        .unwrap();
    assert!(rotated.row_major().iter().all(|v| v.is_finite()));
    near(rotated.row_major()[0], 0.);
    near(rotated.row_major()[7], 2.);
    check_axes(&rotated, Marginal::Position);
    check_axes(&rotated, Marginal::FixedAxisRotation);
    let mut impossible = [0.; 36];
    impossible[1] = 1e-20;
    impossible[6] = 1e-20;
    assert!(Covariance::new(impossible).is_err());
}

#[test]
fn dense_joint_covariance_rotates_all_axes_and_preserves_marginal_reconstruction() {
    let l = [
        [2., 0., 0., 0., 0., 0.],
        [1., 3., 0., 0., 0., 0.],
        [-1., 0.5, 1., 0., 0., 0.],
        [0.1, 0.2, 0.3, 0.4, 0., 0.],
        [-0.1, 0.3, 0.2, 0.1, 0.5, 0.],
        [0.2, -0.1, 0.1, 0.2, 0.1, 0.6],
    ];
    let matrix = std::array::from_fn(|i| l[i / 6].iter().zip(l[i % 6]).map(|(a, b)| a * b).sum());
    let source = Covariance::new(matrix).unwrap();
    let q = [0.2, -0.3, 0.1, 0.86f64.sqrt()];
    let rotated = source.rotated(q).unwrap();
    let mut transformed = [[0.; 6]; 6];
    for col in 0..6 {
        for block in 0..2 {
            let v = crate::mapping::transform_point(
                &[0., 0., 0., q[0], q[1], q[2], q[3]],
                std::array::from_fn(|row| l[block * 3 + row][col]),
            );
            for row in 0..3 {
                transformed[block * 3 + row][col] = v[row];
            }
        }
    }
    for (i, actual) in rotated.row_major().iter().enumerate() {
        near(
            *actual,
            transformed[i / 6]
                .iter()
                .zip(transformed[i % 6])
                .map(|(a, b)| a * b)
                .sum(),
        );
    }
    check_axes(&rotated, Marginal::Position);
    check_axes(&rotated, Marginal::FixedAxisRotation);
    let restored = rotated.rotated([-q[0], -q[1], -q[2], q[3]]).unwrap();
    for (a, b) in restored.row_major().iter().zip(source.row_major()) {
        near(*a, *b);
    }
}
