//! Independent byte-layout and strict cloud2 transport regressions.
use studio_protocol::{
    cloud::{self, Channel, Channels, Counters, Field, Metadata, Sample},
    image::HeaderStamp,
    spatial::TfOutcome,
};

fn metadata(big: bool, fields: Vec<Field>) -> Metadata {
    let mut xyz: Vec<_> = ["x", "y", "z"]
        .into_iter()
        .enumerate()
        .map(|(i, name)| Field {
            name: name.into(),
            offset: i as u32 * 4,
            datatype: 7,
            count: 1,
        })
        .collect();
    xyz.extend(fields);
    Metadata {
        header_stamp: HeaderStamp {
            seconds: 1,
            nanoseconds: 2,
        },
        frame_id: "sensor".into(),
        width: 2,
        height: 2,
        fields: xyz,
        is_bigendian: big,
        point_step: 32,
        row_step: 72,
        is_dense: false,
    }
}
fn field(name: &str, offset: u32, datatype: u8, count: u32) -> Field {
    Field {
        name: name.into(),
        offset,
        datatype,
        count,
    }
}
fn bytes(big: bool) -> Vec<u8> {
    let mut raw = vec![0xa5; 144];
    for i in 0..4 {
        let at = i / 2 * 72 + i % 2 * 32;
        for (axis, value) in [i as f32, 2., 3.].into_iter().enumerate() {
            raw[at + axis * 4..at + axis * 4 + 4].copy_from_slice(&if big {
                value.to_be_bytes()
            } else {
                value.to_le_bytes()
            });
        }
    }
    raw
}
fn sample() -> Sample {
    Sample {
        contract_version: cloud::CONTRACT_VERSION,
        topic: "/cloud".into(),
        message_type: cloud::MESSAGE_TYPE.into(),
        fixed_frame: "map".into(),
        source_frame: "sensor".into(),
        acquisition_stamp_ns: 1,
        source_points: 1,
        skipped_points: 0,
        points: vec![[1., 2., 3.]],
        channels: Channels::default(),
        tf: TfOutcome::Ready {
            components: [0., 0., 0., 0., 0., 0., 1.],
        },
        counters: Counters::default(),
    }
}

#[test]
fn packed_bits_and_intensity_follow_filtered_xyz_across_endian_and_padding() {
    for big in [false, true] {
        for datatype in [6, 7] {
            for name in ["rgb", "rgba"] {
                let metadata = metadata(
                    big,
                    vec![field(name, 12, datatype, 1), field("intensity", 16, 8, 1)],
                );
                let mut raw = bytes(big);
                // This packed FLOAT32 bit pattern is NaN, but its color is valid.
                let colors = [0xffc10203u32, 0x00112233, 0x80445566, 0x40778899];
                for (i, bits) in colors.into_iter().enumerate() {
                    let at = i / 2 * 72 + i % 2 * 32;
                    raw[at + 12..at + 16].copy_from_slice(&if big {
                        bits.to_be_bytes()
                    } else {
                        bits.to_le_bytes()
                    });
                    let n = [1., f64::NAN, f64::INFINITY, -42.][i];
                    raw[at + 16..at + 24].copy_from_slice(&if big {
                        n.to_be_bytes()
                    } else {
                        n.to_le_bytes()
                    });
                }
                // Drop point1 only; all attributes must drop exactly the same slot.
                raw[32..36].copy_from_slice(&if big {
                    f32::NAN.to_be_bytes()
                } else {
                    f32::NAN.to_le_bytes()
                });
                let decoded = metadata.decode(&raw).unwrap();
                assert_eq!(
                    decoded.points.iter().map(|p| p[0]).collect::<Vec<_>>(),
                    [0., 2., 3.]
                );
                assert_eq!(decoded.skipped_points, 1);
                let alpha = |a| if name == "rgba" { a } else { 255 };
                assert_eq!(
                    decoded.channels.color,
                    Channel::Values(vec![
                        [193, 2, 3, alpha(255)],
                        [68, 85, 102, alpha(128)],
                        [119, 136, 153, alpha(64)]
                    ])
                );
                assert_eq!(
                    decoded.channels.intensity,
                    Channel::Values(vec![Some(1.), None, Some(-42.)])
                );
            }
        }
    }
}

#[test]
fn scalar_intensities_support_all_standard_types_without_precision_truncation() {
    for big in [false, true] {
        macro_rules! scalar {
            ($value:expr) => {
                if big {
                    $value.to_be_bytes().to_vec()
                } else {
                    $value.to_le_bytes().to_vec()
                }
            };
        }
        let cases = [
            (1, vec![(-12i8) as u8], -12.),
            (2, vec![250], 250.),
            (3, scalar!(-1234i16), -1234.),
            (4, scalar!(65000u16), 65000.),
            (5, scalar!(i32::MIN), f64::from(i32::MIN)),
            (6, scalar!(u32::MAX), f64::from(u32::MAX)),
            (7, scalar!(12.5f32), 12.5),
            (8, scalar!(f64::MAX), f64::MAX),
        ];
        for (kind, value, expected) in cases {
            let m = metadata(big, vec![field("intensity", 16, kind, 1)]);
            let mut raw = bytes(big);
            for i in 0..4 {
                let at = i / 2 * 72 + i % 2 * 32 + 16;
                raw[at..at + value.len()].copy_from_slice(&value);
            }
            assert_eq!(
                m.decode(&raw).unwrap().channels.intensity,
                Channel::Values(vec![Some(expected); 4])
            );
        }
    }
}

