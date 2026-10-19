//! Library-wide keyword operations: the keyword tree (hierarchical keywords are written
//! `parent|child|grandchild`), rename / delete / merge across every photo, and suggestions.
//!
//! Rename, delete and merge produce one [`Op::Batch`] of [`Op::SetMeta`]s — one per photo that
//! changes — so they are a single undo step and replay from the op log like any other edit.
//! Keyword names compare case-insensitively; renaming a keyword renames its children too
//! (`travel|italy` → `trips|italy`).

use serde::Serialize;

use crate::{Catalog, CatalogError, Meta, Op, Result};

/// Separator of hierarchical keyword levels.
pub const SEP: char = '|';

/// Nonempty, trimmed hierarchy levels: shared by identity, tree paths and matching.
fn components(k: &str) -> impl Iterator<Item = &str> {
    k.split(SEP).map(str::trim).filter(|s| !s.is_empty())
}

/// Normalize a display spelling without changing the case of any level.
pub fn clean(k: &str) -> String {
    components(k).collect::<Vec<_>>().join("|")
}

/// Case-insensitive keyword identity, including legacy spaced/empty hierarchy levels.
pub fn key(k: &str) -> String {
    components(k).map(str::to_lowercase).collect::<Vec<_>>().join("|")
}

/// The raw suffix after matching normalized hierarchy levels. Consume boundaries from `k`,
/// so expanding Unicode lowercase and legacy whitespace never become byte offsets into it.
fn suffix_under<'a>(k: &'a str, parent: &str) -> Option<&'a str> {
    let mut rest = k;
    let mut suffix = None;
    for level in components(parent) {
        loop {
            let end = rest.find(SEP).unwrap_or(rest.len());
            let component = rest.get(..end)?.trim();
            let tail = rest.get(end..)?;
            rest = tail.strip_prefix(SEP).unwrap_or("");
            if component.is_empty() {
                if tail.is_empty() {
                    return None;
                }
                continue;
            }
            if component.to_lowercase() != level.to_lowercase() {
                return None;
            }
            suffix = Some(tail);
            break;
        }
    }
    suffix
}

/// `k` is `parent` or one of its descendants, ignoring case and empty/spaced levels.
pub fn is_under(k: &str, parent: &str) -> bool {
    suffix_under(k, parent).is_some()
}

/// Replace a hierarchy prefix, preserving the raw child suffix and unrelated spellings.
/// An identical display prefix is a no-op even for legacy spaced keywords.
pub fn reparent(k: &str, from: &str, to: &str) -> Option<String> {
    let rest = suffix_under(k, from)?;
    let (from, to) = (clean(from), clean(to));
    Some(if from == to {
        k.to_string()
    } else if to.is_empty() {
        rest.trim_start_matches(SEP).to_string()
    } else {
        format!("{to}{rest}")
    })
}

/// Collapse collisions only for identities rewritten by the requested rename/merge.
/// Unrelated legacy duplicates, empty keywords and original spelling bytes stay untouched.
fn remap_keywords(kws: &[String], f: impl Fn(&str) -> Option<String>) -> Vec<String> {
    let mut affected = std::collections::HashSet::new();
    let mut out: Vec<String> = kws
        .iter()
        .map(|k| match f(k) {
            Some(next) if next != *k => {
                affected.insert(key(&next));
                next
            }
            _ => k.clone(),
        })
        .collect();
    let mut seen = std::collections::HashSet::new();
    out.retain(|k| {
        let identity = key(k);
        !affected.contains(&identity) || seen.insert(identity)
    });
    out
}

/// One node of the keyword tree.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct KeywordNode {
    /// This level's name (`italy`).
    pub name: String,
    /// The full keyword (`travel|italy`).
    pub path: String,
    /// Photos (not deleted) with this keyword or one below it.
    pub count: usize,
    pub children: Vec<KeywordNode>,
}

