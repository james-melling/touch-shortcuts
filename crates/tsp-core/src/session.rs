//! One edit session on one pad file: the current document, revision,
//! selection, undo history and save state. Pure apart from the [`Fs`] passed
//! in; the app wraps one `Session` in one mutex and never holds it across
//! window calls.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::edit::{self, EditError, History, Op, StepKind, step_kind};
use crate::profile::{Loaded, load_doc, load_profile, to_text};
use crate::store::{self, Fs, Identity, SaveError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Selection {
    None,
    /// The header: pad settings, no cell.
    Pad,
    Cell { x: u32, y: u32 },
    Button { id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveState {
    Saved,
    Pending,
    Failed(String),
    /// The file changed on disk; edits are paused until Keep or Load.
    Conflict,
}

pub struct Session {
    pub loaded: Loaded,
    pub path: PathBuf,
    pub rev: u64,
    pub selection: Selection,
    pub history: History,
    pub save_state: SaveState,
    begin: Value,
    identity: Option<Identity>,
    needs_backup: bool,
    allow_system_keys: bool,
    /// Edits since the last successful save (for "discard N edits").
    unsaved_ops: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpResult {
    pub rev: u64,
    pub changed: bool,
    pub notice: Option<String>,
}

fn paused() -> EditError {
    EditError { code: "profile-changed-on-disk", message: SaveError::ChangedOnDisk.message(std::path::Path::new("")), button: None }
}

impl Session {
    /// Starts a session on a loaded pad. `identity` is what was on disk at load.
    pub fn begin(loaded: Loaded, path: PathBuf, identity: Option<Identity>, allow_system_keys: bool) -> Self {
        let needs_backup = loaded.source_schema == 1;
        let save_state = if loaded.normalised.is_empty() { SaveState::Saved } else { SaveState::Pending };
        Session {
            begin: loaded.doc.clone(),
            loaded,
            path,
            rev: 0,
            selection: Selection::None,
            history: History::default(),
            save_state,
            identity,
            needs_backup,
            allow_system_keys,
            unsaved_ops: 0,
        }
    }

    pub fn dirty(&self) -> bool {
        !matches!(self.save_state, SaveState::Saved)
    }

    pub fn unsaved_ops(&self) -> u32 {
        self.unsaved_ops
    }

    fn check_rev(&self, base_rev: u64) -> Result<(), EditError> {
        if base_rev != self.rev {
            return Err(EditError { code: "edit-stale", message: "edit-stale: the pad changed since this view. Try again.".into(), button: None });
        }
        if self.save_state == SaveState::Conflict {
            return Err(paused());
        }
        Ok(())
    }

    fn adopt(&mut self, loaded: Loaded) {
        self.loaded = loaded;
        self.rev += 1;
        self.unsaved_ops += 1;
        self.save_state = SaveState::Pending;
    }

    /// Applies an op made against `base_rev`. Refused ops change nothing.
    pub fn apply(&mut self, op: &Op, base_rev: u64, now_ms: u64) -> Result<OpResult, EditError> {
        self.check_rev(base_rev)?;
        let applied = edit::apply(&self.loaded, op, self.allow_system_keys).inspect_err(|e| {
            if let Some(id) = &e.button {
                self.selection = Selection::Button { id: id.clone() };
            }
        })?;
        if applied.changed {
            self.history.record(self.loaded.doc.clone(), step_kind(op), op.button_id(), now_ms);
            self.adopt(applied.loaded);
        }
        self.selection = match (&applied.select, op) {
            (Some(id), _) => Selection::Button { id: id.clone() },
            (None, Op::Delete { .. }) => Selection::None,
            _ => self.selection.clone(),
        };
        Ok(OpResult { rev: self.rev, changed: applied.changed, notice: applied.notice })
    }

    fn restore(&mut self, doc: Value) -> Result<(), EditError> {
        let loaded = load_doc(doc, self.allow_system_keys).map_err(|e| EditError { code: e.code, message: e.message, button: None })?;
        if let Selection::Button { id } = &self.selection
            && !loaded.profile.buttons.iter().any(|b| &b.id == id)
        {
            self.selection = Selection::None;
        }
        self.adopt(loaded);
        Ok(())
    }

    pub fn undo(&mut self, base_rev: u64) -> Result<bool, EditError> {
        self.check_rev(base_rev)?;
        match self.history.undo(self.loaded.doc.clone()) {
            Some(doc) => self.restore(doc).map(|_| true),
            None => Ok(false),
        }
    }

    pub fn redo(&mut self, base_rev: u64) -> Result<bool, EditError> {
        self.check_rev(base_rev)?;
        match self.history.redo(self.loaded.doc.clone()) {
            Some(doc) => self.restore(doc).map(|_| true),
            None => Ok(false),
        }
    }

    /// The delete notice's Undo: only while that delete is the latest step.
    pub fn undo_delete(&mut self, id: &str, base_rev: u64) -> Result<bool, EditError> {
        if !self.history.last_is_delete_of(id) {
            return Ok(false);
        }
        self.undo(base_rev)
    }

    /// "Revert to when I tapped Edit": one undo step back to the start.
    pub fn revert(&mut self, base_rev: u64, now_ms: u64) -> Result<bool, EditError> {
        self.check_rev(base_rev)?;
        if self.loaded.doc == self.begin {
            return Ok(false);
        }
        self.history.record(self.loaded.doc.clone(), StepKind::Other, None, now_ms);
        let begin = self.begin.clone();
        self.restore(begin).map(|_| true)
    }

    pub fn select(&mut self, selection: Selection) {
        self.selection = selection;
    }

    /// Writes pending edits. A file changed on disk moves the session into
    /// `Conflict` (edits paused); other failures keep the edits in memory.
    pub fn save(&mut self, fs: &mut dyn Fs) -> Result<(), SaveError> {
        if !self.dirty() || self.save_state == SaveState::Conflict {
            return if self.save_state == SaveState::Conflict { Err(SaveError::ChangedOnDisk) } else { Ok(()) };
        }
        self.write(fs, true)
    }

    fn write(&mut self, fs: &mut dyn Fs, check: bool) -> Result<(), SaveError> {
        let text = to_text(&self.loaded.doc);
        let expected = if check { self.identity.as_ref() } else { None };
        match store::save(fs, &self.path, &text, expected, self.needs_backup) {
            Ok(id) => {
                self.identity = Some(id);
                self.needs_backup = false;
                self.save_state = SaveState::Saved;
                self.unsaved_ops = 0;
                Ok(())
            }
            Err(SaveError::ChangedOnDisk) => {
                self.save_state = SaveState::Conflict;
                Err(SaveError::ChangedOnDisk)
            }
            Err(e) => {
                self.save_state = SaveState::Failed(e.message(&self.path));
                Err(e)
            }
        }
    }

    /// Conflict choice "Keep my edits (overwrite file)".
    pub fn keep_mine(&mut self, fs: &mut dyn Fs) -> Result<(), SaveError> {
        self.write(fs, false)
    }

    /// Conflict choice "Load file (discard N edits)". A file that fails to
    /// load keeps the in-memory pad and leaves the conflict open.
    pub fn load_file(&mut self, fs: &mut dyn Fs) -> Result<(), EditError> {
        let bytes = fs.read(&self.path).map_err(|e| EditError { code: "profile-syntax", message: format!("profile-syntax {}: cannot read file: {e}", self.path.display()), button: None })?;
        let loaded = load_profile(&bytes, self.allow_system_keys).map_err(|e| EditError { code: e.code, message: format!("{} {}: {}", e.code, self.path.display(), e.message), button: None })?;
        self.identity = store::identity(fs, &self.path).ok();
        self.needs_backup = loaded.source_schema == 1;
        self.begin = loaded.doc.clone();
        self.loaded = loaded;
        self.history.clear();
        self.rev += 1;
        self.unsaved_ops = 0;
        self.save_state = SaveState::Saved;
        self.selection = Selection::None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{ActionSpec, Op};
    use crate::store::fake::FakeFs;
    use std::path::Path;

    const P: &str = "/cfg/pad.tsp.json";
    const V1: &str = r#"{"schema":1,"name":"T","pad":{"gridColumns":4,"buttonWidth":92,"buttonHeight":78,"gap":8},"buttons":[
        {"id":"a","label":"A","action":{"type":"keys","combo":"A"}},{"id":"b","label":"B","action":{"type":"keys","combo":"B"}}]}"#;

    fn start(fs: &mut FakeFs, text: &str) -> Session {
        fs.put(P, text);
        let loaded = load_profile(text.as_bytes(), false).unwrap();
        let id = store::identity(fs, Path::new(P)).ok();
        Session::begin(loaded, PathBuf::from(P), id, false)
    }

    fn label(id: &str, l: &str) -> Op {
        Op::SetLabel { button_id: id.into(), label: l.into() }
    }

    #[test]
    fn ops_bump_rev_and_stale_revs_are_refused() {
        let mut fs = FakeFs::default();
        let mut s = start(&mut fs, V1);
        let r = s.apply(&label("a", "Aa"), 0, 0).unwrap();
        assert_eq!(r.rev, 1);
        assert_eq!(s.apply(&label("a", "Ab"), 0, 10).unwrap_err().code, "edit-stale");
        assert_eq!(s.loaded.profile.buttons[0].label, "Aa");
    }

    #[test]
    fn a_label_commit_after_the_selection_moved_still_targets_its_button() {
        let mut fs = FakeFs::default();
        let mut s = start(&mut fs, V1);
        s.select(Selection::Button { id: "b".into() });
        s.apply(&label("a", "late commit"), 0, 0).unwrap();
        assert_eq!(s.loaded.profile.buttons[0].label, "late commit");
        assert_eq!(s.loaded.profile.buttons[1].label, "B");
    }

    #[test]
    fn first_save_of_a_v1_pad_backs_it_up_then_saves_v2() {
        let mut fs = FakeFs::default();
        let mut s = start(&mut fs, V1);
        s.apply(&label("a", "Aa"), 0, 0).unwrap();
        s.save(&mut fs).unwrap();
        assert_eq!(fs.text("/cfg/pad.tsp.json.v1.bak").unwrap(), V1);
        assert!(fs.text(P).unwrap().contains("\"schema\": 2"));
        s.apply(&label("a", "Ab"), 1, 10_000).unwrap();
        s.save(&mut fs).unwrap();
        assert!(fs.text("/cfg/pad.tsp.json.v1.1.bak").is_none(), "backup happens once");
        assert!(!s.dirty());
    }

    #[test]
    fn save_then_op_then_save_needs_no_conflict() {
        let mut fs = FakeFs::default();
        let mut s = start(&mut fs, V1);
        for (i, l) in ["1", "2", "3"].iter().enumerate() {
            s.apply(&label("a", l), i as u64, i as u64 * 5000).unwrap();
            s.save(&mut fs).unwrap();
        }
    }

    #[test]
    fn a_hand_edit_pauses_editing_until_keep_or_load() {
        let mut fs = FakeFs::default();
        let mut s = start(&mut fs, V1);
        s.apply(&label("a", "Mine"), 0, 0).unwrap();
        fs.put(P, &V1.replace("\"A\"", "\"Theirs\""));
        assert_eq!(s.save(&mut fs), Err(SaveError::ChangedOnDisk));
        assert_eq!(s.save_state, SaveState::Conflict);
        assert_eq!(s.apply(&label("b", "x"), 1, 1).unwrap_err().code, "profile-changed-on-disk");
        assert_eq!(s.unsaved_ops(), 1);

        // Keep my edits.
        let mut keep = s;
        keep.keep_mine(&mut fs).unwrap();
        assert!(fs.text(P).unwrap().contains("Mine"));
        assert_eq!(keep.save_state, SaveState::Saved);
    }

    #[test]
    fn load_file_discards_edits_and_clears_undo() {
        let mut fs = FakeFs::default();
        let mut s = start(&mut fs, V1);
        s.apply(&label("a", "Mine"), 0, 0).unwrap();
        fs.put(P, &V1.replace("\"A\"", "\"Theirs\""));
        let _ = s.save(&mut fs);
        s.load_file(&mut fs).unwrap();
        assert_eq!(s.loaded.profile.buttons[0].label, "Theirs");
        assert!(!s.history.can_undo());
        assert_eq!(s.save_state, SaveState::Saved);
        // Editing resumes against the new file identity.
        let rev = s.rev;
        s.apply(&label("a", "Next"), rev, 10).unwrap();
        s.save(&mut fs).unwrap();
    }

    #[test]
    fn a_failed_load_file_keeps_the_pad_and_the_conflict() {
        let mut fs = FakeFs::default();
        let mut s = start(&mut fs, V1);
        s.apply(&label("a", "Mine"), 0, 0).unwrap();
        fs.put(P, "{ broken");
        let _ = s.save(&mut fs);
        assert_eq!(s.load_file(&mut fs).unwrap_err().code, "profile-syntax");
        assert_eq!(s.loaded.profile.buttons[0].label, "Mine");
        assert_eq!(s.save_state, SaveState::Conflict);
    }

    #[test]
    fn a_failed_write_keeps_edits_and_retries_on_the_next_save() {
        let mut fs = FakeFs::default();
        let mut s = start(&mut fs, V1);
        s.apply(&label("a", "Mine"), 0, 0).unwrap();
        fs.fail_write = true;
        assert_eq!(s.save(&mut fs).unwrap_err().code(), "profile-save-failed");
        assert!(matches!(s.save_state, SaveState::Failed(_)));
        assert_eq!(s.loaded.profile.buttons[0].label, "Mine");
        fs.fail_write = false;
        s.save(&mut fs).unwrap();
        assert!(fs.text(P).unwrap().contains("Mine"));
    }

    #[test]
    fn undo_redo_revert_and_the_delete_notice() {
        let mut fs = FakeFs::default();
        let mut s = start(&mut fs, V1);
        s.apply(&Op::Delete { button_id: "b".into() }, 0, 0).unwrap();
        assert_eq!(s.selection, Selection::None);
        assert!(s.undo_delete("b", 1).unwrap());
        assert_eq!(s.loaded.profile.buttons.len(), 2);
        s.apply(&Op::Delete { button_id: "b".into() }, 2, 10).unwrap();
        s.apply(&label("a", "x"), 3, 20).unwrap();
        assert!(!s.undo_delete("b", 4).unwrap(), "a later op disarms the notice");
        assert!(s.undo(4).unwrap());
        assert!(s.redo(5).unwrap());
        assert!(s.revert(6, 30).unwrap());
        assert_eq!(s.loaded.doc, load_profile(V1.as_bytes(), false).unwrap().doc);
        assert!(s.undo(7).unwrap(), "revert is one undo step");
        assert_eq!(s.loaded.profile.buttons.len(), 1);
    }

    #[test]
    fn a_refusal_naming_a_button_selects_it() {
        let mut fs = FakeFs::default();
        let mut s = start(&mut fs, V1);
        let e = s.apply(&Op::SetAction { button_id: "a".into(), action: ActionSpec::Keys { combo: "Win+R".into(), repeat: 1 } }, 0, 0).unwrap_err();
        assert_eq!(e.code, "system-key-blocked");
        assert_eq!(s.selection, Selection::Button { id: "a".into() });
        assert_eq!(s.rev, 0);
    }

    #[test]
    fn a_normalised_load_is_pending_and_written_on_the_next_save() {
        let mut fs = FakeFs::default();
        let text = r#"{"schema":2,"name":"T","pad":{"grid":{"columns":2,"rows":2},"cellWidth":40,"cellHeight":40,"gap":0,"backgroundOpacity":0.02}}"#;
        let mut s = start(&mut fs, text);
        assert!(s.dirty());
        s.save(&mut fs).unwrap();
        assert!(fs.text(P).unwrap().contains("\"backgroundOpacity\": 0"));
    }
}
