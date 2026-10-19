use serde_json::json;

use super::{CommandSpec, can_redo, can_undo, cmd};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("edit.undo", "Undo", ["Edit"], Some("Cmd+Z"), "{}", can_undo, |s, _| {
            s.end_interaction()?;
            Ok(json!({"undone": s.undo_step()?}))
        }),
        cmd!(
            "edit.redo",
            "Redo",
            ["Edit"],
            Some("Cmd+Shift+Z"),
            "{} — finish any drag first; a changed drag clears Redo → {redone: label|null}",
            can_redo,
            |s, _| {
                // A changed drag is a new edit: finish it before attempting the old redo branch.
                s.end_interaction()?;
                if s.redo.is_empty() {
                    // The completed edit is still a successful command, so it is saved and its
                    // sidecars are written; no old history step was redone.
                    return Ok(json!({"redone": null}));
                }
                Ok(json!({"redone": s.redo_step()?}))
            }
        ),
    ]
}
