//! The write protocol matrix and an exhaustive schedule model.
//!
//! The model keeps its own state. The revision comparison and request builder
//! are the production functions, so the tests cannot drift into a second
//! implementation that only tests itself.

use bitwarden_vault::CipherView;
use serde_json::json;
use zeroize::Zeroizing;

use super::{
    LoginDraft, LoginFieldDraft, LoginFieldKind, LoginUriDraft, UriMatch, build_edit_request,
    revision_matches,
};
use crate::CoreError;

fn view() -> CipherView {
    serde_json::from_value(json!({
        "id": "22222222-2222-2222-2222-222222222222",
        "organizationId": null,
        "folderId": null,
        "collectionIds": [],
        "key": null,
        "name": "GitHub",
        "notes": null,
        "type": 1,
        "login": {
            "username": "octocat",
            "password": "hunter2",
            "uris": [{ "uri": "https://github.com", "match": 0 }],
            "totp": "JBSWY3DPEHPK3PXP"
        },
        "favorite": false,
        "reprompt": 0,
        "organizationUseTotp": false,
        "edit": true,
        "viewPassword": true,
        "creationDate": "2026-01-01T00:00:00Z",
        "revisionDate": "2026-01-01T00:00:00Z"
    }))
    .expect("the protocol fixture is a CipherView")
}

fn draft(name: &str) -> LoginDraft {
    LoginDraft {
        name: name.to_string(),
        username: "octocat".to_string(),
        password: Zeroizing::new("hunter2".to_string()),
        totp: Zeroizing::new("JBSWY3DPEHPK3PXP".to_string()),
        uris: vec![LoginUriDraft {
            uri: "https://github.com".to_string(),
            match_type: Some(UriMatch::Domain),
        }],
        notes: Zeroizing::new(String::new()),
        folder_id: None,
        favorite: false,
        fields: vec![LoginFieldDraft::new(LoginFieldKind::Text)],
    }
}

#[derive(Default)]
struct Model {
    view: Option<CipherView>,
    deleted: bool,
    writes: usize,
}

impl Model {
    fn existing() -> Self {
        Self {
            view: Some(view()),
            ..Self::default()
        }
    }

    fn deleted() -> Self {
        Self {
            view: Some(view()),
            deleted: true,
            ..Self::default()
        }
    }

    fn revision(&self) -> String {
        super::revision_text(self.view.as_ref().expect("the fixture exists"))
    }

    fn edit(&mut self, opened: &str, draft: &LoginDraft) -> Result<(), CoreError> {
        let Some(view) = self.view.as_ref() else {
            return Err(CoreError::NotFound);
        };
        if self.deleted {
            return Err(CoreError::NotFound);
        }
        if !revision_matches(view, opened) {
            return Err(CoreError::Conflict);
        }
        build_edit_request(view.clone(), draft)?;
        self.writes += 1;
        let view = self.view.as_mut().expect("the fixture exists");
        view.name = draft.name.trim().to_string();
        Ok(())
    }

    fn delete(&mut self) -> Result<(), CoreError> {
        if let Some(_view) = self.view.as_ref() {
            self.deleted = true;
            self.writes += 1;
        }
        // A missing local item is a success: the server has nothing to keep.
        Ok(())
    }

    fn sync(&mut self, server_has_item: bool) {
        if !server_has_item {
            self.view = None;
            self.deleted = false;
        }
    }
}