impl Catalog {
    /// The keyword tree: every keyword level with photo counts, sorted by name (case-insensitive).
    pub fn keyword_tree(&self) -> Vec<KeywordNode> {
        use std::collections::{HashMap, HashSet};
        // every keyword level (`travel`, `travel|italy`…) by its lower-case path: the name and
        // path as first written, and the photos counted (once per photo, even when two of its
        // keywords share a parent)
        struct Level {
            name: String,
            path: String,
            count: usize,
            parent: Option<String>,
        }
        let mut levels: HashMap<String, Level> = HashMap::new();
        let mut this_photo: HashSet<String> = HashSet::new();
        for p in self.photos().filter(|p| p.in_library()) {
            this_photo.clear();
            for k in &p.meta.keywords {
                let mut path = String::new();
                let mut lower = String::new();
                let mut parent: Option<String> = None;
                for part in components(k) {
                    if !path.is_empty() {
                        path.push(SEP);
                        lower.push(SEP);
                    }
                    path.push_str(part);
                    lower.push_str(&part.to_lowercase());
                    let l = levels.entry(lower.clone()).or_insert_with(|| Level {
                        name: part.to_string(),
                        path: path.clone(),
                        count: 0,
                        parent: parent.clone(),
                    });
                    if this_photo.insert(lower.clone()) {
                        l.count += 1;
                    }
                    parent = Some(lower.clone());
                }
            }
        }
        let mut kids: HashMap<Option<String>, Vec<String>> = HashMap::new();
        for (lower, l) in &levels {
            kids.entry(l.parent.clone()).or_default().push(lower.clone());
        }
        fn build(at: Option<String>, levels: &HashMap<String, Level>, kids: &mut HashMap<Option<String>, Vec<String>>) -> Vec<KeywordNode> {
            let Some(mut list) = kids.remove(&at) else { return Vec::new() };
            list.sort_by_key(|l| levels[l].name.to_lowercase());
            list.into_iter()
                .map(|l| {
                    let lv = &levels[&l];
                    let children = build(Some(l.clone()), levels, kids);
                    KeywordNode { name: lv.name.clone(), path: lv.path.clone(), count: lv.count, children }
                })
                .collect()
        }
        build(None, &levels, &mut kids)
    }

    /// `SetMeta` ops for every photo whose keywords `f` changes.
    fn keyword_ops(&self, f: impl Fn(&[String]) -> Vec<String>) -> Vec<Op> {
        self.photos()
            .filter_map(|p| {
                let kws = f(&p.meta.keywords);
                (kws != p.meta.keywords).then(|| Op::SetMeta { id: p.id, meta: Box::new(Meta { keywords: kws, ..p.meta.clone() }) })
            })
            .collect()
    }

    /// Rename `from` (and the keywords below it) to `to` on every photo — one batch. Renaming onto
    /// an existing keyword merges the two.
    pub fn rename_keyword_ops(&self, from: &str, to: &str) -> Result<Op> {
        let (from, to) = (clean(from), clean(to));
        if from.is_empty() || to.is_empty() {
            return Err(CatalogError::Invalid("keyword names can't be empty".into()));
        }
        if suffix_under(&to, &from).is_some_and(|rest| !rest.is_empty()) {
            return Err(CatalogError::Invalid("can't move a keyword below itself".into()));
        }
        let ops = self.keyword_ops(|kws| remap_keywords(kws, |k| reparent(k, &from, &to)));
        Ok(Op::Batch { ops })
    }

    /// Remove `keyword` and the keywords below it from every photo.
    pub fn delete_keyword_ops(&self, keyword: &str) -> Result<Op> {
        let k = clean(keyword);
        if k.is_empty() {
            return Err(CatalogError::Invalid("empty keyword".into()));
        }
        Ok(Op::Batch { ops: self.keyword_ops(|kws| kws.iter().filter(|x| !is_under(x, &k)).cloned().collect()) })
    }

