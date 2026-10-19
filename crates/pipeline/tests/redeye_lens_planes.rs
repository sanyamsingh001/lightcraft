//! Synthetic per-plane lens sampling regressions. These use supported coefficients, not camera data.
use std::sync::Arc;

use lightcraft_develop::{DevelopSettings, EmbeddedLens, EmbeddedWarp, RedEye};
use lightcraft_geom::{Orientation, Point, Rect};
use lightcraft_pipeline::{OutputDepth, PixelWindow, RenderRequest, SourceInfo, StageCache, frame_for, plan, redeye, render, render_cached};
use lightcraft_raster::Rgb32f;

const DEPTHS: [OutputDepth; 3] = [OutputDepth::U8, OutputDepth::U16, OutputDepth::F32Linear];
const ORIENTATIONS: [Orientation; 8] = [
    Orientation::Normal,
    Orientation::Rotate90,
    Orientation::Rotate180,
    Orientation::Rotate270,
    Orientation::FlipH,
    Orientation::Transverse,
    Orientation::FlipV,
    Orientation::Transpose,
];

fn pupil_source() -> Arc<Rgb32f> {
    Arc::new(Rgb32f::from_fn(320, 160, |x, y| if (x as f64 + 0.5 - 60.0).hypot(y as f64 + 0.5 - 80.0) < 4.0 { [0.8, 0.02, 0.02] } else { [0.08; 3] }))
}

fn lens_info(scales: Option<[f64; 3]>) -> SourceInfo {
    SourceInfo {
        lens: scales.map(|scales| EmbeddedLens {
            warp: Some(EmbeddedWarp { planes: scales.map(|s| [s, 0.0, 0.0, 0.0, 0.0, 0.0]), center: Point::new(0.5, 0.5), radius: 1.0 }),
            vignette: None,
        }),
        ..Default::default()
    }
}

/// Measure the displayed strong-red pupil, independently of the detection grid. The lower-red
/// colour fringe at the original G/B location is deliberately excluded.
fn displayed_pupil(img: &Rgb32f) -> Point {
    let (mut x, mut y, mut total) = (0.0, 0.0, 0.0);
    for py in 0..img.height {
        for px in 0..img.width {
            let c = img.get(px, py);
            let red = (c[0] - c[1].max(c[2])) / (c[0] + c[1] + c[2] + 1e-6);
            if c[0] > 0.3 && red > 0.35 {
                let w = red as f64;
                x += (px as f64 + 0.5) * w;
                y += (py as f64 + 0.5) * w;
                total += w;
            }
        }
    }
    assert!(total > 2.0, "the fixture must display a strong-red pupil");
    Point::new(x / total, y / total)
}