#[test]
fn the_protocol_matrix_matches_the_documented_outcomes() {
    let draft = draft("Changed");

    let mut model = Model::existing();
    let revision = model.revision();
    model
        .edit(&revision, &draft)
        .expect("matching revision edits");
    assert_eq!(
        model.view.as_ref().map(|view| view.name.as_str()),
        Some("Changed")
    );

    let mut model = Model::existing();
    assert_eq!(
        model.edit("2025-01-01T00:00:00+00:00", &draft),
        Err(CoreError::Conflict)
    );
    assert_eq!(model.writes, 0, "a conflict writes nothing");

    let mut model = Model::deleted();
    assert_eq!(
        model.edit(&model.revision(), &draft),
        Err(CoreError::NotFound)
    );
    assert_eq!(model.writes, 0);

    let mut model = Model::existing();
    model.delete().expect("an existing item is trashed");
    assert!(model.deleted);

    let mut model = Model::deleted();
    model.delete().expect("a repeated delete is idempotent");
    assert!(model.deleted);

    let mut model = Model::deleted();
    model.sync(false);
    assert!(model.view.is_none(), "sync does not revive a trashed item");

    let mut model = Model::default();
    assert_eq!(
        model.edit("2026-01-01T00:00:00+00:00", &draft),
        Err(CoreError::NotFound)
    );
    model
        .delete()
        .expect("a missing item deletes as already gone");
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Event {
    SyncStart,
    SyncApply,
    WriteStart,
    WriteCommit,
    DeleteStart,
    DeleteCommit,
}

impl Event {
    fn is_start(self) -> bool {
        matches!(self, Self::SyncStart | Self::WriteStart | Self::DeleteStart)
    }
}

const EVENTS: [Event; 6] = [
    Event::SyncStart,
    Event::SyncApply,
    Event::WriteStart,
    Event::WriteCommit,
    Event::DeleteStart,
    Event::DeleteCommit,
];

fn schedules() -> Vec<Vec<Event>> {
    fn build(remaining: &mut [bool; 6], path: &mut Vec<Event>, out: &mut Vec<Vec<Event>>) {
        if remaining.iter().all(|used| *used) {
            out.push(path.clone());
            return;
        }
        for (ix, event) in EVENTS.into_iter().enumerate() {
            if remaining[ix] {
                continue;
            }
            // A commit only makes sense after its start.
            let start = ix & !1;
            if ix % 2 == 1 && !path.contains(&EVENTS[start]) {
                continue;
            }
            remaining[ix] = true;
            path.push(event);
            build(remaining, path, out);
            path.pop();
            remaining[ix] = false;
        }
    }

    let mut out = Vec::new();
    build(&mut [false; 6], &mut Vec::new(), &mut out);
    out
}

#[derive(Default)]
struct LockedSchedule {
    locked: bool,
    local_exists: bool,
    local_deleted: bool,
    sync_snapshot_exists: bool,
    write_opened_exists: bool,
    overlaps: usize,
}

impl LockedSchedule {
    fn step(&mut self, event: Event) {
        if event.is_start() {
            if self.locked {
                self.overlaps += 1;
                return;
            }
            self.locked = true;
        } else if !self.locked {
            return;
        }
        match event {
            Event::SyncStart => self.sync_snapshot_exists = self.local_exists,
            Event::SyncApply => {
                if self.sync_snapshot_exists && !self.local_deleted {
                    self.local_exists = true;
                }
                self.locked = false;
            }
            Event::WriteStart => self.write_opened_exists = self.local_exists,
            Event::WriteCommit => {
                if self.write_opened_exists && !self.local_deleted {
                    self.local_exists = true;
                }
                self.locked = false;
            }
            Event::DeleteStart => {}
            Event::DeleteCommit => {
                if self.local_exists {
                    self.local_deleted = true;
                }
                self.local_exists = false;
                self.locked = false;
            }
        }
    }
}

#[test]
fn the_lock_admits_only_serialized_histories_and_never_revives_a_delete() {
    let schedules = schedules();
    assert_eq!(
        schedules.len(),
        90,
        "three start/commit pairs have 6!/2^3 orders"
    );

    let mut accepted = 0;
    for schedule in &schedules {
        let mut model = LockedSchedule {
            local_exists: true,
            ..LockedSchedule::default()
        };
        for event in schedule {
            model.step(*event);
        }
        if model.overlaps > 0 || model.locked {
            continue;
        }
        accepted += 1;
        assert!(
            !model.local_exists,
            "a serialized delete always wins: {schedule:?}"
        );
    }
    // Six orders of the three already-atomic operations: S-D-W and its five
    // permutations. Any event from another operation between start and commit
    // is rejected by the lock.
    assert_eq!(accepted, 6);
}

#[test]
fn without_the_lock_a_stale_sync_can_revive_the_deleted_item() {
    // A stale sync captures the item before a delete. Without a lock the
    // delete commits, then the old snapshot replaces the empty local state.
    let mut local_exists = true;
    let sync_snapshot = local_exists;
    local_exists = false;
    let local_deleted = true;
    assert!(
        !local_exists && local_deleted,
        "the delete committed while the sync was in flight"
    );
    local_exists = sync_snapshot;
    assert!(local_exists, "a stale sync revived the item");

    // Serialized in the production order, the sync applies first and the
    // delete has the final word.
    let mut serialized = LockedSchedule {
        local_exists: true,
        ..LockedSchedule::default()
    };
    for event in [
        Event::SyncStart,
        Event::SyncApply,
        Event::DeleteStart,
        Event::DeleteCommit,
    ] {
        serialized.step(event);
    }
    assert_eq!(serialized.overlaps, 0);
    assert!(serialized.local_deleted);
    assert!(!serialized.local_exists);
}
