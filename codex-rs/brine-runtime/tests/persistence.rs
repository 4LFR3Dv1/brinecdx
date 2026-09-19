use std::collections::BTreeMap;

use codex_brine_runtime::AttachRequest;
use codex_brine_runtime::FileRuntimeAuthority;
use codex_brine_runtime::ReconcileRequest;
use codex_brine_runtime::RuntimeAuthority;
use codex_brine_runtime::SessionId;
use codex_brine_runtime::WorkspaceMaterialObservation;
use codex_brine_runtime::WorkspaceStructureObservation;
use tempfile::tempdir;

#[test]
fn authority_survives_session_and_replays_remote_delta() {
    let directory = tempdir().expect("temp directory");
    let path = directory.path().join("runtime.json");
    let first_authority = FileRuntimeAuthority::open(&path).expect("open authority");
    let first = first_authority
        .attach(AttachRequest {
            session_id: SessionId::from("codex-session-1"),
            workspace_key: "repo:brinecdx".to_owned(),
            repository_identity: "git:4LFR3Dv1/brinecdx".to_owned(),
            work_key: "work:runtime-reset".to_owned(),
            objective: "implement R1".to_owned(),
            since_revision: None,
            material: None,
        })
        .expect("attach first session");
    let first_revision = first.state.revision;
    let work_id = first.attachment.work_id.clone();
    drop(first_authority);

    let authority_without_session = FileRuntimeAuthority::open(&path).expect("reopen authority");
    authority_without_session
        .record_remote_delta(
            &work_id,
            "remote runtime advanced while Codex was closed".to_owned(),
        )
        .expect("record remote delta");
    let durable_state = authority_without_session
        .state(
            &first.attachment.workspace_id,
            &work_id,
            Some(first_revision),
        )
        .expect("read state without session");
    assert_eq!(durable_state.pending_deltas.len(), 1);
    drop(authority_without_session);

    let second_authority = FileRuntimeAuthority::open(&path).expect("reopen for attach");
    let second = second_authority
        .attach(AttachRequest {
            session_id: SessionId::from("codex-session-2"),
            workspace_key: "repo:brinecdx".to_owned(),
            repository_identity: "git:4LFR3Dv1/brinecdx".to_owned(),
            work_key: "work:runtime-reset".to_owned(),
            objective: "implement R1".to_owned(),
            since_revision: Some(first_revision),
            material: None,
        })
        .expect("attach second session");

    assert_eq!(
        second.attachment.workspace_id,
        first.attachment.workspace_id
    );
    assert_eq!(second.attachment.work_id, first.attachment.work_id);
    assert_eq!(second.state.pending_deltas.len(), 1);
    assert_eq!(
        second.state.pending_deltas[0].summary,
        "remote runtime advanced while Codex was closed"
    );

    let reconciled = second_authority
        .reconcile(ReconcileRequest {
            session_id: SessionId::from("codex-session-2"),
            workspace_id: second.attachment.workspace_id.clone(),
            work_id: second.attachment.work_id.clone(),
            since_revision: Some(second.state.revision),
            material: None,
        })
        .expect("reconcile second session");
    assert!(reconciled.state.pending_deltas.is_empty());
}

#[test]
fn workspace_material_revision_advances_only_on_change_and_survives_restart() {
    let directory = tempdir().expect("temp directory");
    let path = directory.path().join("runtime.json");
    let authority = FileRuntimeAuthority::open(&path).expect("open authority");

    let first = authority
        .attach(AttachRequest {
            session_id: SessionId::from("material-session-1"),
            workspace_key: "repo:material".to_owned(),
            repository_identity: "git:material".to_owned(),
            work_key: String::new(),
            objective: "observe material".to_owned(),
            since_revision: None,
            material: Some(material_observation("head-a", "digest-a", "src/lib.rs")),
        })
        .expect("attach first material snapshot");
    let first_material = first
        .state
        .workspace_material
        .as_ref()
        .expect("material state should exist");
    assert_eq!(first_material.material_revision, 1);
    assert_eq!(first.state.revision, 3);

    let same = authority
        .attach(AttachRequest {
            session_id: SessionId::from("material-session-2"),
            workspace_key: "repo:material".to_owned(),
            repository_identity: "git:material".to_owned(),
            work_key: String::new(),
            objective: "observe material".to_owned(),
            since_revision: Some(first.state.revision),
            material: Some(material_observation("head-a", "digest-a", "src/lib.rs")),
        })
        .expect("attach identical material snapshot");
    assert_eq!(same.state.revision, first.state.revision);
    assert_eq!(
        same.state
            .workspace_material
            .as_ref()
            .expect("material state should exist")
            .material_revision,
        1
    );

    let changed = authority
        .attach(AttachRequest {
            session_id: SessionId::from("material-session-3"),
            workspace_key: "repo:material".to_owned(),
            repository_identity: "git:material".to_owned(),
            work_key: String::new(),
            objective: "observe material".to_owned(),
            since_revision: Some(same.state.revision),
            material: Some(material_observation("head-a", "digest-b", "src/lib.rs")),
        })
        .expect("attach changed material snapshot");
    assert_eq!(changed.state.revision, first.state.revision + 1);
    let changed_material = changed
        .state
        .workspace_material
        .as_ref()
        .expect("changed material state should exist");
    assert_eq!(changed_material.material_revision, 2);
    assert_eq!(changed_material.runtime_revision, changed.state.revision);

    drop(authority);

    let reopened = FileRuntimeAuthority::open(&path).expect("reopen authority");
    let stable = reopened
        .attach(AttachRequest {
            session_id: SessionId::from("material-session-4"),
            workspace_key: "repo:material".to_owned(),
            repository_identity: "git:material".to_owned(),
            work_key: String::new(),
            objective: "observe material".to_owned(),
            since_revision: Some(changed.state.revision),
            material: Some(material_observation("head-a", "digest-b", "src/lib.rs")),
        })
        .expect("reattach same material after restart");
    assert_eq!(stable.state.revision, changed.state.revision);
    assert_eq!(
        stable
            .state
            .workspace_material
            .as_ref()
            .expect("material state should survive restart")
            .material_revision,
        2
    );

    let persisted: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&path).expect("read persisted runtime"),
    )
    .expect("parse persisted runtime");
    assert_eq!(
        persisted["workspace_revisions"]
            .as_array()
            .expect("workspace revision history")
            .len(),
        2
    );
}