fn check_correction(scales: Option<[f64; 3]>, orientation: Orientation, manual: bool) {
    let src = pupil_source();
    let info = lens_info(scales);
    let mut s = DevelopSettings { orientation, ..Default::default() };
    s.optics.lens_profile = scales.is_some();
    if manual {
        s.geometry.horizontal = -12.0;
        s.geometry.vertical = 17.0;
        s.geometry.rotate = 7.0;
        s.geometry.aspect = 10.0;
        s.geometry.scale = 108.0;
        s.crop.geometry.rect = Rect::new(0.1, 0.1, 0.9, 0.9);
        s.crop.geometry.angle = 11.0;
        s.crop.flip_h = true;
        s.crop.flip_v = true;
    }
    let req = RenderRequest::fit(320, 320);
    let full_frame = frame_for(&src, &info, &s, true);
    let (w, h) = full_frame.fit(req.max_w, req.max_h);
    let displayed = displayed_pupil(&full_frame.sample(&src, w, h));
    let center = full_frame.out_to_norm(w, h).apply(displayed);
    let long = full_frame.ow.max(full_frame.oh);
    let uncorrected = s.clone();
    s.red_eye.push(RedEye {
        center: Point::new(center.x + 16.0 / full_frame.ow, center.y),
        rx: 26.0 / long,
        ry: 12.0 / long,
        pupil_size: 50.0,
        darken: 50.0,
        pet: false,
        catchlight: None,
    });
    let stored = s.clone();
    for depth in DEPTHS {
        let req = RenderRequest { depth, ..req };
        let cache = StageCache::default();
        let before = render_cached(&src, &info, &uncorrected, &req, &cache);
        let p = plan(&src, &info, &s, &req);
        let pupil = p.eyes[0];
        assert!(
            Point::new(pupil.cx as f64, pupil.cy as f64).dist(displayed) < 1.0,
            "{scales:?} {orientation:?} {manual} {depth:?}: {pupil:?} vs {displayed:?}"
        );
        let after = render(&src, &info, &s, &req);
        let cached = render_cached(&src, &info, &s, &req, &cache);
        assert_eq!(after.image, cached.image);
        assert_eq!(after.deep, cached.deep);
        let (x, y) = (displayed.x.floor() as usize, displayed.y.floor() as usize);
        let corrected = after.image.get(x, y);
        assert_ne!(corrected, before.image.get(x, y), "the displayed pupil must be changed");
        assert!(corrected[0].abs_diff(corrected[1]) <= 3 && corrected[0].abs_diff(corrected[2]) <= 3, "neutral corrected pupil: {corrected:?}");
        let whole = PixelWindow { x: 0, y: 0, w, h };
        let whole_output = render(&src, &info, &s, &RenderRequest { window: Some(whole), ..req });
        assert_eq!(after.image, whole_output.image);
        assert_eq!(after.deep, whole_output.deep);
        if depth == OutputDepth::U8 {
            let win = PixelWindow { x: x.saturating_sub(12), y: y.saturating_sub(12), w: 25, h: 25 }.clamped(w, h);
            let tile_req = RenderRequest { window: Some(win), ..req };
            let tile = render(&src, &info, &s, &tile_req);
            let cached_tile = render_cached(&src, &info, &s, &tile_req, &cache);
            assert_eq!(tile.image, cached_tile.image);
            let expected = after.image.crop(win.x, win.y, win.w, win.h);
            assert_eq!((tile.image.width, tile.image.height), (expected.width, expected.height));
            assert!(
                tile.image.data.iter().zip(&expected.data).all(|(a, b)| a.iter().zip(b).all(|(a, b)| a.abs_diff(*b) <= 1)),
                "proper-window quantization exceeds one U8 step"
            );
        }
        assert_eq!(s, stored, "resolving pupils must not change stored settings");
    }
}

#[test]
fn per_plane_lens_detection_corrects_the_displayed_pupil_with_orientation_geometry_and_windows() {
    for orientation in ORIENTATIONS {
        for manual in [false, true] {
            check_correction(Some([2.0, 1.0, 1.0]), orientation, manual);
        }
    }
}

#[test]
fn no_lens_and_common_plane_controls_keep_detecting_and_correcting_the_pupil() {
    for scales in [None, Some([1.0; 3]), Some([2.0; 3])] {
        for orientation in [Orientation::Normal, Orientation::Rotate90] {
            for manual in [false, true] {
                check_correction(scales, orientation, manual);
            }
        }
    }
}

#[test]
fn a_red_plane_on_the_source_cannot_bypass_green_blank_canvas_coverage() {
    let src =
        Rgb32f::from_fn(320, 160, |x, y| if (x as f64 + 0.5 - 288.0).hypot(y as f64 + 0.5 - 80.0) < 4.0 { [0.8, 0.02, 0.02] } else { [0.08; 3] });
    let info = lens_info(Some([1.0, 2.0, 1.0]));
    let mut s = DevelopSettings::default();
    s.optics.lens_profile = true;
    s.red_eye.push(RedEye { center: Point::new(0.9, 0.5), rx: 0.03, ry: 0.03, pupil_size: 50.0, ..Default::default() });
    let req = RenderRequest::fit(320, 160);
    let p = plan(&src, &info, &s, &req);
    let k = p.eyes[0];
    assert!((k.cx - 288.0).abs() < 1e-5 && (k.cy - 80.0).abs() < 1e-5);
    assert!((k.r - 4.8).abs() < 1e-5, "blank samples must use the fallback, not the red plane");
    assert_eq!(p.frame.sample(&src, p.w, p.h).get(288, 80), lightcraft_pipeline::geometry::BLANK);
}

