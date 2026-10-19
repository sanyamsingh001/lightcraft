//! Painted glyph bounds, not just hit rectangles, for the shared Masking sidebar (#372).
use super::{MASK_TILES, TILE_GAP, Tokens, describe_input, mask_name, mask_tile, tile_layout};
use crate::headless::HeadlessView;
use crate::i18n::{Locale, set_language};
use crate::{theme, widgets};
use egui::{Rect, Sense, epaint::Shape, vec2};

fn frame(width: f32, scale: f32, locale: Locale, mut draw: impl FnMut(&mut egui::Ui)) -> (egui::FullOutput, Vec<(String, Rect)>) {
    set_language(locale);
    let ctx = egui::Context::default();
    theme::install_fonts(&ctx);
    theme::apply(&ctx);
    let mut last = None;
    for i in 0..3 {
        last = Some(ctx.run_ui(HeadlessView::raw_input(vec2(width, 1800.0), scale, i as f64 / 60.0, vec![]), |ui| {
            ui.set_width(width);
            draw(ui);
        }));
        last.as_mut().unwrap().textures_delta.clear();
        if i < 2 {
            widgets::take_registry(&ctx);
        }
    }
    set_language(Locale::En);
    (last.unwrap(), widgets::take_registry(&ctx))
}
fn texts(shape: &Shape, out: &mut Vec<(String, Rect)>) {
    match shape {
        Shape::Vec(shapes) => {
            for s in shapes {
                texts(s, out);
            }
        }
        Shape::Text(t) => out.push((t.galley.text().to_string(), t.visual_bounding_rect())),
        _ => {}
    }
}
fn painted(out: &egui::FullOutput) -> Vec<(String, Rect)> {
    let mut result = vec![];
    for s in &out.shapes {
        texts(&s.shape, &mut result);
    }
    result
}
#[test]
fn translated_tile_glyphs_fit_at_sidebar_widths_and_retina_scale() {
    for locale in [Locale::En, Locale::De, Locale::PtBr] {
        for width in [250.0, 270.0, 330.0, 520.0] {
            for scale in [1.0, 2.0] {
                let mut labels = vec![];
                let (out, widgets) = frame(width, scale, locale, |ui| {
                    labels = MASK_TILES.iter().map(|(kind, label, _)| (format!("maskNew:{kind}"), crate::i18n::tr(label).to_string())).collect();
                    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 0, bottom: 10 }).show(ui, |ui| {
                        let t = Tokens::get(ui.ctx());
                        let max = labels
                            .iter()
                            .map(|(_, s)| ui.painter().layout_no_wrap(s.clone(), t.font(10.5), t.text_dim).size().x)
                            .fold(0.0_f32, f32::max);
                        let (cols, tile) = tile_layout(ui.available_width(), max);
                        egui::Grid::new("mask-tiles").spacing(vec2(TILE_GAP, TILE_GAP)).show(ui, |ui| {
                            for (i, (kind, label, icon)) in MASK_TILES.iter().enumerate() {
                                mask_tile(ui, kind, label, *icon, tile);
                                if i % cols == cols - 1 {
                                    ui.end_row();
                                }
                            }
                        });
                    });
                });
                let text = painted(&out);
                for (id, label) in &labels {
                    let tile = widgets.iter().find(|(key, _)| key == id).unwrap().1;
                    let bounds = text.iter().find(|(s, _)| s == label).unwrap().1;
                    assert!(
                        tile.shrink(2.0).expand(0.5).contains_rect(bounds),
                        "{locale:?} width={width} scale={scale} {label}: {bounds:?} outside {tile:?}"
                    );
                    assert!(tile.left() >= 24.0 - 0.5 && tile.right() <= width - 22.0 + 0.5, "{tile:?}");
                }
            }
        }
    }
}
#[test]
fn long_mask_names_end_before_the_visibility_control() {
    let name = "The person standing beside the very long fence across the background 日本語";
    for width in [250.0, 270.0, 330.0, 520.0] {
        for scale in [1.0, 2.0] {
            let mut row = Rect::NOTHING;
            let (out, _) = frame(width, scale, Locale::En, |ui| {
                row = ui.allocate_exact_size(vec2(width - 32.0, 30.0), Sense::hover()).0;
                mask_name(ui, row, name, false);
            });
            let bounds = painted(&out).into_iter().find(|(s, _)| s == name).unwrap().1;
            assert!(bounds.left() >= row.left() + 31.5 && bounds.right() <= row.right() - 31.5, "{bounds:?} vs {row:?}");
            assert!(row.expand(0.5).contains_rect(bounds));
        }
    }
}
#[test]
fn describe_reserves_the_translated_action_and_preserves_long_input() {
    let original = "a very long description of several objects across the background; ".repeat(20);
    for locale in [Locale::En, Locale::De, Locale::PtBr] {
        for width in [250.0, 270.0, 330.0, 520.0] {
            for scale in [1.0, 2.0] {
                let mut text = original.clone();
                let (out, widgets) = frame(width, scale, locale, |ui| {
                    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 0, bottom: 10 }).show(ui, |ui| {
                        assert!(!describe_input(ui, &mut text));
                    });
                });
                assert_eq!(text, original, "layout keeps all user text");
                let input = widgets.iter().find(|(id, _)| id == "maskDescribe").unwrap().1;
                let button = widgets.iter().find(|(id, _)| id == "button:maskDescribeGo").unwrap().1;
                assert!(input.left() >= 24.0 - 0.5 && button.right() <= width - 22.0 + 0.5, "{locale:?} width={width} {input:?} {button:?}");
                assert!(input.right() <= button.left(), "editor and action must stay separate");
                let text = painted(&out);
                let label = locale.tr("Select");
                let bounds = text.iter().find(|(s, _)| s == label).unwrap().1;
                assert!(button.expand(0.5).contains_rect(bounds), "translated action text remains inside its button");
            }
        }
    }
}
