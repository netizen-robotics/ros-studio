//! Independent complete-byte XYZ contract tests; no native ROS data access.
use studio_protocol::{
    cloud::{Counters, Field, Metadata, Sample},
    image::HeaderStamp,
    spatial::TfOutcome,
};
fn metadata(big: bool) -> Metadata {
    Metadata {
        header_stamp: HeaderStamp {
            seconds: 12,
            nanoseconds: 34,
        },
        frame_id: "sensor_cloud".into(),
        width: 2,
        height: 2,
        fields: vec![
            Field {
                name: "y".into(),
                offset: 20,
                datatype: 8,
                count: 1,
            },
            Field {
                name: "intensity".into(),
                offset: 12,
                datatype: 7,
                count: 1,
            },
            Field {
                name: "z".into(),
                offset: 0,
                datatype: 8,
                count: 1,
            },
            Field {
                name: "x".into(),
                offset: 8,
                datatype: 7,
                count: 1,
            },
        ],
        is_bigendian: big,
        point_step: 32,
        row_step: 72,
        is_dense: true,
    }
}

#[test]
fn cloud_rejections_distinguish_frame_layout_and_admission_limits() {
    let mut value = metadata(false);
    value.frame_id = "invalid frame".into();
    assert!(value.raw_length().unwrap_err().contains("header.frame_id"));
    value = metadata(false);
    value.point_step = 0;
    let layout = value.raw_length().unwrap_err();
    assert!(layout.contains("point stride"));
    assert!(!layout.contains("frame"));
    value = metadata(false);
    value.width = 128;
    value.height = 128;
    value.row_step = 4096;
    let capacity = value.raw_length().unwrap_err();
    assert!(capacity.contains("16384 source points"));
    assert!(capacity.contains("8192"));
    assert!(capacity.contains("downsampled"));
}
const POINTS: [[f32; 3]; 4] = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [1., 1., 1.]];
fn raw(big: bool) -> Vec<u8> {
    let mut bytes = vec![0xa5; 144];
    for (i, p) in POINTS.iter().enumerate() {
        let at = (i / 2) * 72 + (i % 2) * 32;
        let x = if big {
            p[0].to_be_bytes()
        } else {
            p[0].to_le_bytes()
        };
        let y = if big {
            (p[1] as f64).to_be_bytes()
        } else {
            (p[1] as f64).to_le_bytes()
        };
        let z = if big {
            (p[2] as f64).to_be_bytes()
        } else {
            (p[2] as f64).to_le_bytes()
        };
        bytes[at + 8..at + 12].copy_from_slice(&x);
        bytes[at + 20..at + 28].copy_from_slice(&y);
        bytes[at..at + 8].copy_from_slice(&z);
    }
    bytes
}
#[test]
fn reordered_mixed_width_endian_and_organized_padding_decode_identically() {
    for big in [false, true] {
        let m = metadata(big);
        assert_eq!(m.raw_length().unwrap(), 144);
        let d = m.decode(&raw(big)).unwrap();
        assert_eq!(d.points, POINTS);
        assert_eq!(d.source_points, 4);
        assert_eq!(d.skipped_points, 0);
        assert_eq!(d.source_frame, "sensor_cloud");
        assert_eq!(d.acquisition_stamp_ns, 12_000_000_034);
    }
}
#[test]
fn nonfinite_and_out_of_range_points_are_skipped_without_identity_fallback() {
    let m = metadata(false);
    let mut data = raw(false);
    data[8..12].copy_from_slice(&f32::NAN.to_le_bytes());
    data[72 + 8..72 + 12].copy_from_slice(&1_000_001_f32.to_le_bytes());
    let d = m.decode(&data).unwrap();
    assert_eq!(d.source_points, 4);
    assert_eq!(d.skipped_points, 2);
    assert_eq!(d.points, vec![POINTS[1], POINTS[3]]);
}
#[test]
fn empty_cloud_is_valid_and_wrong_complete_byte_lengths_fail() {
    let mut m = metadata(false);
    m.width = 0;
    m.height = 1;
    m.row_step = 0;
    let d = m.decode(&[]).unwrap();
    assert!(d.points.is_empty());
    assert_eq!(d.source_points, 0);
    let m = metadata(false);
    assert!(m.decode(&raw(false)[..143]).is_err());
    let mut extra = raw(false);
    extra.push(0);
    assert!(m.decode(&extra).is_err());
}
#[test]
fn malformed_metadata_and_independent_budgets_are_rejected_before_copy() {
    for case in 0..13 {
        let mut m = metadata(false);
        match case {
            0 => m.fields[0].name = "x".into(),
            1 => m.fields[0].count = 2,
            2 => m.fields[0].datatype = 6,
            3 => m.fields[0].offset = 31,
            4 => m.fields[1].offset = 8,
            5 => m.row_step = 63,
            6 => m.point_step = 257,
            7 => m.width = 4097,
            8 => {
                m.width = 4096;
                m.height = 3;
                m.row_step = 131072;
            }
            9 => {
                m.width = 4096;
                m.height = 2;
                m.point_step = 33;
                m.row_step = 135168;
            }
            10 => m.fields[0].name = "y".repeat(65),
            11 => m.fields.extend((0..29).map(|i| Field {
                name: format!("extra{i}"),
                offset: 16,
                datatype: 2,
                count: 1,
            })),
            _ => m.fields[0].count = u32::MAX,
        }
        assert!(m.raw_length().is_err(), "metadata case{case} must reject");
    }
}

