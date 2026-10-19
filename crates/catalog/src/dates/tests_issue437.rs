//! Capture-date consumers must not mistake the import timestamp for capture metadata.

use super::*;
use crate::{Filter, Op, Photo, Sort, Source};

const IMPORTED: &str = "2026-10-09T12:34:56";

fn imported_photo(c: &mut Catalog, captured: Option<&str>, imported: &str) -> PhotoId {
    let id = c.alloc_photo_id();
    // The importer constructs a File photo with an import timestamp, then copies the optional
    // capture metadata. No file decode is needed to exercise its catalog consumers.
    let mut p = Photo::new(id, Source::File { path: format!("/photos/{id:?}.jpg") }, "scan.jpg", "JPEG", 3, 2, imported);
    p.captured = captured.map(str::to_owned);
    c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    id
}

fn sort(ascending: bool) -> Sort {
    Sort { ascending, ..Sort::default() }
}

#[test]
fn imported_undated_files_use_unknown_capture_groups_but_keep_import_dates() {
    let mut c = Catalog::new();
    let a = imported_photo(&mut c, None, IMPORTED);
    let b = imported_photo(&mut c, None, "2010-01-02T03:04:05");
    let before = c.to_snapshot();
    for ascending in [false, true] {
        let ids = c.query(&Filter::default(), &sort(ascending));
        assert_eq!(ids, if ascending { vec![a, b] } else { vec![b, a] }, "undated ties use id, never import time");
        for by in [GroupBy::Auto, GroupBy::Day, GroupBy::Month, GroupBy::Year] {
            assert_eq!(
                c.date_runs(&ids, SortKey::CaptureDate, by),
                [DateRun { key: String::new(), label: "Unknown Date".into(), start: 0, count: 2 }]
            );
        }
    }
    assert!(c.date_groups().is_empty());
    let ids = c.query(&Filter::default(), &Sort { key: SortKey::ImportDate, ..Sort::default() });
    assert_eq!(ids, [a, b]);
    let runs = c.date_runs(&ids, SortKey::ImportDate, GroupBy::Day);
    assert_eq!(runs.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(), ["2026-10-09", "2010-01-02"]);
    assert_eq!(runs[0].label, "Friday, 9 October 2026");
    assert_eq!(c.photo(a).unwrap().date(), IMPORTED, "filename/template fallback remains available");
    assert_eq!(c.photo(a).unwrap().captured, None);
    assert_eq!(c.photo(a).unwrap().imported, IMPORTED);
    assert_eq!(c.to_snapshot(), before, "queries do not invent or rewrite metadata");
    let restored = Catalog::from_snapshot(&before).unwrap();
    assert_eq!(restored.query(&Filter::default(), &Sort::default()), [b, a]);
    assert_eq!(restored.date_runs(&[b, a], SortKey::CaptureDate, GroupBy::Day)[0].key, "");
}

#[test]
fn unusable_capture_metadata_forms_one_run_in_both_sort_directions() {
    let mut c = Catalog::new();
    let unknown: Vec<_> =
        [None, Some(""), Some("2024-02-30"), Some("9999-99-99"), Some("2024-01é0"), Some("2020x01x02"), Some("2024-02"), Some("照片2024-02-29")]
            .into_iter()
            .map(|d| imported_photo(&mut c, d, IMPORTED))
            .collect();
    let known: Vec<_> = ["1900-02-28", "2000-02-29", "2024-02-29T09:00:00", "2024-02-29T18:00:00.25+02:00", IMPORTED]
        .into_iter()
        .map(|d| imported_photo(&mut c, Some(d), "2030-01-01T00:00:00"))
        .collect();
    let before = c.to_snapshot();
    for ascending in [false, true] {
        let ids = c.query(&Filter::default(), &sort(ascending));
        let mut expected = unknown.iter().chain(&known).copied().collect::<Vec<_>>();
        if !ascending {
            expected.reverse();
        }
        assert_eq!(ids, expected);
        for by in [GroupBy::Auto, GroupBy::Day, GroupBy::Month, GroupBy::Year] {
            let runs = c.date_runs(&ids, SortKey::CaptureDate, by);
            let undated = runs.iter().filter(|r| r.key.is_empty()).collect::<Vec<_>>();
            assert_eq!(undated.len(), 1, "one unknown run: {ascending}, {by:?}");
            assert_eq!(undated[0].count, unknown.len());
            assert_eq!(undated[0].start, if ascending { 0 } else { known.len() });
            assert_eq!(undated[0].label, "Unknown Date");
            assert_eq!(runs.iter().map(|r| r.count).sum::<usize>(), ids.len());
            assert_eq!(runs.len(), 5, "real dates keep their day/month/year groups");
        }
        let runs = c.date_runs(&ids, SortKey::CaptureDate, GroupBy::Day);
        assert!(runs.iter().any(|r| r.key == "2024-02-29" && r.count == 2));
        assert!(runs.iter().any(|r| r.key == "2026-10-09" && r.count == 1), "a real capture date equal to import day stays dated");
    }
    assert_eq!(c.date_groups().iter().map(|g| (g.year.as_str(), g.count)).collect::<Vec<_>>(), [("2026", 1), ("2024", 2), ("2000", 1), ("1900", 1)]);
    assert_eq!(c.to_snapshot(), before);
}

