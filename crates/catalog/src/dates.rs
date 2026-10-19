//! Calendar helpers for ISO 8601 local times (`2026-09-30T14:05:00`): seconds ↔ civil date,
//! weekday and month names, shifting a time by an offset, and grouping a photo list into runs of
//! the same day / month / year (the date headers of the photo grid).

use serde::{Deserialize, Serialize};

pub use crate::stacks::iso_seconds;
use crate::{Catalog, PhotoId, SortKey};

pub const WEEKDAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
pub const MONTHS: [&str; 12] =
    ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

/// Unix seconds → `YYYY-MM-DDTHH:MM:SS` (proleptic Gregorian; H. Hinnant's algorithm).
pub fn civil(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Days since 1970-01-01 → (year, month 1..=12, day 1..=31).
pub fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Weekday name of an ISO date (`None` if it doesn't parse).
pub fn weekday(iso: &str) -> Option<&'static str> {
    let days = iso_seconds(valid_date_prefix(iso)?)?.div_euclid(86_400);
    // 1970-01-01 was a Thursday
    WEEKDAYS.get((days + 3).rem_euclid(7) as usize).copied()
}

/// Shift an ISO time by `secs`, keeping anything after the seconds (fractions, zone) as it was.
/// A date without a time gets one. `None` if it doesn't parse.
pub fn shift_iso(iso: &str, secs: i64) -> Option<String> {
    valid_date_prefix(iso.trim())?;
    let t = iso_seconds(iso)?;
    let tail = iso.trim().get(19..).unwrap_or("");
    Some(format!("{}{tail}", civil(t.checked_add(secs)?)))
}

/// Normalize user input to `YYYY-MM-DDTHH:MM:SS` (accepts a space instead of `T`, missing
/// seconds or time). `None` if it isn't a valid date.
pub fn normalize_iso(s: &str) -> Option<String> {
    let t = iso_seconds(s)?;
    let out = civil(t);
    // reject impossible dates that the arithmetic silently rolls over (2026-02-30)
    if out.get(..10) != s.trim().get(..10) {
        return None;
    }
    Some(out)
}

/// A complete, valid ISO calendar date at the start of metadata, borrowed without splitting
/// UTF-8 characters. Use the same calendar validation as capture-time input; fractions, times
/// and zone suffixes do not affect date grouping.
pub(crate) fn valid_date_prefix(iso: &str) -> Option<&str> {
    let date = iso.get(..10)?;
    normalize_iso(date)?;
    Some(date)
}

/// Granularity of the grid's date headers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GroupBy {
    /// Day headers, or months/years when the thumbnails are small (decided by the UI).
    #[default]
    Auto,
    None,
    Day,
    Month,
    Year,
}

impl GroupBy {
    pub fn parse(s: &str) -> Option<GroupBy> {
        Some(match s.to_ascii_lowercase().as_str() {
            "auto" => GroupBy::Auto,
            "none" | "off" => GroupBy::None,
            "day" => GroupBy::Day,
            "month" => GroupBy::Month,
            "year" => GroupBy::Year,
            _ => return None,
        })
    }
    /// Length of the ISO prefix that identifies a group.
    fn prefix(self) -> usize {
        match self {
            GroupBy::Year => 4,
            GroupBy::Month => 7,
            _ => 10,
        }
    }
}

/// A run of consecutive photos with the same date (one header in the grid).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DateRun {
    /// `2026-09-30` / `2026-09` / `2026`, or empty for photos without a usable date.
    pub key: String,
    /// "Wednesday, 30 September 2026" / "September 2026" / "2026" / "Unknown Date".
    pub label: String,
    /// Index of the first photo in the list.
    pub start: usize,
    pub count: usize,
}

/// A capture time for display: `2022-03-30T22:11:11` → "March 30, 2022 at 10:11:11 PM" (the
/// date alone when there is no time; the input unchanged when it doesn't parse).
pub fn display_time(iso: &str) -> String {
    if valid_date_prefix(iso).is_none() {
        return iso.to_string();
    }
    let num = |r: std::ops::Range<usize>| iso.get(r).and_then(|v| v.parse::<u32>().ok());
    let (Some(y), Some(mo), Some(d)) = (num(0..4), num(5..7), num(8..10)) else { return iso.to_string() };
    let Some(month) = MONTHS.get((mo as usize).wrapping_sub(1)) else { return iso.to_string() };
    let date = format!("{month} {d}, {y}");
    match (num(11..13), num(14..16), num(17..19)) {
        (Some(h), Some(mi), s) => {
            let (h12, ampm) = match h {
                0 => (12, "AM"),
                1..=11 => (h, "AM"),
                12 => (12, "PM"),
                _ => (h - 12, "PM"),
            };
            format!("{date} at {h12}:{mi:02}:{:02} {ampm}", s.unwrap_or(0))
        }
        _ => date,
    }
}

