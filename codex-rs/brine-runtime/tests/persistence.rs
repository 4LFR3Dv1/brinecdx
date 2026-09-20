use std::collections::BTreeMap;

use codex_brine_runtime::AttachRequest;
use codex_brine_runtime::FileRuntimeAuthority;
use codex_brine_runtime::ReconcileRequest;
use codex_brine_runtime::RuntimeAuthority;
use codex_brine_runtime::SessionId;
use codex_brine_runtime::UpdateWorkRequest;
use codex_brine_runtime::WorkStatus;
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
            workspace_aliases: Vec::new(),
            repository_identity: "git:4LFR3Dv1/brinecdx".to_owned(),
            work_key: "work:runtime-reset".to_owned(),
            objective: "implement R1".to_owned(),
            parent_session_id: None,
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
            workspace_aliases: Vec::new(),
            repository_identity: "git:4LFR3Dv1/brinecdx".to_owned(),
            work_key: "work:runtime-reset".to_owned(),
            objective: "implement R1".to_owned(),
            parent_session_id: None,
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
fn r2_runtime_state_migrates_into_r3_workgraph_defaults() {
    let directory = tempdir().expect("temp directory");
    let path = directory.path().join("runtime.json");

    let legacy_workspace_id = "workspace-r2";
    let legacy_work_id = "work-r2";
    let legacy_state = serde_json::json!({
        "revision": 6,
        "workspaces": {
            legacy_workspace_id: {
                "id": legacy_workspace_id,
                "key": "local-git:r2",
                "repository_identity": "origin-sha256:r2",
                "created_revision": 1
            }
        },
        "works": {
            legacy_work_id: {
                "id": legacy_work_id,
                "workspace_id": legacy_workspace_id,
                "key": "root",
                "objective": "R2 durable work",
                "created_revision": 2,
                "last_revision": 6
            }
        },
        "attachments": {},
        "deltas": [],
        "workspace_material": {},
        "workspace_revisions": []
    });
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&legacy_state).expect("serialize legacy R2 state"),
    )
    .expect("write legacy R2 state");

    let authority = FileRuntimeAuthority::open(&path).expect("open R2 state with R3 authority");
    let attached = authority
        .attach(AttachRequest {
            session_id: SessionId::from("r3-session"),
            workspace_key: "local-git:r2".to_owned(),
            workspace_aliases: Vec::new(),
            repository_identity: "origin-sha256:r2".to_owned(),
            work_key: String::new(),
            objective: "R2 durable work".to_owned(),
            parent_session_id: None,
            since_revision: Some(6),
            material: None,
        })
        .expect("attach R3 session to migrated R2 state");

    assert_eq!(attached.state.workspace.id.as_str(), legacy_workspace_id);
    assert_eq!(attached.state.work.id.as_str(), legacy_work_id);
    assert!(attached.state.work.parent_work.is_none());
    assert_eq!(attached.state.work.status, WorkStatus::Active);
    assert_eq!(
        attached.state.work.assigned_thread.as_ref(),
        Some(&SessionId::from("r3-session"))
    );
    assert!(attached.state.work.candidate.is_none());
    let graph = attached.state.work_graph.as_ref().expect("work graph");
    assert_eq!(graph.nodes.len(), 1);
    assert_eq!(graph.nodes[0].id.as_str(), legacy_work_id);
}

#[test]
fn work_graph_binds_child_work_and_survives_detach_and_reattach() {
    let directory = tempdir().expect("temp directory");
    let path = directory.path().join("runtime.json");
    let authority = FileRuntimeAuthority::open(&path).expect("open authority");

    let root_session = SessionId::from("root-session");
    let root = authority
        .attach(AttachRequest {
            session_id: root_session.clone(),
            workspace_key: "repo:workgraph".to_owned(),
            workspace_aliases: Vec::new(),
            repository_identity: "git:workgraph".to_owned(),
            work_key: String::new(),
            objective: "root objective".to_owned(),
            parent_session_id: None,
            since_revision: None,
            material: None,
        })
        .expect("attach root work");

    let child_session = SessionId::from("child-session");
    let child = authority
        .attach(AttachRequest {
            session_id: child_session.clone(),
            workspace_key: "repo:workgraph".to_owned(),
            workspace_aliases: Vec::new(),
            repository_identity: "git:workgraph".to_owned(),
            work_key: String::new(),
            objective: "child objective".to_owned(),
            parent_session_id: Some(root_session.clone()),
            since_revision: Some(root.state.revision),
            material: None,
        })
        .expect("attach child work");

    assert_ne!(child.attachment.work_id, root.attachment.work_id);
    assert_eq!(
        child.state.work.parent_work.as_ref(),
        Some(&root.attachment.work_id)
    );
    assert_eq!(child.state.work.status, WorkStatus::Active);
    assert_eq!(
        child.state.work.assigned_thread.as_ref(),
        Some(&child_session)
    );
    let graph = child.state.work_graph.as_ref().expect("work graph");
    assert_eq!(graph.active_work, child.attachment.work_id);
    assert_eq!(graph.nodes.len(), 2);
    assert!(
        graph
            .nodes
            .iter()
            .any(|node| node.id == root.attachment.work_id && node.parent_work.is_none())
    );

    let completed = authority
        .update_work(UpdateWorkRequest {
            session_id: child_session.clone(),
            objective: None,
            status: Some(WorkStatus::Complete),
            since_revision: Some(child.state.revision),
        })
        .expect("complete child work");
    assert_eq!(completed.state.work.status, WorkStatus::Complete);

    authority
        .detach(&child_session)
        .expect("detach child without deleting binding");
    drop(authority);

    let reopened = FileRuntimeAuthority::open(&path).expect("reopen authority");
    let rebound = reopened
        .attach(AttachRequest {
            session_id: child_session.clone(),
            workspace_key: "repo:workgraph".to_owned(),
            workspace_aliases: Vec::new(),
            repository_identity: "git:workgraph".to_owned(),
            work_key: String::new(),
            objective: "child objective".to_owned(),
            parent_session_id: None,
            since_revision: Some(completed.state.revision),
            material: None,
        })
        .expect("reattach child by durable thread binding");

    assert_eq!(rebound.attachment.work_id, child.attachment.work_id);
    assert_eq!(rebound.state.work.status, WorkStatus::Complete);
    assert_eq!(
        rebound.state.work.parent_work.as_ref(),
        Some(&root.attachment.work_id)
    );

    let resumed = reopened
        .update_work(UpdateWorkRequest {
            session_id: child_session,
            objective: None,
            status: Some(WorkStatus::Active),
            since_revision: Some(rebound.state.revision),
        })
        .expect("reactivate child on new work");
    assert_eq!(resumed.attachment.work_id, child.attachment.work_id);
    assert_eq!(resumed.state.work.status, WorkStatus::Active);
}

