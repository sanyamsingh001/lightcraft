//! Real command exclusion and co-occurrence ranking use the catalog's Unicode identity.
use super::*;
use lightcraft_catalog::{Op, Photo, PhotoId, Source};

#[test]
fn suggestions_exclude_assigned_unicode_spellings_and_rank_equivalent_co_occurrence() {
    for (stored, alternate, prefix) in
        [("CAFÉ", "café", "ca"), ("TARGET|STRAẞE", "Target|Straße", "str"), ("ΑΘΉΝΑ", "Αθήνα", "αθ"), ("TRAVEL", "travel", "tr")]
    {
        let mut s = Session::new();
        for (id, keywords) in [(1, vec![stored]), (2, vec![alternate, "park"]), (3, vec!["beach"]), (4, vec!["beach"]), (5, vec!["beach"])] {
            let mut p = Photo::new(PhotoId(id), Source::Demo { scene: 0 }, "fixture.jpg", "JPEG", 4, 3, "");
            p.meta.keywords = keywords.into_iter().map(str::to_string).collect();
            s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
        }
        s.selection = crate::Selection::single(PhotoId(1));
        let snapshot = s.catalog.to_snapshot();
        for prefix in [prefix, ""] {
            let result = s.execute("keyword.suggest", &json!({"prefix": prefix, "limit": 12})).unwrap();
            let suggestions = result.as_array().unwrap();
            assert!(suggestions.iter().all(|k| k.as_str().unwrap().to_lowercase() != stored.to_lowercase()), "{stored}: {result}");
            if prefix.is_empty() {
                assert_eq!(suggestions[0], "park", "co-occurrence outranks the more frequent unrelated beach");
            }
        }
        assert_eq!(s.catalog.to_snapshot(), snapshot);
        assert!(s.undo.is_empty() && s.redo.is_empty() && s.pending_log.is_empty(), "queries leave history/log unchanged");
    }
}

#[test]
fn suggestions_keep_descendants_and_neighboring_keyword_boundaries() {
    let mut s = Session::new();
    let mut p = Photo::new(PhotoId(1), Source::Demo { scene: 0 }, "fixture.jpg", "JPEG", 4, 3, "");
    p.meta.keywords = vec!["CAFÉ".into(), "café|Street".into(), "caféhouse".into()];
    s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    let result = s.execute("keyword.suggest", &json!({"ids": [], "prefix": "ca"})).unwrap();
    assert_eq!(result.as_array().unwrap().len(), 3, "without current keywords all three are distinct suggestions");
    let suggestions = s.catalog.keyword_suggestions(&["café".into()], "ca", 12);
    assert!(!suggestions.contains(&"CAFÉ".into()));
    assert!(suggestions.contains(&"café|Street".into()));
    assert!(suggestions.contains(&"caféhouse".into()));
}