/// Header text for a group key.
pub fn group_label(key: &str) -> String {
    let complete;
    let date = match key.len() {
        4 => {
            complete = format!("{key}-01-01");
            complete.as_str()
        }
        7 => {
            complete = format!("{key}-01");
            complete.as_str()
        }
        10 => key,
        _ => return "Unknown Date".into(),
    };
    let Some(date) = valid_date_prefix(date) else { return "Unknown Date".into() };
    let Some(year) = date.get(..4) else { return "Unknown Date".into() };
    let month = date.get(5..7).and_then(|m| m.parse::<usize>().ok()).and_then(|m| MONTHS.get(m.wrapping_sub(1)));
    match key.len() {
        10 => match (weekday(date), month, date.get(8..10).and_then(|d| d.parse::<u32>().ok())) {
            (Some(w), Some(m), Some(d)) => format!("{w}, {d} {m} {year}"),
            _ => "Unknown Date".into(),
        },
        7 => month.map(|m| format!("{m} {year}")).unwrap_or_else(|| "Unknown Date".into()),
        4 => year.to_string(),
        _ => "Unknown Date".into(),
    }
}

impl Catalog {
    /// Split `ids` (the grid order) into runs of the same day / month / year for `key`'s date.
    /// Members of a stack share the group of the stack's first photo in the list, so a stack is
    /// never split by a header. Empty for sort keys without a date and for [`GroupBy::None`]
    /// ([`GroupBy::Auto`] means days here).
    pub fn date_runs(&self, ids: &[PhotoId], key: SortKey, by: GroupBy) -> Vec<DateRun> {
        if by == GroupBy::None || ids.is_empty() {
            return Vec::new();
        }
        let n = by.prefix();
        let date_of = |id: &PhotoId| -> String {
            let Some(p) = self.photo(*id) else { return String::new() };
            let d = match key {
                SortKey::CaptureDate => Some(p.date()),
                SortKey::ImportDate => Some(p.imported.as_str()),
                SortKey::EditDate => p.edited.as_deref(),
                _ => None,
            };
            d.and_then(valid_date_prefix).and_then(|d| d.get(..n)).unwrap_or("").to_string()
        };
        if !matches!(key, SortKey::CaptureDate | SortKey::ImportDate | SortKey::EditDate) {
            return Vec::new();
        }
        let stacks = self.stack_index();
        let mut runs: Vec<DateRun> = Vec::new();
        let mut stack_key: std::collections::HashMap<crate::StackId, String> = Default::default();
        for (i, id) in ids.iter().enumerate() {
            let k = match stacks.get(id) {
                Some((sid, _)) => stack_key.entry(*sid).or_insert_with(|| date_of(id)).clone(),
                None => date_of(id),
            };
            match runs.last_mut() {
                Some(r) if r.key == k => r.count += 1,
                _ => runs.push(DateRun { label: group_label(&k), key: k, start: i, count: 1 }),
            }
        }
        runs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Op, Photo, Source};

    #[test]
    fn calendar() {
        assert_eq!(civil(0), "1970-01-01T00:00:00");
        assert_eq!(civil(951_782_400), "2000-02-29T00:00:00");
        assert_eq!(weekday("2026-09-30"), Some("Wednesday"));
        assert_eq!(weekday("2026-10-01T09:00:00"), Some("Thursday"));
        assert_eq!(weekday("2000-02-29"), Some("Tuesday"));
        assert_eq!(group_label("2026-09-29"), "Tuesday, 29 September 2026");
        assert_eq!(group_label("2026-02"), "February 2026");
        assert_eq!(group_label("2026"), "2026");
        assert_eq!(group_label(""), "Unknown Date");
        assert_eq!(group_label("2024-01é0"), "Unknown Date");
        assert_eq!(group_label("202é01"), "Unknown Date");
        assert_eq!(shift_iso("2026-12-31T23:30:00", 3600).as_deref(), Some("2027-01-01T00:30:00"));
        assert_eq!(shift_iso("2026-03-01T00:00:00.25+02:00", -1).as_deref(), Some("2026-02-28T23:59:59.25+02:00"));
        assert_eq!(shift_iso("nope", 5), None);
        assert_eq!(normalize_iso("2026-04-01 10:05").as_deref(), Some("2026-04-01T10:05:00"));
        assert_eq!(normalize_iso("2026-02-30"), None);
    }

