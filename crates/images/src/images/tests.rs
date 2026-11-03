use super::*;
#[test]
fn metric_depth_handles_both_byte_orders_padding_and_invalid_values() {
    for big_endian in [false, true] {
        for encoding in ["16UC1", "32FC1"] {
            let mut data = Vec::new();
            for meters in [0., 1., 5., 10.] {
                if encoding == "16UC1" {
                    let value = (meters * 1000.) as u16;
                    data.extend(if big_endian {
                        value.to_be_bytes()
                    } else {
                        value.to_le_bytes()
                    });
                } else {
                    let value = meters as f32;
                    data.extend(if big_endian {
                        value.to_be_bytes()
                    } else {
                        value.to_le_bytes()
                    });
                }
            }
            data.extend([123, 123]);
            let decoded = decode(RawFrame {
                width: 4,
                height: 1,
                encoding,
                step: data.len() as u32,
                big_endian,
                data: &data,
            })
            .unwrap();
            let expected = [
                [0, 0, 0, 0],
                [230, 230, 230, 255],
                [128, 128, 128, 255],
                [0, 0, 0, 255],
            ];
            assert_eq!(
                decoded
                    .pixels()
                    .iter()
                    .map(|p| p.to_array())
                    .collect::<Vec<_>>(),
                expected
            );
        }
    }
    let data = [f32::NAN, f32::INFINITY, -1., 0., 20.]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect::<Vec<_>>();
    let decoded = decode(RawFrame {
        width: 5,
        height: 1,
        encoding: "32FC1",
        step: 20,
        big_endian: false,
        data: &data,
    })
    .unwrap();
    assert!(decoded.pixels()[..4].iter().all(|p| p.to_array() == [0; 4]));
    assert_eq!(decoded.pixels()[4].to_array(), [0, 0, 0, 255]);
}
use eframe::egui;

fn frame(
    encoding: &str,
    width: u32,
    height: u32,
    step: u32,
    data: &[u8],
) -> Result<DecodedFrame, DecodeError> {
    decode(RawFrame {
        width,
        height,
        encoding,
        step,
        big_endian: false,
        data,
    })
}
fn solid(value: u8) -> DecodedFrame {
    frame("mono8", 1, 1, 1, &[value]).unwrap()
}
#[test]
fn colors_padding_orientation_and_endian() {
    for (encoding, bytes, expected) in [
        ("mono8", vec![91], [91, 91, 91, 255]),
        ("rgb8", vec![255, 64, 0], [255, 64, 0, 255]),
        ("bgr8", vec![0, 64, 255], [255, 64, 0, 255]),
        ("rgba8", vec![255, 64, 0, 255], [255, 64, 0, 255]),
        ("bgra8", vec![0, 64, 255, 255], [255, 64, 0, 255]),
    ] {
        for big_endian in [false, true] {
            let decoded = decode(RawFrame {
                width: 1,
                height: 1,
                step: bytes.len() as u32,
                encoding,
                big_endian,
                data: &bytes,
            })
            .unwrap();
            assert_eq!(decoded.pixels()[0].to_array(), expected);
        }
    }
    let decoded = frame("mono8", 3, 2, 4, &[1, 2, 3, 99, 4, 5, 6, 99]).unwrap();
    assert_eq!(decoded.size(), [3, 2]);
    assert_eq!(
        decoded.pixels().iter().map(|p| p.r()).collect::<Vec<_>>(),
        [1, 2, 3, 4, 5, 6]
    );
    let alpha = frame("rgba8", 1, 1, 4, &[255, 0, 0, 128]).unwrap();
    assert_eq!(alpha.pixels()[0].to_srgba_unmultiplied(), [255, 0, 0, 128]);
}
#[test]
fn rejects_invalid_and_incomplete_without_guessing() {
    assert!(matches!(
        frame("16SC1", 1, 1, 2, &[0, 0]),
        Err(DecodeError::UnsupportedEncoding)
    ));
    for (width, height) in [(0, 1), (1, 0), (u32::MAX, 1), (2049, 1)] {
        assert!(matches!(
            frame("rgb8", width, height, 3, &[0; 3]),
            Err(DecodeError::InvalidDimensions)
        ));
    }
    assert!(matches!(
        frame("rgb8", 2, 1, 5, &[0; 5]),
        Err(DecodeError::InvalidStride)
    ));
    for len in [2, 4] {
        assert!(matches!(
            frame("rgb8", 1, 1, 3, &vec![0; len]),
            Err(DecodeError::PayloadLength { .. })
        ));
    }
    assert!(matches!(
        frame("mono8", 1, 2048, u32::MAX, &[]),
        Err(DecodeError::TooLarge)
    ));
    assert!(matches!(
        frame("rgba8", 2048, 2048, 8193, &[]),
        Err(DecodeError::TooLarge)
    ));
}
#[test]
fn flood_pause_stop_and_stale_worker_are_bounded() {
    let mailbox = LatestFrames::default();
    let first = mailbox.start();
    for n in 0..1000 {
        assert!(mailbox.publish(first, solid((n % 255) as u8)));
    }
    assert_eq!(mailbox.counters().superseded, 999);
    assert_eq!(mailbox.take().unwrap().pixels()[0].r(), 234);
    assert!(mailbox.take().is_none());
    mailbox.set_paused(true);
    assert!(!mailbox.publish(first, solid(8)));
    assert_eq!(mailbox.counters().paused_skips, 1);
    mailbox.set_paused(false);
    mailbox.publish(first, solid(9));
    mailbox.stop();
    assert!(mailbox.take().is_none());
    assert!(!mailbox.publish(first, solid(10)));
    let second = mailbox.start();
    assert!(!mailbox.publish(first, solid(11)));
    assert!(mailbox.publish(second, solid(12)));
    assert_eq!(mailbox.take().unwrap().pixels()[0].r(), 12);
}
#[test]
fn headless_texture_pause_format_change_and_stop() {
    let ctx = egui::Context::default();
    let mut view = ImageView::default();
    let generation = view.start();
    let mailbox = view.frames();
    let draw = |view: &mut ImageView| {
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| view.show(ui));
        });
    };
    draw(&mut view);
    assert!(!view.has_displayed_frame());
    mailbox.publish(generation, solid(23));
    draw(&mut view);
    assert!(view.has_displayed_frame());
    view.set_paused(true);
    assert!(view.is_paused());
    assert!(!mailbox.publish(generation, solid(45)));
    draw(&mut view);
    assert!(view.has_displayed_frame());
    view.set_paused(false);
    mailbox.publish(generation, frame("rgb8", 3, 1, 9, &[0; 9]).unwrap());
    view.set_display_mode(DisplayMode::ActualSize);
    draw(&mut view);
    view.fail("Invalid frame");
    assert!(!view.has_displayed_frame());
    draw(&mut view);
    view.start();
    assert!(!view.has_displayed_frame());
    view.stop();
    assert!(!mailbox.publish(generation, solid(1)));
}
#[test]
fn budget_boundary_and_view_drop_release_pending_frames() {
    let data = vec![127; MAX_FRAME_BYTES];
    let decoded = frame("rgba8", 2048, 2048, 8192, &data).unwrap();
    assert_eq!(decoded.pixels().len(), 4_194_304);
    assert!(matches!(
        frame("rgba8", 2048, 2048, 8193, &[]),
        Err(DecodeError::TooLarge)
    ));
    let mut view = ImageView::default();
    let generation = view.start();
    let mailbox = view.frames();
    mailbox.publish(generation, decoded);
    drop(view);
    assert!(mailbox.take().is_none());
    assert!(!mailbox.publish(generation, solid(1)));
}