#[test]
fn structural_revision_advances_only_when_structure_changes() {
    let directory = tempdir().expect("temp directory");
    let path = directory.path().join("runtime.json");
    let authority = FileRuntimeAuthority::open(&path).expect("open authority");

    let first = authority
        .attach(AttachRequest {
            session_id: SessionId::from("structure-session-1"),
            workspace_key: "repo:structure".to_owned(),
            repository_identity: "git:structure".to_owned(),
            work_key: String::new(),
            objective: "observe structure".to_owned(),
            since_revision: None,
            material: Some(material_observation_with_structure(
                "head-a",
                "material-a",
                "structure-a",
                "src/lib.rs",
            )),
        })
        .expect("attach first structural snapshot");
    let first_state = first
        .state
        .workspace_material
        .as_ref()
        .expect("workspace material");
    assert_eq!(first_state.material_revision, 1);
    assert_eq!(first_state.structural_revision, 1);

    let material_only = authority
        .attach(AttachRequest {
            session_id: SessionId::from("structure-session-2"),
            workspace_key: "repo:structure".to_owned(),
            repository_identity: "git:structure".to_owned(),
            work_key: String::new(),
            objective: "observe structure".to_owned(),
            since_revision: Some(first.state.revision),
            material: Some(material_observation_with_structure(
                "head-a",
                "material-b",
                "structure-a",
                "src/lib.rs",
            )),
        })
        .expect("attach material-only change");
    let material_only_state = material_only
        .state
        .workspace_material
        .as_ref()
        .expect("workspace material");
    assert_eq!(material_only_state.material_revision, 2);
    assert_eq!(material_only_state.structural_revision, 1);

    let structural = authority
        .attach(AttachRequest {
            session_id: SessionId::from("structure-session-3"),
            workspace_key: "repo:structure".to_owned(),
            repository_identity: "git:structure".to_owned(),
            work_key: String::new(),
            objective: "observe structure".to_owned(),
            since_revision: Some(material_only.state.revision),
            material: Some(material_observation_with_structure(
                "head-a",
                "material-c",
                "structure-b",
                "src/lib.rs",
            )),
        })
        .expect("attach structural change");
    let structural_state = structural
        .state
        .workspace_material
        .as_ref()
        .expect("workspace material");
    assert_eq!(structural_state.material_revision, 3);
    assert_eq!(structural_state.structural_revision, 2);
}


fn material_observation_with_structure(
    head: &str,
    material_digest: &str,
    structure_digest: &str,
    changed_path: &str,
) -> WorkspaceMaterialObservation {
    let mut observation = material_observation(head, material_digest, changed_path);
    observation.structure.digest = structure_digest.to_owned();
    observation
}

fn material_observation(
    head: &str,
    material_digest: &str,
    changed_path: &str,
) -> WorkspaceMaterialObservation {
    let mut file_digests = BTreeMap::new();
    file_digests.insert(changed_path.to_owned(), format!("file:{material_digest}"));
    WorkspaceMaterialObservation {
        head: Some(head.to_owned()),
        index_state: format!("index:{material_digest}"),
        working_tree: format!("worktree:{material_digest}"),
        changed_paths: vec![changed_path.to_owned()],
        file_digests,
        material_digest: material_digest.to_owned(),
        structure: WorkspaceStructureObservation::default(),
    }
}