    #[test]
    fn malformed_date_metadata_is_rejected_by_shared_calendar_helpers() {
        for date in [
            "",
            "2026-04",
            "照片2026-04-12",
            "2024-01é0",
            "202é01",
            "２０２６-04-12",
            "2026/04/12",
            "2026-4-12",
            "abcd-04-12",
            "2026-00-12",
            "2026-13-01",
            "2026-02-29",
            "1900-02-29",
            "2026-02-30",
            "2026-04-31",
        ] {
            assert_eq!(valid_date_prefix(date), None, "{date:?}");
            assert_eq!(normalize_iso(date), None, "{date:?}");
            assert_eq!(weekday(date), None, "{date:?}");
            assert_eq!(shift_iso(date, 1), None, "{date:?}");
            assert_eq!(display_time(date), date, "unusable metadata remains unchanged: {date:?}");
        }
        for key in ["", "abcd", "202é01", "2026x01", "2026-00", "2026-13", "2024-01é0", "2026-02-29", "1900-02-29"] {
            assert_eq!(group_label(key), "Unknown Date", "{key:?}");
        }
    }

    #[test]
    fn valid_calendar_boundaries_keep_their_dates_weekdays_and_labels() {
        for (date, day, label) in [
            ("1970-01-01", "Thursday", "Thursday, 1 January 1970"),
            ("1900-02-28", "Wednesday", "Wednesday, 28 February 1900"),
            ("2000-02-29", "Tuesday", "Tuesday, 29 February 2000"),
            ("2024-02-29", "Thursday", "Thursday, 29 February 2024"),
            ("2026-04-30", "Thursday", "Thursday, 30 April 2026"),
            ("9999-12-31", "Friday", "Friday, 31 December 9999"),
        ] {
            assert_eq!(valid_date_prefix(date), Some(date));
            assert_eq!(normalize_iso(date), Some(format!("{date}T00:00:00")));
            assert_eq!(weekday(date), Some(day));
            assert_eq!(group_label(date), label);
            let timestamp = format!("{date}T12:34:56.25+02:00");
            assert_eq!(valid_date_prefix(&timestamp), Some(date), "time suffixes do not change date grouping");
            assert_eq!(weekday(&timestamp), Some(day));
        }
        assert_eq!(valid_date_prefix("0000-02-29"), Some("0000-02-29"));
        assert_eq!(group_label("2024-02"), "February 2024");
        assert_eq!(group_label("1900"), "1900");
        assert_eq!(normalize_iso(" 2024-02-29 10:15 ").as_deref(), Some("2024-02-29T10:15:00"));
        assert_eq!(display_time("2024-02-29T12:00:00"), "February 29, 2024 at 12:00:00 PM");
        assert_eq!(shift_iso("2024-02-29T23:59:59.25+02:00", 1).as_deref(), Some("2024-03-01T00:00:00.25+02:00"));
        assert_eq!(shift_iso("2000-03-01T00:00:00", -1).as_deref(), Some("2000-02-29T23:59:59"));
        assert_eq!(shift_iso("1900-03-01T00:00:00", -1).as_deref(), Some("1900-02-28T23:59:59"));
    }

    #[test]
    fn date_shifts_reject_out_of_range_offsets() {
        assert_eq!(shift_iso("1970-01-02", i64::MAX), None);
        assert_eq!(shift_iso("1969-12-31", i64::MIN), None);
        assert_eq!(shift_iso("1970-01-01", 0).as_deref(), Some("1970-01-01T00:00:00"));
    }

