use std::net::TcpListener;
use std::sync::Arc;
use std::thread;

use codex_brine_runtime::AttachRequest;
use codex_brine_runtime::FileRuntimeAuthority;
use codex_brine_runtime::ReconcileRequest;
use codex_brine_runtime::RuntimeAuthority;
use codex_brine_runtime::RuntimeAuthorityServer;
use codex_brine_runtime::SessionId;
use codex_brine_runtime::TcpRuntimeAuthority;

#[test]
fn remote_authority_survives_the_codex_session_boundary() {
    let directory = tempfile::tempdir().expect("temp directory");
    let path = directory.path().join("remote-runtime.json");
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind authority");
    let address = listener.local_addr().expect("authority address");
    let authority = Arc::new(FileRuntimeAuthority::open(&path).expect("open authority"));
    let server = thread::spawn(move || {
        RuntimeAuthorityServer::serve(listener, authority, Some(5)).expect("serve authority")
    });

    let (first, first_revision, work_id) = {
        let client = TcpRuntimeAuthority::new(address);
        let first = client
            .attach(AttachRequest {
                session_id: SessionId::from("codex-session-1"),
                workspace_key: "repo:brinecdx".to_owned(),
            workspace_aliases: Vec::new(),
                repository_identity: "git:4LFR3Dv1/brinecdx".to_owned(),
                work_key: String::new(),
                objective: "implement R1".to_owned(),
                since_revision: None,
                material: None,
            })
            .expect("attach first session");
        let first_revision = first.state.revision;
        let work_id = first.attachment.work_id.clone();
        (first, first_revision, work_id)
    };

    let state_while_closed = {
        let client = TcpRuntimeAuthority::new(address);
        client
            .record_remote_delta(
                &work_id,
                "remote progress while Codex was closed".to_owned(),
            )
            .expect("record remote progress");
        client
            .state(
                &first.attachment.workspace_id,
                &work_id,
                Some(first_revision),
            )
            .expect("read remote state while closed")
    };
    assert_eq!(state_while_closed.pending_deltas.len(), 1);

    let third_client = TcpRuntimeAuthority::new(address);
    let second = third_client
        .attach(AttachRequest {
            session_id: SessionId::from("codex-session-2"),
                workspace_key: "repo:brinecdx".to_owned(),
                workspace_aliases: Vec::new(),
            repository_identity: "git:4LFR3Dv1/brinecdx".to_owned(),
            work_key: String::new(),
            objective: "implement R1".to_owned(),
            since_revision: Some(first_revision),
            material: None,
        })
        .expect("attach second session");
    assert_eq!(
        second.attachment.workspace_id,
        first.attachment.workspace_id
    );
    assert_ne!(second.attachment.session_id, first.attachment.session_id);
    assert_eq!(second.attachment.work_id, first.attachment.work_id);
    assert_eq!(second.state.work.key, "root");
    assert_eq!(second.state.pending_deltas.len(), 1);
    third_client
        .reconcile(ReconcileRequest {
            session_id: SessionId::from("codex-session-2"),
            workspace_id: second.attachment.workspace_id,
            work_id: second.attachment.work_id,
            since_revision: Some(second.state.revision),
            material: None,
        })
        .expect("reconcile second session");

    server.join().expect("authority server thread");
    let persisted = std::fs::read_to_string(path).expect("read persisted authority state");
    assert!(!persisted.contains("host-only-workspace"));
}