#[test]
fn workspace_alias_migrates_r1_identity_without_forking_work() {
    let directory = tempdir().expect("temp directory");
    let path = directory.path().join("runtime.json");
    let authority = FileRuntimeAuthority::open(&path).expect("open authority");

    let legacy_key = "local:legacy-cwd-hash";
    let first = authority
        .attach(AttachRequest {
            session_id: SessionId::from("legacy-session"),
            workspace_key: legacy_key.to_owned(),
            workspace_aliases: Vec::new(),
            repository_identity: legacy_key.to_owned(),
            work_key: String::new(),
            objective: "persistent root work".to_owned(),
            parent_session_id: None,
            since_revision: None,
            material: None,
        })
        .expect("attach legacy workspace");

    let migrated = authority
        .attach(AttachRequest {
            session_id: SessionId::from("migrated-session"),
            workspace_key: "local-git:new-common-dir-hash".to_owned(),
            workspace_aliases: vec![legacy_key.to_owned()],
            repository_identity: "origin-sha256:repo".to_owned(),
            work_key: String::new(),
            objective: "persistent root work".to_owned(),
            parent_session_id: None,
            since_revision: Some(first.state.revision),
            material: None,
        })
        .expect("attach migrated workspace");

    assert_eq!(
        migrated.attachment.workspace_id,
        first.attachment.workspace_id
    );
    assert_eq!(migrated.attachment.work_id, first.attachment.work_id);
    assert_eq!(
        migrated.state.workspace.key,
        "local-git:new-common-dir-hash"
    );
    assert_eq!(
        migrated.state.workspace.repository_identity,
        "origin-sha256:repo"
    );

    let persisted: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(path).expect("read persisted runtime"),
    )
    .expect("parse persisted runtime");
    assert_eq!(
        persisted["workspaces"]
            .as_object()
            .expect("workspace map")
            .len(),
        1
    );
    assert_eq!(
        persisted["works"].as_object().expect("work map").len(),
        1
    );
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
            workspace_aliases: Vec::new(),
            repository_identity: "git:material".to_owned(),
            work_key: String::new(),
            objective: "observe material".to_owned(),
            parent_session_id: None,
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
            workspace_aliases: Vec::new(),
            repository_identity: "git:material".to_owned(),
            work_key: String::new(),
            objective: "observe material".to_owned(),
            parent_session_id: None,
            since_revision: Some(first.state.revision),
            material: Some(material_observation("head-a", "digest-a", "src/lib.rs")),
        })
        .expect("attach identical material snapshot");
    assert!(same.state.revision > first.state.revision);
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
            workspace_aliases: Vec::new(),
            repository_identity: "git:material".to_owned(),
            work_key: String::new(),
            objective: "observe material".to_owned(),
            parent_session_id: None,
            since_revision: Some(same.state.revision),
            material: Some(material_observation("head-a", "digest-b", "src/lib.rs")),
        })
        .expect("attach changed material snapshot");
    assert!(changed.state.revision > same.state.revision);
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
            workspace_aliases: Vec::new(),
            repository_identity: "git:material".to_owned(),
            work_key: String::new(),
            objective: "observe material".to_owned(),
            parent_session_id: None,
            since_revision: Some(changed.state.revision),
            material: Some(material_observation("head-a", "digest-b", "src/lib.rs")),
        })
        .expect("reattach same material after restart");
    assert!(stable.state.revision >= changed.state.revision);
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
            workspace_aliases: Vec::new(),
            repository_identity: "git:structure".to_owned(),
            work_key: String::new(),
            objective: "observe structure".to_owned(),
            parent_session_id: None,
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
            workspace_aliases: Vec::new(),
            repository_identity: "git:structure".to_owned(),
            work_key: String::new(),
            objective: "observe structure".to_owned(),
            parent_session_id: None,
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
            workspace_aliases: Vec::new(),
            repository_identity: "git:structure".to_owned(),
            work_key: String::new(),
            objective: "observe structure".to_owned(),
            parent_session_id: None,
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
