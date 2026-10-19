//! Geometry-aware pupil regression extracted unchanged from the existing mixed module.

use std::sync::Arc;

use lightcraft_develop::{DevelopSettings, RedEye};
use lightcraft_geom::{Orientation, Point, Rect};
use lightcraft_pipeline::{OutputDepth, PixelWindow, RenderRequest, SourceInfo, StageCache, plan, render, render_cached};
use lightcraft_raster::Rgb32f;

const DEPTHS: [OutputDepth; 3] = [OutputDepth::U8, OutputDepth::U16, OutputDepth::F32Linear];

fn pupil_source() -> Arc<Rgb32f> {
    Arc::new(Rgb32f::from_fn(200, 100, |x, y| {
        let d = ((x as f32 + 0.5 - 70.0).powi(2) + (y as f32 + 0.5 - 50.0).powi(2)).sqrt();
        if d < 6.0 { [0.5, 0.04, 0.04] } else { [0.12, 0.10, 0.08] }
    }))
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