#[test]
fn invalid_sample_inputs_return_finite_fallbacks_without_indexing_a_malformed_buffer() {
    let eye = RedEye { center: Point::new(0.5, 0.5), rx: 0.1, ry: 0.1, ..Default::default() };
    for src in
        [Rgb32f::new(0, 0), Rgb32f { width: 3, height: 2, data: vec![[0.8, 0.02, 0.02]] }, Rgb32f { width: usize::MAX, height: 2, data: vec![] }]
    {
        let pupil = redeye::detect(&src, Orientation::Normal, 320.0, 160.0, &eye);
        assert!(!pupil.found && pupil.center == eye.center && pupil.radius.is_finite());
    }
    let src = pupil_source();
    for dimensions in [(0.0, 160.0), (320.0, f64::NAN), (f64::INFINITY, 160.0)] {
        assert!(!redeye::detect(&src, Orientation::Normal, dimensions.0, dimensions.1, &eye).found);
    }
    for scale in [f64::NAN, f64::INFINITY, -f64::INFINITY, f64::MAX] {
        let info = lens_info(Some([scale, 1.0, 1.0]));
        let mut s = DevelopSettings::default();
        s.optics.lens_profile = true;
        s.red_eye.push(eye);
        let p = plan(&src, &info, &s, &RenderRequest::fit(320, 160));
        assert!(p.eyes.iter().all(|k| k.cx.is_finite() && k.cy.is_finite() && k.r.is_finite()));
        assert!(p.frame.sample(&src, p.w, p.h).data.iter().flatten().all(|v| v.is_finite()));
    }
}

#[test]
fn malformed_source_buffers_are_rejected_before_orientation_and_prefiltering() {
    use lightcraft_pipeline::geometry::{BLANK, Frame};
    for src in [
        Rgb32f::new(0, 0),
        Rgb32f { width: 3, height: 2, data: vec![[0.8, 0.02, 0.02]] },
        Rgb32f { width: 3, height: 2, data: vec![[0.08; 3]; 7] },
        Rgb32f { width: usize::MAX, height: 2, data: vec![] },
    ] {
        for orientation in [Orientation::Normal, Orientation::Rotate90, Orientation::FlipH] {
            for warped in [false, true] {
                let mut s = DevelopSettings { orientation, ..Default::default() };
                if warped {
                    s.optics.distortion = 20.0;
                }
                let frame = Frame::new(32, 16, &s, true);
                assert_eq!(frame.sample(&src, 8, 4), Rgb32f::filled(8, 4, BLANK));
            }
        }
    }
}

#[test]
fn warped_sampling_checks_selected_rgb_after_gain_and_preserves_finite_output() {
    use lightcraft_pipeline::geometry::{BLANK, Frame};
    for channel in 0..3 {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut color = [0.08; 3];
            color[channel] = value;
            let src = Rgb32f::filled(32, 16, color);
            let mut s = DevelopSettings::default();
            s.optics.lens_profile = true;
            let info = lens_info(Some([1.01, 1.0, 0.99]));
            let frame = frame_for(&src, &info, &s, true);
            assert_eq!(frame.sample(&src, 32, 16), Rgb32f::filled(32, 16, BLANK));
        }
    }
    let src = Rgb32f::filled(32, 16, [f32::MAX; 3]);
    let mut s = DevelopSettings::default();
    s.optics.vignetting = 100.0;
    let frame = Frame::new(32, 16, &s, true);
    let sampled = frame.sample(&src, 32, 16);
    assert!(sampled.data.contains(&BLANK), "overflowing gain must produce blank pixels");
    assert!(sampled.data.iter().flatten().all(|v| v.is_finite()));
}