    #[test]
    fn malformed_dates_are_ignored_by_sidebar_counts_and_use_unknown_grid_groups() {
        let mut c = Catalog::new();
        for date in ["2026-01-01", "2024-02-29", "2024-02-29T12:00:00", "2000-02-29", "1900-02-28"] {
            let id = c.alloc_photo_id();
            let mut p = Photo::new(id, Source::Demo { scene: 1 }, "valid.jpg", "JPEG", 3, 2, "2026-10-01T00:00:00");
            p.captured = Some(date.into());
            c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
        }
        let mut malformed = Vec::new();
        for date in ["", "照片2026-04-12", "2024-01é0", "202é01", "２０２６-04-12", "2026x04x12", "2026-02-29", "1900-02-29", "2026-04-31"] {
            let id = c.alloc_photo_id();
            let mut p = Photo::new(id, Source::Demo { scene: 1 }, "malformed.jpg", "JPEG", 3, 2, date);
            p.captured = Some(date.into());
            p.edited = Some(date.into());
            c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
            malformed.push(id);
        }
        for (local, deleted) in [(true, false), (false, true)] {
            let id = c.alloc_photo_id();
            let mut p = Photo::new(id, Source::Demo { scene: 1 }, "excluded.jpg", "JPEG", 3, 2, "2026-10-01T00:00:00");
            p.captured = Some("2024-02-29".into());
            p.local = local;
            p.deleted = deleted;
            c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
        }
        // An absent capture time still uses the existing import-date fallback.
        let id = c.alloc_photo_id();
        let fallback = Photo::new(id, Source::Demo { scene: 1 }, "fallback.jpg", "JPEG", 3, 2, "2026-06-01T12:00:00");
        c.apply(Op::AddPhoto { photo: Box::new(fallback) }).unwrap();
        let snapshot = c.to_snapshot();
        let groups = c.date_groups();
        assert_eq!(groups.iter().map(|g| (g.year.as_str(), g.count)).collect::<Vec<_>>(), [("2026", 2), ("2024", 2), ("2000", 1), ("1900", 1)]);
        assert_eq!(groups[0].months, [("2026-06".to_string(), 1), ("2026-01".to_string(), 1)]);
        assert_eq!(groups[0].days, [("2026-06-01".to_string(), 1), ("2026-01-01".to_string(), 1)]);
        assert_eq!(groups[1].months, [("2024-02".to_string(), 2)]);
        assert_eq!(groups[1].days, [("2024-02-29".to_string(), 2)]);
        assert_eq!(groups[2].days, [("2000-02-29".to_string(), 1)]);
        assert_eq!(groups[3].days, [("1900-02-28".to_string(), 1)]);
        for key in [SortKey::CaptureDate, SortKey::ImportDate, SortKey::EditDate] {
            for by in [GroupBy::Day, GroupBy::Month, GroupBy::Year] {
                assert_eq!(
                    c.date_runs(&malformed, key, by),
                    [DateRun { key: String::new(), label: "Unknown Date".into(), start: 0, count: malformed.len() }],
                    "{key:?}, {by:?}"
                );
            }
        }
        assert_eq!(c.to_snapshot(), snapshot, "grouping does not rewrite malformed metadata");
    }

    #[test]
    fn runs_by_day_month_year_keep_stacks_together() {
        let mut c = Catalog::new();
        let times = ["2026-09-30T18:00:00", "2026-09-30T09:00:00", "2026-09-29T23:59:00", "2026-08-01T10:00:00", "2025-12-31T10:00:00", ""];
        let mut ids = Vec::new();
        for t in times {
            let id = c.alloc_photo_id();
            let mut p = Photo::new(id, Source::Demo { scene: 1 }, "a.jpg", "JPEG", 3, 2, "2026-10-01T00:00:00");
            p.captured = (!t.is_empty()).then(|| t.to_string());
            c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
            ids.push(id);
        }
        let shape = |r: &[DateRun]| r.iter().map(|r| (r.key.clone(), r.start, r.count)).collect::<Vec<_>>();
        let day = c.date_runs(&ids, SortKey::CaptureDate, GroupBy::Day);
        // the undated photo falls back to its import date
        assert_eq!(
            shape(&day),
            vec![
                ("2026-09-30".into(), 0, 2),
                ("2026-09-29".into(), 2, 1),
                ("2026-08-01".into(), 3, 1),
                ("2025-12-31".into(), 4, 1),
                ("2026-10-01".into(), 5, 1)
            ]
        );
        assert_eq!(day[0].label, "Wednesday, 30 September 2026");
        assert_eq!(c.date_runs(&ids, SortKey::CaptureDate, GroupBy::Auto), day);
        assert_eq!(
            shape(&c.date_runs(&ids[..5], SortKey::CaptureDate, GroupBy::Month)),
            vec![("2026-09".into(), 0, 3), ("2026-08".into(), 3, 1), ("2025-12".into(), 4, 1)]
        );
        assert_eq!(shape(&c.date_runs(&ids[..5], SortKey::CaptureDate, GroupBy::Year)), vec![("2026".into(), 0, 4), ("2025".into(), 4, 1)]);
        assert!(c.date_runs(&ids, SortKey::FileName, GroupBy::Day).is_empty());
        assert!(c.date_runs(&ids, SortKey::CaptureDate, GroupBy::None).is_empty());
        // edit date: unedited photos have no date
        let r = c.date_runs(&ids, SortKey::EditDate, GroupBy::Day);
        assert_eq!(shape(&r), vec![(String::new(), 0, 6)]);
        assert_eq!(r[0].label, "Unknown Date");
        // a stack spanning midnight stays in its first member's group
        let op = c.group_ops(ids[1], &[ids[1], ids[2]], false).unwrap();
        c.apply(op).unwrap();
        let arranged = c.arrange_stacks(&ids[..5]);
        assert_eq!(
            shape(&c.date_runs(&arranged, SortKey::CaptureDate, GroupBy::Day))[..2],
            [("2026-09-30".into(), 0, 3), ("2026-08-01".into(), 3, 1)]
        );
    }
}
