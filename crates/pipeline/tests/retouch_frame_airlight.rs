//! Geometry-aware pupil detection and retouch-aware whole-frame airlight regressions.

use std::sync::Arc;

use lightcraft_develop::{DevelopSettings, LocalAdjustments, Mask, MaskComponent, MaskOp, MaskShape, RedEye, Spot, SpotMode, WbMode};
use lightcraft_geom::{Orientation, Point, Rect};
use lightcraft_pipeline::{OutputDepth, PixelWindow, RenderRequest, SourceInfo, StageCache, plan, render, render_cached};
use lightcraft_raster::Rgb32f;

const DEPTHS: [OutputDepth; 3] = [OutputDepth::U8, OutputDepth::U16, OutputDepth::F32Linear];
const WHOLE: PixelWindow = PixelWindow { x: 0, y: 0, w: 200, h: 100 };

fn pupil_source() -> Arc<Rgb32f> {
    Arc::new(Rgb32f::from_fn(200, 100, |x, y| {
        let d = ((x as f32 + 0.5 - 70.0).powi(2) + (y as f32 + 0.5 - 50.0).powi(2)).sqrt();
        if d < 6.0 { [0.5, 0.04, 0.04] } else { [0.12, 0.10, 0.08] }
    }))
}

fn haze_source() -> Arc<Rgb32f> {
    Arc::new(Rgb32f::from_fn(200, 100, |x, y| if (90..110).contains(&x) && (40..60).contains(&y) { [1.0; 3] } else { [0.1; 3] }))
}

fn clone_or_heal(mode: SpotMode) -> Spot {
    Spot { mode, points: vec![Point::new(0.5, 0.5)], size: 0.1, feather: 20.0, opacity: 100.0, source_offset: Some(Point::new(-0.3, 0.0)) }
}

#[test]
fn red_eye_detection_undoes_geometry_before_all_user_orientations() {
    let src = pupil_source();
    let info = SourceInfo::default();
    use Orientation::*;
    for orientation in [Normal, Rotate90, Rotate180, Rotate270, FlipH, Transverse, FlipV, Transpose] {
        for warped in [false, true] {
            let mut s = DevelopSettings { orientation, ..Default::default() };
            if warped {
                s.geometry.offset_x = if orientation == Normal { 80.0 } else { 40.0 };
                // The exact Offset X reproduction stays separate from the compound homography.
                if orientation != Normal {
                    s.geometry.horizontal = -10.0;
                    s.geometry.vertical = 12.0;
                    s.geometry.rotate = 8.0;
                    s.geometry.aspect = 15.0;
                    s.geometry.scale = 110.0;
                }
            }
            let frame = lightcraft_pipeline::frame_for(&src, &info, &s, true);
            let oriented = orientation.map_norm_rect(Rect::from_center(Point::new(0.35, 0.5), 0.0, 0.0)).center();
            let center = frame.corrected_to_transformed(oriented);
            let long = frame.ow.max(frame.oh);
            let eye = RedEye {
                center: Point::new(center.x + 14.0 / frame.ow, center.y),
                rx: 20.0 / long,
                ry: 12.0 / long,
                pupil_size: 50.0,
                darken: 50.0,
                pet: false,
                catchlight: None,
            };
            for depth in DEPTHS {
                let req = RenderRequest { depth, ..RenderRequest::fit(frame.ow as usize, frame.oh as usize) };
                let baseline = render(&src, &info, &s, &req);
                let cache = StageCache::default();
                let _warm = render_cached(&src, &info, &s, &req, &cache);
                let mut corrected = s.clone();
                corrected.red_eye.push(eye);
                let saved = corrected.clone();
                let p = plan(&src, &info, &corrected, &req);
                let at = p.frame.norm_to_out(p.w, p.h).apply(center);
                assert!((p.eyes[0].cx - at.x as f32).abs() < 2.0, "{orientation:?} {warped}: detected X follows the transformed pupil");
                assert!((p.eyes[0].cy - at.y as f32).abs() < 2.0, "{orientation:?} {warped}: detected Y follows the transformed pupil");
                let direct = render(&src, &info, &corrected, &req);
                let cached = render_cached(&src, &info, &corrected, &req, &cache);
                assert_eq!(direct.image, cached.image);
                assert_eq!(direct.deep, cached.deep);
                let whole = PixelWindow { x: 0, y: 0, w: p.w, h: p.h };
                let window_req = RenderRequest { window: Some(whole), ..req };
                let window = render(&src, &info, &corrected, &window_req);
                assert_eq!(window.image, direct.image, "whole window red-eye output");
                assert_eq!(window.deep, direct.deep, "whole window deep red-eye output");
                let cached_window = render_cached(&src, &info, &corrected, &window_req, &cache);
                assert_eq!(cached_window.image, window.image);
                assert_eq!(cached_window.deep, window.deep);
                let (x, y) = (at.x.floor() as usize, at.y.floor() as usize);
                let tile = PixelWindow { x: x.saturating_sub(20), y: y.saturating_sub(20), w: 40, h: 40 };
                let tile_output = render(&src, &info, &corrected, &RenderRequest { window: Some(tile), ..req });
                assert_eq!(tile_output.image, direct.image.crop(tile.x, tile.y, tile.w, tile.h), "pupil in a proper output window");
                let fixed = direct.image.get(x, y);
                assert_ne!(fixed, baseline.image.get(x, y), "{orientation:?} {warped} {depth:?}: the pupil is corrected");
                assert!(fixed[0].abs_diff(fixed[1]) <= 3, "{orientation:?} {warped} {depth:?}: neutral pupil {fixed:?}");
                assert_eq!(corrected, saved);
            }
        }
    }
}

