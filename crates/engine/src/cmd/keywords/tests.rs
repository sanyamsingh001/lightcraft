//! Keyword hierarchy regressions, including the followed-filter history contract.
use super::*;
use lightcraft_catalog::{Op, Photo, PhotoId, Source};

#[test]
fn unicode_keyword_filters_follow_rename_merge_and_history() {
    for (from, alternate) in [("STRAẞE", "Straße"), ("Straße", "STRAẞE"), ("DACH|STRAẞE", "Dach|Straße")] {
        for merge in [false, true] {
            let mut s = Session::new();
            for (id, spelling) in [(1, from), (2, alternate)] {
                let mut p = Photo::new(PhotoId(id), Source::Demo { scene: 1 }, &format!("{id}.jpg"), "JPEG", 4, 3, "2026-04-12T00:00:00");
                p.meta.keywords = vec![format!("{spelling}|Berlin|Mitte"), "Wege|Berlin|Mitte".into(), "Straßenfest|Berlin".into()];
                p.rating = 3;
                s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
            }
            let original = s.catalog.to_snapshot();
            let original_filter = format!("{alternate}|Berlin");
            s.execute("library.filter", &json!({"keyword": original_filter, "rating": 3})).unwrap();
            assert_eq!(s.visible().len(), 2);
            let (command, params) = if merge {
                ("keyword.merge", json!({"from": [from], "into": "Wege"}))
            } else {
                ("keyword.rename", json!({"from": from, "to": "Wege"}))
            };
            assert_eq!(s.execute(command, &params).unwrap()["changed"], 2);
            for id in [1, 2] {
                assert_eq!(s.catalog.photo(PhotoId(id)).unwrap().meta.keywords, ["Wege|Berlin|Mitte", "Straßenfest|Berlin"]);
            }
            let changed = s.catalog.to_snapshot();
            assert_eq!(s.filter.keyword.as_deref(), Some("Wege|Berlin"));
            assert_eq!(s.visible().len(), 2);
            s.execute("edit.undo", &json!({})).unwrap();
            assert_eq!(s.catalog.to_snapshot(), original);
            assert_eq!(s.filter.keyword.as_deref(), Some(original_filter.as_str()));
            assert_eq!(s.filter.rating, 3);
            assert_eq!(s.visible().len(), 2);
            s.execute("edit.redo", &json!({})).unwrap();
            assert_eq!(s.catalog.to_snapshot(), changed);
            assert_eq!(s.filter.keyword.as_deref(), Some("Wege|Berlin"));
            assert_eq!(s.filter.rating, 3);
            assert_eq!(s.visible().len(), 2);
            // A later manual filter choice remains independent of this global history.
            s.execute("library.filter", &json!({"keyword": "Straßenfest", "rating": 2})).unwrap();
            let independent = s.filter.clone();
            s.execute("edit.undo", &json!({})).unwrap();
            s.execute("edit.redo", &json!({})).unwrap();
            assert_eq!(s.filter, independent);
        }
    }
}

#[test]
fn unicode_case_only_keyword_filter_history_is_conditional_and_preserves_context() {
    for (from, to) in [("STRAẞE", "Straße"), ("Straße", "STRAẞE"), ("DACH|STRAẞE", "Dach|Straße")] {
        let mut s = Session::new();
        let mut photo = Photo::new(PhotoId(1), Source::Demo { scene: 1 }, "one.jpg", "JPEG", 4, 3, "");
        photo.meta.keywords = vec![format!("{from}|Berlin|Mitte"), "Other".into()];
        photo.rating = 3;
        s.catalog.apply(Op::AddPhoto { photo: Box::new(photo) }).unwrap();
        s.selection = crate::Selection::single(PhotoId(1));
        s.execute("library.filter", &json!({"keyword": format!("{from}|Berlin"), "rating": 3})).unwrap();
        let before = s.catalog.to_snapshot();
        let selection = s.selection.clone();
        assert_eq!(s.execute("keyword.rename", &json!({"from": from, "to": to})).unwrap()["changed"], 1);
        let after = s.catalog.to_snapshot();
        assert_eq!(s.filter.keyword, Some(format!("{to}|Berlin")));
        for (step, spelling, snapshot) in [("edit.undo", from, &before), ("edit.redo", to, &after)] {
            s.execute(step, &json!({})).unwrap();
            assert_eq!(&s.catalog.to_snapshot(), snapshot);
            assert_eq!(s.filter.keyword, Some(format!("{spelling}|Berlin")));
            assert_eq!(s.filter.rating, 3);
            assert_eq!(s.selection, selection);
            assert_eq!(s.visible(), &[PhotoId(1)]);
        }
        for descendant in [format!("{from}|Berlin"), format!("{to}|Berlin")] {
            assert!(s.execute("keyword.rename", &json!({"from": to, "to": descendant})).is_err());
            assert_eq!(s.catalog.to_snapshot(), after);
        }
        s.execute("library.filter", &json!({"keyword": "Other", "rating": 2})).unwrap();
        let chosen = s.filter.clone();
        for step in ["edit.undo", "edit.redo"] {
            s.execute(step, &json!({})).unwrap();
            assert_eq!(s.filter, chosen);
            assert_eq!(s.selection, selection);
        }
    }
}