    /// Merge several keywords (with their children) into `into`.
    pub fn merge_keywords_ops(&self, from: &[String], into: &str) -> Result<Op> {
        let into = clean(into);
        let into_key = key(&into);
        let from: Vec<String> = from.iter().map(|f| clean(f)).filter(|f| !f.is_empty() && key(f) != into_key).collect();
        if into.is_empty() || from.is_empty() {
            return Err(CatalogError::Invalid("merge needs keywords and a target".into()));
        }
        if from.iter().any(|f| is_under(&into, f)) {
            return Err(CatalogError::Invalid("can't merge a keyword into one below it".into()));
        }
        let ops = self.keyword_ops(|kws| remap_keywords(kws, |k| from.iter().find_map(|f| reparent(k, f, &into))));
        Ok(Op::Batch { ops })
    }

    /// Keyword suggestions for a photo that has `current` keywords: with a typed `prefix`, the
    /// library's keywords containing it (those starting with it first); otherwise keywords that
    /// appear together with `current` on other photos, then the most used ones. Most frequent
    /// first, at most `n`, never one of `current`.
    pub fn keyword_suggestions(&self, current: &[String], prefix: &str, n: usize) -> Vec<String> {
        let all = self.keywords();
        let current_keys: std::collections::HashSet<String> = current.iter().map(|k| key(k)).filter(|k| !k.is_empty()).collect();
        let has = |k: &str| current_keys.contains(&key(k));
        let q = key(prefix);
        let mut scored: Vec<((usize, usize), String)> = if !q.is_empty() {
            all.into_iter()
                .filter(|(k, _)| !has(k))
                .filter_map(|(k, c)| {
                    let l = key(&k);
                    let leaf = l.rsplit(SEP).next().unwrap_or(&l);
                    let rank = if l.starts_with(&q) || leaf.starts_with(&q) {
                        2
                    } else if l.contains(&q) {
                        1
                    } else {
                        return None;
                    };
                    Some(((rank, c), k))
                })
                .collect()
        } else {
            let mut co: std::collections::HashMap<String, usize> = Default::default();
            if !current_keys.is_empty() {
                for p in self.photos().filter(|p| p.in_library()) {
                    let keys: std::collections::HashSet<String> = p.meta.keywords.iter().map(|k| key(k)).filter(|k| !k.is_empty()).collect();
                    if keys.iter().any(|k| current_keys.contains(k)) {
                        for k in keys.into_iter().filter(|k| !current_keys.contains(k)) {
                            *co.entry(k).or_default() += 1;
                        }
                    }
                }
            }
            all.into_iter().filter(|(k, _)| !has(k)).map(|(k, c)| ((co.get(&key(&k)).copied().unwrap_or(0), c), k)).collect()
        };
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| key(&a.1).cmp(&key(&b.1))));
        scored.into_iter().take(n).map(|(_, k)| k).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Filter, Photo, PhotoId, Source};

    fn lib(kws: &[&[&str]]) -> (Catalog, Vec<PhotoId>) {
        let mut c = Catalog::new();
        let mut ids = Vec::new();
        for k in kws {
            let id = c.alloc_photo_id();
            let mut p = Photo::new(id, Source::Demo { scene: 1 }, "a.jpg", "JPEG", 3, 2, "2026-01-01");
            p.meta.keywords = k.iter().map(|s| s.to_string()).collect();
            c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
            ids.push(id);
        }
        (c, ids)
    }
    fn kws(c: &Catalog, id: PhotoId) -> Vec<String> {
        c.photo(id).unwrap().meta.keywords.clone()
    }

    #[test]
    fn tree_counts_photos_per_level() {
        let (c, _) = lib(&[&["travel|Italy|Rome", "beach"], &["Travel|italy"], &["travel|France", "travel|italy|rome"], &["beach"]]);
        let t = c.keyword_tree();
        assert_eq!(t.iter().map(|n| (n.name.as_str(), n.count)).collect::<Vec<_>>(), vec![("beach", 2), ("travel", 3)]);
        let travel = &t[1];
        assert_eq!(travel.children.iter().map(|n| (n.path.as_str(), n.count)).collect::<Vec<_>>(), vec![("travel|France", 1), ("travel|Italy", 3)]);
        assert_eq!(travel.children[1].children[0].path, "travel|Italy|Rome");
        assert_eq!(travel.children[1].children[0].count, 2);
        // filtering by a parent finds its children
        let f = Filter { keyword: Some("travel|italy".into()), ..Default::default() };
        assert_eq!(c.query(&f, &Default::default()).len(), 3);
    }

    #[test]
    fn rename_delete_merge_are_single_undoable_batches() {
        let (mut c, ids) = lib(&[&["travel|italy|rome", "beach"], &["Travel|Italy"], &["italia", "travel|italy"], &["sea"]]);
        let before = c.to_snapshot();
        let op = c.rename_keyword_ops("travel|italy", "Europe|Italy").unwrap();
        let Op::Batch { ops } = &op else { panic!() };
        assert_eq!(ops.len(), 3, "only photos that change");
        let inv = c.apply(op).unwrap();
        assert_eq!(kws(&c, ids[0]), ["Europe|Italy|rome", "beach"]);
        assert_eq!(kws(&c, ids[1]), ["Europe|Italy"]);
        c.apply(inv).unwrap();
        assert_eq!(c.to_snapshot(), before);
        // merge `italia` into the existing hierarchical keyword: duplicates collapse
        let op = c.merge_keywords_ops(&["italia".into()], "travel|italy").unwrap();
        c.apply(op).unwrap();
        assert_eq!(kws(&c, ids[2]), ["travel|italy"]);
        // delete removes the keyword and its children everywhere
        let op = c.delete_keyword_ops("TRAVEL").unwrap();
        c.apply(op).unwrap();
        assert_eq!(kws(&c, ids[0]), ["beach"]);
        assert!(kws(&c, ids[1]).is_empty());
        assert_eq!(c.keywords(), vec![("beach".to_string(), 1), ("sea".to_string(), 1)]);
        // invalid requests
        assert!(c.rename_keyword_ops("beach", " ").is_err());
        assert!(c.rename_keyword_ops("a", "a|b").is_err());
        assert!(c.merge_keywords_ops(&["a".into()], "a|b").is_err());
        assert_eq!(clean(" a | |b "), "a|b");
    }

    #[test]
    fn unicode_hierarchy_reparent_preserves_component_boundaries() {
        for (from, spelling) in [("STRAẞE", "Straße"), ("Straße", "STRAẞE"), ("DACH|STRAẞE", "Dach|Straße")] {
            assert!(is_under(spelling, from));
            assert_eq!(reparent(spelling, from, "Wege"), Some("Wege".into()));
            assert_eq!(reparent(&format!("{spelling}|Berlin|Mitte"), from, "Wege"), Some("Wege|Berlin|Mitte".into()));
            assert_eq!(reparent(&format!("{spelling}|Berlin"), from, ""), Some("Berlin".into()));
            assert!(!is_under(&format!("{spelling}nfest|Berlin"), from));
            assert_eq!(reparent(&format!("{spelling}nfest|Berlin"), from, "Wege"), None);
        }
    }

    #[test]
    fn unicode_case_only_rename_preserves_children_and_rejects_descendants() {
        for (from, to) in [("STRAẞE", "Straße"), ("Straße", "STRAẞE"), ("DACH|STRAẞE", "Dach|Straße")] {
            let first = format!("{from}|Berlin");
            let second = format!("{to}|Berlin|Mitte");
            let (mut c, ids) = lib(&[&[first.as_str()], &[second.as_str()]]);
            let before = c.to_snapshot();
            let op = c.rename_keyword_ops(from, to).unwrap();
            let undo = c.apply(op).unwrap();
            assert_eq!(kws(&c, ids[0]), [format!("{to}|Berlin")]);
            assert_eq!(kws(&c, ids[1]), [second]);
            let renamed = c.to_snapshot();
            let redo = c.apply(undo).unwrap();
            assert_eq!(c.to_snapshot(), before);
            c.apply(redo).unwrap();
            assert_eq!(c.to_snapshot(), renamed);
            for descendant in [format!("{from}|Berlin"), format!("{to}|Berlin")] {
                assert!(c.rename_keyword_ops(from, &descendant).is_err());
                assert_eq!(c.to_snapshot(), renamed, "a rejected move leaves metadata unchanged");
            }
        }
    }

    #[test]
    fn unicode_rename_merge_round_trip_exact_keyword_spellings() {
        for merge in [false, true] {
            let (mut c, ids) = lib(&[&["STRAẞE|Berlin", "Wege|Berlin", "Straßenfest|Berlin"], &["Straße|Berlin|Mitte"]]);
            let before = c.to_snapshot();
            let op =
                if merge { c.merge_keywords_ops(&["STRAẞE".into()], "Wege").unwrap() } else { c.rename_keyword_ops("STRAẞE", "Wege").unwrap() };
            let undo = c.apply(op).unwrap();
            assert_eq!(kws(&c, ids[0]), ["Wege|Berlin", "Straßenfest|Berlin"]);
            assert_eq!(kws(&c, ids[1]), ["Wege|Berlin|Mitte"]);
            let after = c.to_snapshot();
            let redo = c.apply(undo).unwrap();
            assert_eq!(c.to_snapshot(), before);
            c.apply(redo).unwrap();
            assert_eq!(c.to_snapshot(), after);
            let f = Filter { keyword: Some("Wege|Berlin".into()), ..Default::default() };
            assert_eq!(c.query(&f, &Default::default()).len(), 2);
        }
    }

    #[test]
    fn suggestions_rank_co_occurrence_and_prefixes() {
        let (c, _) = lib(&[&["dog", "park"], &["dog", "park", "ball"], &["dog", "beach"], &["cat"], &["cat"], &["cat"], &["parade"], &["eagle"]]);
        // co-occurring with `dog` first (park twice), then the most used
        let s = c.keyword_suggestions(&["dog".into()], "", 3);
        assert_eq!(s, ["park", "ball", "beach"]);
        let s = c.keyword_suggestions(&[], "", 2);
        assert_eq!(s, ["cat", "dog"]);
        let s = c.keyword_suggestions(&["park".into()], "pa", 5);
        assert_eq!(s, ["parade"]);
        let s = c.keyword_suggestions(&[], "E", 5);
        assert_eq!(s, ["eagle", "beach", "parade"], "prefix matches before substring matches");
    }

    #[test]
    fn legacy_spaced_hierarchies_match_the_counted_tree_without_migration() {
        let raw = "  旅行 || CAFÉ | Straße  | ";
        let (c, _) = lib(&[&[raw], &["旅行|café|STRAẞE"], &["旅行house|CAFÉ"]]);
        let before = c.to_snapshot();
        let tree = c.keyword_tree();
        let parent = tree.iter().find(|n| n.path == "旅行").unwrap();
        let leaf = &parent.children[0].children[0];
        assert_eq!(leaf.count, 2);
        for keyword in [parent.path.as_str(), leaf.path.as_str(), " 旅行 | |café | straße "] {
            let f = Filter { keyword: Some(keyword.into()), ..Default::default() };
            assert_eq!(c.query(&f, &Default::default()).len(), 2);
        }
        assert!(!is_under(raw, "旅行house"));
        assert!(!is_under(raw, " | "));
        assert_eq!(c.to_snapshot(), before, "reads never normalize stored bytes");
        assert_eq!(reparent(" X | İ || Child ", " x || i\u{307} ", " Y "), Some("Y|| Child ".into()));
    }

    #[test]
    fn legacy_rename_merge_delete_preserve_unrelated_duplicates_and_exact_undo() {
        for action in ["rename", "merge", "delete"] {
            let original = [" Old || Café ", "Target|CAFÉ", "Other ", "other", ""];
            let unrelated = ["  Another | Tag ", "another|tag", ""];
            let (mut c, ids) = lib(&[&original, &unrelated]);
            let before = c.to_snapshot();
            let op = match action {
                "rename" => c.rename_keyword_ops(" old ", "Target").unwrap(),
                "merge" => c.merge_keywords_ops(&["old".into()], "Target").unwrap(),
                _ => c.delete_keyword_ops(" old ").unwrap(),
            };
            let Op::Batch { ops } = &op else { panic!() };
            assert_eq!(ops.len(), 1, "the unrelated photo is never rewritten");
            let undo = c.apply(op).unwrap();
            let expected =
                if action == "delete" { vec!["Target|CAFÉ", "Other ", "other", ""] } else { vec!["Target|| Café ", "Other ", "other", ""] };
            assert_eq!(kws(&c, ids[0]), expected);
            assert_eq!(kws(&c, ids[1]), unrelated);
            let after = c.to_snapshot();
            let redo = c.apply(undo).unwrap();
            assert_eq!(c.to_snapshot(), before);
            c.apply(redo).unwrap();
            assert_eq!(c.to_snapshot(), after);
        }
    }

    #[test]
    fn absent_and_identical_keyword_actions_preserve_all_legacy_bytes() {
        let (c, _) = lib(&[&["CAFÉ", "café", "  Travel | Coast ", "", " | "]]);
        let before = c.to_snapshot();
        for op in [
            c.rename_keyword_ops("missing", "new").unwrap(),
            c.delete_keyword_ops("missing").unwrap(),
            c.merge_keywords_ops(&["missing".into()], "new").unwrap(),
            c.rename_keyword_ops("Travel", " Travel ").unwrap(),
        ] {
            let Op::Batch { ops } = op else { panic!() };
            assert!(ops.is_empty(), "a no-op cannot clean unrelated metadata");
        }
        assert_eq!(c.to_snapshot(), before);
    }

    #[test]
    fn suggestions_aggregate_unicode_variants_per_photo_with_stable_display() {
        for records in [vec![vec![" café ", "CAFÉ"], vec!["café"], vec!["beach"]], vec![vec!["beach"], vec!["café"], vec!["CAFÉ", " café "]]] {
            let borrowed: Vec<&[&str]> = records.iter().map(Vec::as_slice).collect();
            let (c, _) = lib(&borrowed);
            let before = c.to_snapshot();
            assert_eq!(c.keywords(), [("beach".into(), 1), ("CAFÉ".into(), 2)]);
            assert_eq!(c.keyword_suggestions(&[], "", 1), ["CAFÉ"]);
            assert_eq!(c.keyword_suggestions(&[], "", 12), ["CAFÉ", "beach"]);
            assert_eq!(c.keyword_suggestions(&[], " ca ", 12), ["CAFÉ"]);
            assert_eq!(c.keyword_suggestions(&[" café ".into()], "", 12), ["beach"]);
            assert_eq!(c.keyword_suggestions(&[], "", 0), Vec::<String>::new());
            assert_eq!(c.to_snapshot(), before);
        }
    }

    #[test]
    fn suggestions_count_co_occurrence_once_and_normalize_full_hierarchy_prefixes() {
        let (mut c, ids) = lib(&[
            &["  Target || STRAẞE ", "park", "PARK", " park "],
            &["target|Straße", "ball"],
            &["TARGET | straße", "ball"],
            &["Target|STRAẞE|Berlin"],
            &["targethouse|Straße"],
            &["hidden"],
            &["local"],
        ]);
        c.apply(Op::SetDeleted { id: ids[5], deleted: true }).unwrap();
        c.apply(Op::SetLocal { id: ids[6], local: true }).unwrap();
        assert_eq!(c.keyword_suggestions(&[" target || straße ".into()], "", 2), ["ball", "PARK"]);
        assert_eq!(c.keyword_suggestions(&[], " target | str ", 12), ["TARGET|straße", "Target|STRAẞE|Berlin"]);
        let assigned = c.keyword_suggestions(&["TARGET||STRAẞE".into()], "straße", 12);
        assert!(!assigned.iter().any(|k| is_under(k, "target|straße") && !k.contains("Berlin")));
        assert!(assigned.contains(&"Target|STRAẞE|Berlin".into()), "exact assigned keys do not exclude descendants");
        assert!(c.keywords().iter().all(|(k, _)| k != "hidden" && k != "local"));
    }
}
