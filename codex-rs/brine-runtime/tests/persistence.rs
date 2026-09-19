use codex_brine_runtime::AttachRequest;
use codex_brine_runtime::FileRuntimeAuthority;
use codex_brine_runtime::ReconcileRequest;
use codex_brine_runtime::RuntimeAuthority;
use codex_brine_runtime::SessionId;
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