#[test]
fn ignored_uint8_array_count_is_limited_by_stride_not_an_unadvertised_count_cap() {
    for big in [false, true] {
        let mut m = metadata(big);
        m.point_step = 64;
        m.row_step = 136;
        m.fields.push(Field {
            name: "ignored_bytes".into(),
            offset: 32,
            datatype: 2,
            count: 32,
        });
        let original = raw(big);
        let mut bytes = vec![0xa5; 272];
        for i in 0..4 {
            let source = (i / 2) * 72 + (i % 2) * 32;
            let target = (i / 2) * 136 + (i % 2) * 64;
            bytes[target..target + 32].copy_from_slice(&original[source..source + 32]);
        }
        assert_eq!(m.raw_length().unwrap(), bytes.len());
        assert_eq!(m.decode(&bytes).unwrap().points, POINTS);
        m.fields.last_mut().unwrap().count = 33;
        assert!(
            m.raw_length().is_err(),
            "extra fields cannot exceed the stride"
        );
        let extra = m.fields.last_mut().unwrap();
        extra.datatype = 8;
        extra.offset = u32::MAX;
        extra.count = u32::MAX;
        assert!(
            m.raw_length().is_err(),
            "large field products and offsets must not wrap into a valid range"
        );
    }
}
#[test]
fn bounded_source_space_sample_roundtrips_with_exact_acquisition_tf_and_counts() {
    let d = metadata(false).decode(&raw(false)).unwrap();
    let s = Sample {
        contract_version: studio_protocol::cloud::CONTRACT_VERSION,
        topic: "/sensor_scene/cloud".into(),
        message_type: "sensor_msgs/msg/PointCloud2".into(),
        fixed_frame: "map".into(),
        source_frame: d.source_frame,
        acquisition_stamp_ns: d.acquisition_stamp_ns,
        source_points: d.source_points,
        skipped_points: d.skipped_points,
        points: d.points,
        channels: d.channels,
        tf: TfOutcome::Ready {
            components: [
                1.,
                2.,
                3.,
                0.,
                0.,
                std::f64::consts::FRAC_1_SQRT_2,
                std::f64::consts::FRAC_1_SQRT_2,
            ],
        },
        counters: Counters::default(),
    };
    s.validate().unwrap();
    let bytes = s.encode().unwrap();
    assert!(bytes.len() <= 512 * 1024);
    let decoded: Sample = serde_json::from_slice(&bytes).unwrap();
    decoded.validate().unwrap();
    assert_eq!(decoded.points, POINTS);
    let mut bad = s.clone();
    bad.source_points = 3;
    assert!(bad.validate().is_err());
    let mut bad = s.clone();
    bad.skipped_points = 1;
    assert!(bad.validate().is_err());
    let mut bad = s.clone();
    bad.points[0][0] = f32::NAN;
    assert!(bad.validate().is_err());
    let mut bad = s.clone();
    bad.acquisition_stamp_ns = 0;
    assert!(bad.validate().is_err());
    let mut upper_stamp = s.clone();
    upper_stamp.acquisition_stamp_ns = studio_protocol::cloud::MAX_STAMP_NS;
    upper_stamp.validate().unwrap();
    upper_stamp.acquisition_stamp_ns += 1;
    assert!(upper_stamp.validate().is_err());
    assert!(
        upper_stamp.encode().is_err(),
        "wire timestamps cannot exceed the native ROS header range"
    );
    let mut missing = s;
    missing.tf = TfOutcome::Missing;
    missing.validate().unwrap();
}