#[test]
fn airlight_uses_clone_and_heal_after_white_balance_in_every_output_depth() {
    let src = haze_source();
    let info = SourceInfo { raw: true, as_shot_temp: 6500.0, ..Default::default() };
    for mode in [SpotMode::Clone, SpotMode::Heal] {
        for custom_wb in [false, true] {
            for depth in DEPTHS {
                let mut s = DevelopSettings::default();
                s.effects.dehaze = 40.0;
                if custom_wb {
                    s.wb.mode = WbMode::Custom;
                    s.wb.temp = 3200.0;
                    s.wb.tint = 12.0;
                }
                let req = RenderRequest { depth, ..RenderRequest::fit(200, 100) };
                let wr = RenderRequest { window: Some(WHOLE), ..req };
                let cache = StageCache::default();
                let unretouched = render_cached(&src, &info, &s, &wr, &cache);
                let full_unretouched = render(&src, &info, &s, &req);
                assert_eq!(unretouched.image, full_unretouched.image, "no retouch control");
                assert_eq!(unretouched.deep, full_unretouched.deep);
                s.spots.push(clone_or_heal(mode));
                let saved = s.clone();
                let full = render(&src, &info, &s, &req);
                let direct = render(&src, &info, &s, &wr);
                assert_eq!(direct.image, full.image, "{mode:?} {custom_wb} {depth:?}: whole window equals full output");
                assert_eq!(direct.deep, full.deep, "deep output equality, including float");
                let cold = StageCache::default();
                for stages in [&cache, &cold] {
                    let cached = render_cached(&src, &info, &s, &wr, stages);
                    assert_eq!(cached.image, direct.image);
                    assert_eq!(cached.deep, direct.deep);
                }
                assert_ne!(full.image, full_unretouched.image, "retouch must alter this fixture");
                let sub = PixelWindow { x: 0, y: 0, w: 60, h: 60 };
                let tile = render(&src, &info, &s, &RenderRequest { window: Some(sub), ..req });
                // The retouched bright patch is outside this tile, and all of its pixels are
                // constant. This isolates global airlight from spatial-filter boundaries.
                assert_eq!(tile.image, full.image.crop(sub.x, sub.y, sub.w, sub.h));
                assert_eq!(s, saved);
                s.spots.clear();
                let restored = render_cached(&src, &info, &s, &wr, &cache);
                assert_eq!(restored.image, unretouched.image, "removing retouch restores a warm cache");
                assert_eq!(restored.deep, unretouched.deep);
            }
        }
    }
}

#[test]
fn window_airlight_retains_inactive_dehaze_and_section_cache_controls() {
    let src = haze_source();
    let info = SourceInfo::default();
    let req = RenderRequest::fit(200, 100);
    let wr = RenderRequest { window: Some(WHOLE), ..req };
    let mut base = DevelopSettings::default();
    base.spots.push(clone_or_heal(SpotMode::Clone));
    let neutral = render(&src, &info, &base, &wr).image;
    let cache = StageCache::default();
    for state in ["absent", "hidden", "empty", "zero", "active"] {
        let mut s = base.clone();
        s.effects.dehaze = 40.0;
        let healthy = render_cached(&src, &info, &s, &wr, &cache).image;
        s.set_section_enabled("effects", false);
        let mut mask = Mask {
            id: 1,
            components: vec![MaskComponent {
                name: None,
                op: MaskOp::Add,
                invert: false,
                shape: MaskShape::Radial { center: Point::new(0.5, 0.5), rx: 1.0, ry: 1.0, angle: 0.0, feather: 0.0, invert: false },
            }],
            adjust: LocalAdjustments { dehaze: 60.0, ..Default::default() },
            ..Default::default()
        };
        match state {
            "hidden" => mask.visible = false,
            "empty" => mask.components.clear(),
            "zero" => mask.adjust.dehaze = 0.0,
            _ => {}
        }
        if state != "absent" {
            s.masks.push(mask);
        }
        let saved = s.clone();
        let p = plan(&src, &info, &s, &wr);
        assert_eq!(p.fixed_air.is_some(), state == "active");
        let direct = render(&src, &info, &s, &wr);
        let cached = render_cached(&src, &info, &s, &wr, &cache);
        assert_eq!(direct.image, cached.image);
        assert_eq!(direct.image, render(&src, &info, &s, &req).image);
        if state != "active" {
            assert_eq!(direct.image, neutral);
        }
        assert_eq!(s, saved);
        s.masks.clear();
        s.set_section_enabled("effects", true);
        assert!(plan(&src, &info, &s, &wr).fixed_air.is_some());
        assert_eq!(render_cached(&src, &info, &s, &wr, &cache).image, healthy, "re-enable cache control");
    }
}