#[test]
fn capture_date_filters_and_sidebar_counts_agree_without_import_fallback() {
    let mut c = Catalog::new();
    imported_photo(&mut c, None, IMPORTED);
    imported_photo(&mut c, Some(""), IMPORTED);
    imported_photo(&mut c, Some("2026-10-32"), IMPORTED);
    let valid = imported_photo(&mut c, Some("2026-10-01T10:05+02:00"), "2030-01-01");
    let old = imported_photo(&mut c, Some("2024-02-29"), IMPORTED);
    for filter in [
        Filter { date: Some("2026-10".into()), ..Default::default() },
        Filter { text: "date:2026-10".into(), ..Default::default() },
        Filter { date_from: Some("2026-10".into()), ..Default::default() },
        Filter { date_from: Some("2026-10-01".into()), date_to: Some("2026-10".into()), ..Default::default() },
    ] {
        assert_eq!(c.query(&filter, &Sort::default()), [valid], "{filter:?}");
    }
    assert_eq!(c.query(&Filter { date_to: Some("2024-02".into()), ..Default::default() }, &Sort::default()), [old]);
    let imported = c.query(&Filter { imported: Some("2026-10".into()), ..Default::default() }, &Sort::default());
    assert_eq!(imported.len(), 4, "explicit import filter still includes undated files");
    let groups = c.date_groups();
    assert_eq!(groups[0].count, 1);
    assert_eq!(groups[0].days, [("2026-10-01".into(), 1)]);
    assert_eq!(groups[1].days, [("2024-02-29".into(), 1)]);
}

#[test]
fn stacks_keep_the_first_visible_members_capture_group_even_when_undated() {
    for first_capture in [None, Some("2024-02-29")] {
        let mut c = Catalog::new();
        let first = imported_photo(&mut c, first_capture, IMPORTED);
        let second = imported_photo(&mut c, if first_capture.is_some() { None } else { Some("2024-02-29") }, IMPORTED);
        let other = imported_photo(&mut c, Some("2000-02-29"), IMPORTED);
        let op = c.group_ops(first, &[first, second], false).unwrap();
        c.apply(op).unwrap();
        let ids = c.arrange_stacks(&[first, other, second]);
        assert_eq!(ids, [first, second, other]);
        for by in [GroupBy::Auto, GroupBy::Day, GroupBy::Month, GroupBy::Year] {
            let runs = c.date_runs(&ids, SortKey::CaptureDate, by);
            assert_eq!(runs.len(), 2);
            assert_eq!(runs[0].count, 2, "the mixed-date stack stays together");
            assert_eq!(runs[0].key.is_empty(), first_capture.is_none());
            assert_eq!(runs[1].start, 2);
            if first_capture.is_none() {
                assert_eq!(runs[0].label, "Unknown Date");
            }
        }
        assert!(c.date_runs(&ids, SortKey::CaptureDate, GroupBy::None).is_empty());
    }
}

#[test]
fn allocation_free_calendar_validation_matches_capture_time_normalization() {
    for y in [0, 1, 4, 100, 400, 1900, 2000, 2024, 2026, 9999] {
        for m in 0..=13 {
            for d in 0..=32 {
                let date = format!("{y:04}-{m:02}-{d:02}");
                assert_eq!(valid_date_prefix(&date).is_some(), normalize_iso(&date).is_some(), "{date}");
            }
        }
    }
    for date in ["2024x02x29", "２０２４-02-29", "2024-01é0", " 2024-02-29", "2024-02"] {
        assert!(valid_date_prefix(date).is_none(), "{date}");
    }
    // As with the existing grid calendar policy, time and zone suffixes do not change the day.
    for date in ["2024-02-29T10:05+02:00", "2024-02-29T23:59:59.25Z", "2024-02-29Tbad-clock"] {
        assert_eq!(valid_date_prefix(date), Some("2024-02-29"));
    }
}

#[test]
fn empty_capture_date_filters_remain_unconstrained() {
    let mut c = Catalog::new();
    imported_photo(&mut c, None, IMPORTED);
    imported_photo(&mut c, Some(""), IMPORTED);
    imported_photo(&mut c, Some("bad-date"), IMPORTED);
    imported_photo(&mut c, Some("2024-02-29"), IMPORTED);
    let all = c.query(&Filter::default(), &Sort::default());
    for filter in [
        Filter { date: Some(String::new()), ..Default::default() },
        Filter { date_from: Some(String::new()), ..Default::default() },
        Filter { date_to: Some(String::new()), ..Default::default() },
        Filter { text: "date:".into(), ..Default::default() },
    ] {
        assert_eq!(c.query(&filter, &Sort::default()), all, "{filter:?}");
    }
}
