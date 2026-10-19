//! Retouch-aware whole-frame airlight regressions extracted from the existing mixed module.

use std::sync::Arc;

use lightcraft_develop::{DevelopSettings, LocalAdjustments, Mask, MaskComponent, MaskOp, MaskShape, Spot, SpotMode, WbMode};
use lightcraft_geom::Point;
use lightcraft_pipeline::{OutputDepth, PixelWindow, RenderRequest, SourceInfo, StageCache, plan, render, render_cached};
use lightcraft_raster::Rgb32f;

const DEPTHS: [OutputDepth; 3] = [OutputDepth::U8, OutputDepth::U16, OutputDepth::F32Linear];
const WHOLE: PixelWindow = PixelWindow { x: 0, y: 0, w: 200, h: 100 };

fn haze_source() -> Arc<Rgb32f> {
    Arc::new(Rgb32f::from_fn(200, 100, |x, y| if (90..110).contains(&x) && (40..60).contains(&y) { [1.0; 3] } else { [0.1; 3] }))
}

fn clone_or_heal(mode: SpotMode) -> Spot {
    Spot { mode, points: vec![Point::new(0.5, 0.5)], size: 0.1, feather: 20.0, opacity: 100.0, source_offset: Some(Point::new(-0.3, 0.0)) }
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