#[test]
fn layout_availability_and_precedence_survive_empty_clouds() {
    let missing = metadata(false, vec![]).decode(&bytes(false)).unwrap();
    assert_eq!(missing.channels, Channels::default());
    let mut m = metadata(
        false,
        vec![field("rgba", 12, 2, 4), field("intensity", 16, 7, 2)],
    );
    let decoded = m.decode(&bytes(false)).unwrap();
    assert_eq!(
        decoded.channels,
        Channels {
            color: Channel::Unsupported,
            intensity: Channel::Unsupported
        }
    );
    m.fields.push(field("rgb", 24, 6, 1));
    assert!(matches!(
        m.decode(&bytes(false)).unwrap().channels.color,
        Channel::Values(_)
    ));
    m.fields
        .retain(|f| !matches!(f.name.as_str(), "rgba" | "intensity"));
    m.fields.push(field("rgba", 12, 6, 1));
    let mut raw = bytes(false);
    raw[12..16].copy_from_slice(&0x80445566u32.to_le_bytes());
    raw[24..28].copy_from_slice(&0x00112233u32.to_le_bytes());
    assert!(
        matches!(m.decode(&raw).unwrap().channels.color,Channel::Values(v) if v[0]==[68,85,102,128])
    );
    m.width = 0;
    m.height = 1;
    m.row_step = 0;
    assert_eq!(
        m.decode(&[]).unwrap().channels.color,
        Channel::Values(vec![])
    );
}

#[test]
fn strict_wire_rejects_old_clouds_invalid_counts_xyz_and_attribute_alignment() {
    let s = sample();
    let wire = serde_json::to_value(&s).unwrap();
    assert_eq!(wire["points"]["data_base64"], "AACAPwAAAEAAAEBA");
    for bad_points in [
        serde_json::json!([[1, 2, 3]]),
        serde_json::json!({"count":8193,"data_base64":""}),
        serde_json::json!({"count":1,"data_base64":""}),
        serde_json::json!({"count":0,"data_base64":"","extra":1}),
        // Positive infinity followed by two zeros.
        serde_json::json!({"count":1,"data_base64":"AACAfwAAAAAAAAAA"}),
    ] {
        let mut bad = wire.clone();
        bad["points"] = bad_points;
        assert!(serde_json::from_value::<Sample>(bad).is_err());
    }
    let mut old = s.clone();
    old.contract_version = 1;
    assert!(old.validate().is_err());
    let mut bad = s.clone();
    bad.channels.color = Channel::Values(vec![]);
    assert!(bad.validate().is_err());
    bad = s.clone();
    bad.channels.intensity = Channel::Values(vec![Some(f64::INFINITY)]);
    assert!(bad.validate().is_err());
    for invalid in [
        serde_json::json!({"color":{"status":"missing","values":[[1,2,3,4]]},"intensity":{"status":"missing"}}),
        serde_json::json!({"color":{"status":"values","values":[[256,0,0,0]]},"intensity":{"status":"missing"}}),
        serde_json::json!({"color":{"status":"missing"},"intensity":{"status":"missing"},"extra":1}),
    ] {
        assert!(serde_json::from_value::<Channels>(invalid).is_err());
    }
}

#[test]
fn full_attributed_cloud_stays_inside_unchanged_sample_and_event_budgets() {
    let mut s = sample();
    s.source_points = 8192;
    s.points = vec![[-999999., -999999., -999999.]; 8192];
    s.channels = Channels {
        color: Channel::Values(vec![[255; 4]; 8192]),
        intensity: Channel::Values(vec![Some(-1.2345678901234567e-200); 8192]),
    };
    let encoded = s.encode().unwrap();
    assert!(encoded.len() < cloud::MAX_SAMPLE_BYTES);
    let decoded: Sample = serde_json::from_slice(&encoded).unwrap();
    decoded.validate().unwrap();
    assert_eq!(decoded, s);
    let event = studio_protocol::session::Event::PointCloudObservation {
        generation: studio_protocol::subscriptions::Generation(1),
        subscription: studio_protocol::subscriptions::SubscriptionId(1),
        sequence: 1,
        sample: Box::new(s),
        backend_drops: 0,
    };
    let wire = event.encode().unwrap();
    assert!(wire.len() < studio_protocol::MAX_LINE);
    let decoded = studio_protocol::session::Event::decode(&wire).unwrap();
    assert_eq!(decoded.encode().unwrap(), wire);
}
