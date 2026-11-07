#[path = "../src/cloud/mod.rs"]
mod cloud;

use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use noesora_engine::{
    ContentScope, RecordInput, ReplicaEvent, ReplicaObjectManifestV1, ReplicaOperation,
    ReplicaSnapshotHeadV1, ReplicaSnapshotV1, SearchFilters, SearchMode, Store,
};
use serde_json::{json, Value};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

struct LocalStore {
    home: PathBuf,
    store: Store,
}

impl LocalStore {
    fn new() -> Self {
        let sequence = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let home = std::env::temp_dir().join(format!(
            "noesora-cloud-test-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&home).expect("create unique store home");
        let store = Store::open(&home).expect("open isolated Engine store");
        Self { home, store }
    }

    fn note(&self, title: &str, body: &str, project: &str) -> noesora_engine::WrittenRecord {
        self.store
            .write_record(&RecordInput {
                title: title.to_owned(),
                body: body.to_owned(),
                kind: "note".to_owned(),
                status: "open".to_owned(),
                project: project.to_owned(),
                files: Vec::new(),
            })
            .expect("write fixture note")
    }
    fn reopen(&mut self) {
        let reopened = Store::open(&self.home).expect("reopen durable Engine store");
        let previous = std::mem::replace(&mut self.store, reopened);
        drop(previous);
    }
}

impl Drop for LocalStore {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.home);
    }
}

#[derive(Debug)]
struct Request {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

#[derive(Clone)]
struct Reply {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
    disconnect: bool,
    echo_proposal: bool,
}

impl Reply {
    fn json(value: Value) -> Self {
        Self {
            status: 200,
            content_type: "application/json",
            body: serde_json::to_vec(&value).expect("encode fixture response"),
            disconnect: false,
            echo_proposal: false,
        }
    }

    fn status_json(status: u16, value: Value) -> Self {
        Self {
            status,
            ..Self::json(value)
        }
    }

    fn disconnect() -> Self {
        Self {
            status: 200,
            content_type: "application/json",
            body: Vec::new(),
            disconnect: true,
            echo_proposal: false,
        }
    }

    fn pending_proposal() -> Self {
        Self {
            status: 200,
            content_type: "application/json",
            body: Vec::new(),
            disconnect: false,
            echo_proposal: true,
        }
    }

    fn octets(body: Vec<u8>) -> Self {
        Self {
            status: 200,
            content_type: "application/octet-stream",
            body,
            disconnect: false,
            echo_proposal: false,
        }
    }
}

fn fixture(replies: Vec<Reply>) -> (String, JoinHandle<Vec<Request>>) {
    fixture_with(move |_| replies)
}

fn fixture_with(build: impl FnOnce(&str) -> Vec<Reply>) -> (String, JoinHandle<Vec<Request>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local HTTP fixture");
    let address = listener.local_addr().expect("fixture address");
    let origin = format!("http://{address}");
    let replies = build(&origin);
    let worker = thread::spawn(move || {
        let mut requests = Vec::with_capacity(replies.len());
        for reply in replies {
            let (mut stream, _) = listener.accept().expect("accept fixture request");
            let request = read_request(&stream);
            let body = if reply.echo_proposal {
                proposal_view(&request.body)
            } else {
                reply.body
            };
            requests.push(request);
            if reply.disconnect {
                let _ = stream.shutdown(Shutdown::Both);
                continue;
            }
            let reason = match reply.status {
                200 => "OK",
                400 => "Bad Request",
                401 => "Unauthorized",
                403 => "Forbidden",
                404 => "Not Found",
                409 => "Conflict",
                410 => "Gone",
                _ => "Fixture Response",
            };
            write!(
                stream,
                "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                reply.status,
                reason,
                reply.content_type,
                body.len(),
            ).expect("write fixture response header");
            stream
                .write_all(&body)
                .expect("write fixture response body");
            let _ = stream.shutdown(Shutdown::Both);
        }
        requests
    });
    (origin, worker)
}

fn read_request(stream: &TcpStream) -> Request {
    let mut reader = BufReader::new(stream.try_clone().expect("clone fixture stream"));
    let mut first = String::new();
    reader.read_line(&mut first).expect("read request line");
    let mut first_parts = first.split_whitespace();
    let method = first_parts.next().expect("request method").to_owned();
    let path = first_parts.next().expect("request path").to_owned();
    let mut headers = BTreeMap::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).expect("read request header");
        if line == "\r\n" || line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    let length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0; length];
    reader.read_exact(&mut body).expect("read request body");
    Request {
        method,
        path,
        headers,
        body,
    }
}

fn identity(user_id: &str) -> Value {
    json!({"schema":"noesora.identity/v1","user_id":user_id,"org_id":null})
}

fn organization_list(org_id: &str) -> Value {
    json!({"organizations":[{"id":org_id,"name":"Fixture organization","status":"ready","region":"local","steward":true,"can_manage":true}],"next_offset":null})
}

fn org_status(org_id: &str) -> Value {
    json!({"org_id":org_id,"phase":"ready","customer_ready":true,"region":"local","restore_verified_at":"2026-11-01T00:00:00Z"})
}

fn snapshot(org_id: &str, epoch: &str, seq: u64, last_event_hash: &str) -> ReplicaSnapshotV1 {
    let mut snapshot = ReplicaSnapshotV1 {
        schema: "noesora.sync-snapshot/v1".to_owned(),
        org_id: org_id.to_owned(),
        restore_epoch: epoch.to_owned(),
        seq,
        last_event_hash: last_event_hash.to_owned(),
        objects: Vec::new(),
        active_objects: Vec::new(),
        tombstones: Vec::new(),
        sha256: String::new(),
    };
    snapshot.sha256 = snapshot
        .calculated_sha256()
        .expect("calculate fixture snapshot hash");
    snapshot
}

fn page(
    org_id: &str,
    epoch: &str,
    high_water: u64,
    events: Vec<Value>,
    after: Option<&str>,
    next: &str,
    has_more: bool,
) -> Value {
    json!({
        "schema":"noesora.sync-page/v1",
        "org_id":org_id,
        "restore_epoch":epoch,
        "after":after,
        "high_water_seq":high_water,
        "events":events,
        "next_cursor":next,
        "has_more":has_more,
    })
}

fn context_replies(snapshot: &ReplicaSnapshotV1, actor: &str) -> Vec<Reply> {
    vec![
        Reply::json(identity(actor)),
        Reply::json(organization_list(&snapshot.org_id)),
        Reply::json(org_status(&snapshot.org_id)),
        Reply::json(serde_json::to_value(snapshot).expect("encode fixture snapshot")),
        Reply::json(page(
            &snapshot.org_id,
            &snapshot.restore_epoch,
            snapshot.seq,
            Vec::new(),
            None,
            "noesora.sync-cursor/v1.fixture",
            false,
        )),
    ]
}

fn note_manifest(store: &Store, id: &str) -> (ReplicaObjectManifestV1, Vec<u8>) {
    let object = store.export_record(id).expect("export fixture record");
    let manifest = store
        .export_record_manifest(id)
        .expect("export known structured record references");
    (manifest, object.bytes)
}

fn upload_replies(manifest: &ReplicaObjectManifestV1, proposal_reply: Reply) -> Vec<Reply> {
    vec![
        Reply::json(
            json!({"upload_id":"fixture-upload","status":"awaiting_bytes","sha256":manifest.sha256,"expires_at":"2026-11-07T00:00:00Z"}),
        ),
        Reply::json(
            json!({"object":{"object_id":manifest.object_id,"sha256":manifest.sha256},"manifest":manifest}),
        ),
        proposal_reply,
    ]
}

fn proposal_view(proposal_bytes: &[u8]) -> Vec<u8> {
    let proposal: Value = serde_json::from_slice(proposal_bytes).expect("parse proposal request");
    serde_json::to_vec(&json!({
        "id":"proposal-1","revision":1,"state":"pending_review",
        "proposal":proposal,"submitted_by":"member-1","prepared_content":null,"base_content":null,
        "content_sha256":null,"approval":null,"decisions":[],"created_at":"2026-11-07T00:00:00Z",
        "updated_at":"2026-11-07T00:00:00Z","commit_id":null,"error_code":null,
        "can_approve":false,"can_reject":false,"can_withdraw":true
    }))
    .expect("encode proposal view")
}

fn proposal_pending() -> Reply {
    Reply::pending_proposal()
}

fn error_reply(status: u16, code: &str) -> Reply {
    Reply::status_json(
        status,
        json!({
            "schema":"noesora.error/v1",
            "code":code,
            "message":"The base cursor is stale.",
            "request_id":"fixture-request",
            "retryable":false,
            "details":{"current_cursor":null,"current_hash":null,"proposal_revision":null,"retry_after":null}
        }),
    )
}

fn cloud_account_metadata(origin: &str) -> Value {
    json!({
        "schema":"noesora.auth-metadata/v1",
        "issuer":origin,
        "client_id":"fixture-public-client",
        "api_origin":origin,
        "required_scopes":["user:org:read"]
    })
}

fn oauth_metadata(origin: &str) -> Value {
    json!({
        "issuer":origin,
        "device_authorization_endpoint":format!("{origin}/oauth/device"),
        "token_endpoint":format!("{origin}/oauth/token"),
        "revocation_endpoint":format!("{origin}/oauth/revoke"),
        "grant_types_supported":["authorization_code","urn:ietf:params:oauth:grant-type:device_code","refresh_token"]
    })
}

#[test]
fn device_grant_honors_pending_slow_down_and_keeps_tokens_out_of_sidecars() {
    let (origin, server) = fixture_with(|origin| {
        vec![
            Reply::json(cloud_account_metadata(origin)),
            Reply::json(oauth_metadata(origin)),
            Reply::json(
                json!({"device_code":"private-device-code","user_code":"ABCD-EFGH","verification_uri":format!("{origin}/device"),"expires_in":60,"interval":0}),
            ),
            Reply::status_json(400, json!({"error":"authorization_pending"})),
            Reply::status_json(400, json!({"error":"slow_down"})),
            Reply::json(
                json!({"access_token":"fixture-access-token","refresh_token":"fixture-refresh-token","token_type":"Bearer","expires_in":3600,"refresh_expires_in":7200}),
            ),
            Reply::json(identity("fixture-user")),
        ]
    });
    let local = LocalStore::new();
    let credentials = cloud::test_support::MemoryCredentialStore::default();
    let elapsed = Cell::new(Duration::ZERO);
    let started = Instant::now();
    let output = cloud::test_support::login_with_fake_clock(
        &local.store,
        &origin,
        &credentials,
        |duration| elapsed.set(elapsed.get().saturating_add(duration)),
        || started + elapsed.get(),
    )
    .expect("complete device flow against fixture");
    assert_eq!(output.payload["user_id"], "fixture-user");
    assert_eq!(elapsed.get(), Duration::from_secs(5));
    assert!(cloud::test_support::memory_token_present(
        &credentials,
        &origin
    ));
    let account = fs::read_to_string(local.store.root().join("cloud/account.json"))
        .expect("read non-secret Cloud account sidecar");
    assert!(!account.contains("fixture-access-token"));
    assert!(!account.contains("fixture-refresh-token"));
    assert!(!account.contains("private-device-code"));
    let requests = server.join().expect("fixture worker");
    assert_eq!(requests.len(), 7);
    assert!(requests[2]
        .body
        .windows(b"client_id=fixture-public-client".len())
        .any(|window| window == b"client_id=fixture-public-client"));
    assert!(!String::from_utf8_lossy(&requests[2].body).contains("client_secret"));
    assert!(String::from_utf8_lossy(&requests[3].body).contains("private-device-code"));
}

#[test]
fn device_grant_expires_without_polling_after_the_deadline() {
    let (origin, server) = fixture_with(|origin| {
        vec![
            Reply::json(cloud_account_metadata(origin)),
            Reply::json(oauth_metadata(origin)),
            Reply::json(
                json!({"device_code":"short-lived-device-code","user_code":"WXYZ-1234","verification_uri":format!("{origin}/device"),"expires_in":1,"interval":5}),
            ),
        ]
    });
    let local = LocalStore::new();
    let credentials = cloud::test_support::MemoryCredentialStore::default();
    let elapsed = Cell::new(Duration::ZERO);
    let started = Instant::now();
    let error = cloud::test_support::login_with_fake_clock(
        &local.store,
        &origin,
        &credentials,
        |duration| elapsed.set(elapsed.get().saturating_add(duration)),
        || started + elapsed.get(),
    )
    .expect_err("device authorization expires");
    assert_eq!(error.code, "OAUTH_DEVICE_EXPIRED");
    assert!(!cloud::test_support::memory_token_present(
        &credentials,
        &origin
    ));
    let requests = server.join().expect("fixture worker");
    assert_eq!(
        requests.len(),
        3,
        "an expired grant must not poll token endpoint"
    );
}

#[test]
fn stored_credential_must_match_account_issuer_and_client_before_refresh() {
    for (issuer, client_id) in [
        ("http://invalid-issuer.example", "fixture-public-client"),
        ("", "different-client"),
    ] {
        let local = LocalStore::new();
        let (origin, server) = fixture(Vec::new());
        cloud::test_support::write_account_for_test(
            &local.store,
            &origin,
            &origin,
            "fixture-public-client",
        )
        .expect("write account metadata");
        let credentials = cloud::test_support::MemoryCredentialStore::default();
        cloud::test_support::seed_memory_credential(&credentials, &origin, issuer, client_id, 0)
            .expect("seed mismatched test credential");
        let error = cloud::test_support::status_with_credentials(&local.store, &credentials)
            .expect_err("mismatched tuple must be rejected before OAuth refresh");
        assert_eq!(error.code, "CLOUD_CREDENTIAL_MISMATCH");
        assert!(server.join().expect("no OAuth requests").is_empty());
    }
}
#[test]
fn matching_stored_tuple_refreshes_at_its_discovered_issuer() {
    let (origin, server) = fixture_with(|origin| {
        vec![
            Reply::json(oauth_metadata(origin)),
            Reply::json(
                json!({"access_token":"refreshed-access","refresh_token":"replacement-refresh","token_type":"Bearer","expires_in":3600,"refresh_expires_in":7200}),
            ),
            Reply::json(identity("fixture-user")),
            Reply::json(organization_list("org-a")),
        ]
    });
    let local = LocalStore::new();
    let credentials = cloud::test_support::MemoryCredentialStore::default();
    cloud::test_support::write_account_for_test(
        &local.store,
        &origin,
        &origin,
        "fixture-public-client",
    )
    .expect("write account metadata");
    cloud::test_support::seed_memory_credential(
        &credentials,
        &origin,
        &origin,
        "fixture-public-client",
        0,
    )
    .expect("seed matching test credential");
    let output = cloud::test_support::status_with_credentials(&local.store, &credentials)
        .expect("refresh matching credential and load live identity");
    assert_eq!(output.payload["authenticated"], true);
    let requests = server.join().expect("fixture worker");
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[0].path, "/.well-known/oauth-authorization-server");
    assert!(String::from_utf8_lossy(&requests[1].body).contains("grant_type=refresh_token"));
    assert!(String::from_utf8_lossy(&requests[1].body).contains("client_id=fixture-public-client"));
    assert_eq!(
        requests[2].headers.get("authorization").map(String::as_str),
        Some("Bearer refreshed-access")
    );
}
#[test]
fn preview_binds_exact_selection_actor_destination_and_manifest() {
    let local = LocalStore::new();
    let selected = local.note("Selected", "selected full body", "shared-project");
    let canary = local.note(
        "Canary",
        "unselected same-project content",
        "shared-project",
    );
    let snapshot = snapshot(
        "org-a",
        "epoch-a",
        0,
        "sha256:0000000000000000000000000000000000000000000000000000000000000000",
    );
    let mut replies = context_replies(&snapshot, "member-a");
    replies.extend(context_replies(&snapshot, "member-b"));
    replies.extend(context_replies(&snapshot, "member-a"));
    let (origin, server) = fixture(replies);
    let preview = cloud::test_support::push_with_token(
        &local.store,
        &origin,
        "fixture-token",
        "org-a",
        &[selected.id.clone()],
        &[],
        None,
    )
    .expect("create selected preview");
    let preview_sha = preview.payload["preview_sha256"]
        .as_str()
        .expect("preview hash")
        .to_owned();
    let preview_path = PathBuf::from(
        preview.payload["preview_path"]
            .as_str()
            .expect("preview path"),
    );
    let disclosure: Value =
        serde_json::from_slice(&fs::read(&preview_path).expect("read full disclosure preview"))
            .expect("parse disclosure preview");
    assert_eq!(
        disclosure["items"]
            .as_array()
            .expect("disclosure items")
            .len(),
        1
    );
    assert!(disclosure.to_string().contains("selected full body"));
    assert!(!disclosure.to_string().contains(&canary.id));
    let actor_error = cloud::test_support::push_with_token(
        &local.store,
        &origin,
        "fixture-token",
        "org-a",
        &[selected.id.clone()],
        &[],
        Some(&preview_sha),
    )
    .expect_err("actor change invalidates consent");
    assert_eq!(actor_error.code, "CLOUD_CONFIRMATION_REQUIRED");
    let destination_error = cloud::test_support::push_with_token(
        &local.store,
        &origin,
        "fixture-token",
        "org-other",
        &[selected.id.clone()],
        &[],
        Some(&preview_sha),
    )
    .expect_err("destination change invalidates consent");
    assert_eq!(destination_error.code, "CLOUD_CONFIRMATION_REQUIRED");
    let changed = fs::read(&selected.path).expect("read selected record");
    let changed = String::from_utf8(changed)
        .expect("record UTF-8")
        .replace("selected full body", "changed after preview");
    fs::write(&selected.path, changed).expect("mutate local fixture record");
    let manifest_error = cloud::test_support::push_with_token(
        &local.store,
        &origin,
        "fixture-token",
        "org-a",
        &[selected.id.clone()],
        &[],
        Some(&preview_sha),
    )
    .expect_err("manifest change invalidates consent");
    assert_eq!(manifest_error.code, "CLOUD_CONFIRMATION_REQUIRED");
    let requests = server.join().expect("fixture worker");
    assert_eq!(
        requests.len(),
        15,
        "consent refusal must occur before upload or proposal calls"
    );
    assert!(requests.iter().all(|request| request.method != "POST"));
}

#[test]
fn unselected_session_evidence_fails_reference_closure_before_upload() {
    let local = LocalStore::new();
    let record = local.note("Citation", "citation evidence", "project");
    let hash = local
        .store
        .export_record(&record.id)
        .expect("export record")
        .sha256;
    let error = cloud::test_support::reference_closure_refusal(
        &local.store,
        &record.id,
        &hash,
        noesora_engine::ReplicaEvidenceKind::SessionEvent,
    )
    .expect_err("unselected raw session evidence is refused");
    assert_eq!(error.code, "REFERENCE_CLOSURE_REQUIRED");
}

#[test]
fn pull_integrity_refusals_do_not_initialize_or_advance_engine_checkpoint() {
    let cases = [
        "wrong_org",
        "wrong_snapshot_hash",
        "wrong_epoch",
        "sequence_gap",
        "wrong_event_hash",
    ];
    for case in cases {
        let local = LocalStore::new();
        let org_id = "org-a";
        let (snapshot, event_page) = match case {
            "wrong_org" => (snapshot("org-b", "epoch-a", 0, zero_hash()), None),
            "wrong_snapshot_hash" => {
                let mut value = snapshot(org_id, "epoch-a", 0, zero_hash());
                value.sha256 =
                    "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
                        .to_owned();
                (value, None)
            }
            "wrong_epoch" => (
                snapshot(org_id, "epoch-a", 0, zero_hash()),
                Some(page(
                    org_id,
                    "epoch-b",
                    0,
                    Vec::new(),
                    None,
                    "noesora.sync-cursor/v1.fixture",
                    false,
                )),
            ),
            "sequence_gap" => {
                let event = sample_event(org_id, "epoch-a", 2);
                let mut value = snapshot(
                    org_id,
                    "epoch-a",
                    2,
                    &event.event_hash().expect("event hash"),
                );
                value.sha256 = value.calculated_sha256().expect("re-hash fixture snapshot");
                (
                    value,
                    Some(page(
                        org_id,
                        "epoch-a",
                        2,
                        vec![serde_json::to_value(event).expect("encode event")],
                        None,
                        "noesora.sync-cursor/v1.fixture",
                        false,
                    )),
                )
            }
            _ => {
                let event = sample_event(org_id, "epoch-a", 1);
                let mut value = snapshot(
                    org_id,
                    "epoch-a",
                    1,
                    "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
                );
                value.sha256 = value
                    .calculated_sha256()
                    .expect("re-hash wrong-chain snapshot");
                (
                    value,
                    Some(page(
                        org_id,
                        "epoch-a",
                        1,
                        vec![serde_json::to_value(event).expect("encode event")],
                        None,
                        "noesora.sync-cursor/v1.fixture",
                        false,
                    )),
                )
            }
        };
        let mut replies = vec![
            Reply::json(identity("member-a")),
            Reply::json(organization_list(org_id)),
            Reply::json(org_status(org_id)),
            Reply::json(serde_json::to_value(&snapshot).expect("encode snapshot")),
        ];
        if let Some(page) = event_page {
            replies.push(Reply::json(page));
        }
        let (origin, server) = fixture(replies);
        let error =
            cloud::test_support::pull_with_token(&local.store, &origin, "fixture-token", org_id)
                .expect_err("corrupt remote cut must be rejected");
        assert!(
            matches!(error.category.as_str(), "integrity" | "conflict"),
            "{case}: {error:?}"
        );
        assert!(
            local
                .store
                .replica_checkpoint(org_id)
                .expect("read Engine checkpoint")
                .is_none(),
            "{case}: invalid cut must not initialize a checkpoint"
        );
        let requests = server.join().expect("fixture worker");
        assert_eq!(
            requests.len(),
            if matches!(case, "wrong_org" | "wrong_snapshot_hash") {
                4
            } else {
                5
            },
            "{case}"
        );
    }
}

#[test]
fn lost_proposal_reply_replays_the_same_idempotency_keys() {
    let local = LocalStore::new();
    let selected = local.note("Lost receipt", "stable proposal bytes", "project");
    let (manifest, selected_bytes) = note_manifest(&local.store, &selected.id);
    let snapshot = snapshot("org-a", "epoch-a", 0, zero_hash());
    let mut replies = context_replies(&snapshot, "member-a");
    replies.extend(context_replies(&snapshot, "member-a"));
    replies.extend(upload_replies(&manifest, Reply::disconnect()));
    replies.extend(context_replies(&snapshot, "member-a"));
    replies.extend(upload_replies(&manifest, proposal_pending()));
    let (origin, server) = fixture(replies);
    let preview = cloud::test_support::push_with_token(
        &local.store,
        &origin,
        "fixture-token",
        "org-a",
        &[selected.id.clone()],
        &[],
        None,
    )
    .expect("create durable preview");
    let digest = preview.payload["preview_sha256"]
        .as_str()
        .expect("preview digest")
        .to_owned();
    let unknown = cloud::test_support::push_with_token(
        &local.store,
        &origin,
        "fixture-token",
        "org-a",
        &[selected.id.clone()],
        &[],
        Some(&digest),
    )
    .expect_err("lost proposal response is unknown");
    assert_eq!(unknown.code, "CLOUD_RECEIPT_UNKNOWN");
    let replay = cloud::test_support::push_with_token(
        &local.store,
        &origin,
        "fixture-token",
        "org-a",
        &[selected.id.clone()],
        &[],
        Some(&digest),
    )
    .expect("replay same proposal key");
    assert_eq!(replay.payload["items"][0]["state"], "review_pending");
    let requests = server.join().expect("fixture worker");
    assert_eq!(requests.len(), 21);
    assert_eq!(requests[10].path, "/v1/orgs/org-a/sync/objects");
    assert_eq!(
        requests[11].path,
        "/v1/orgs/org-a/sync/objects/fixture-upload/bytes"
    );
    assert_eq!(requests[12].path, "/v1/orgs/org-a/proposals");
    assert_eq!(requests[11].body, selected_bytes);
    assert_eq!(requests[19].body, selected_bytes);
    assert_eq!(
        requests[10].headers.get("idempotency-key"),
        requests[18].headers.get("idempotency-key")
    );
    let first_proposal: Value =
        serde_json::from_slice(&requests[12].body).expect("first proposal request");
    let replay_proposal: Value =
        serde_json::from_slice(&requests[20].body).expect("replayed proposal request");
    assert_eq!(
        first_proposal["idempotency_key"],
        replay_proposal["idempotency_key"]
    );
    assert_eq!(first_proposal, replay_proposal);
}

#[test]
fn stale_base_is_preserved_as_typed_conflict() {
    let local = LocalStore::new();
    let selected = local.note("Stale base", "causal bytes", "project");
    let (manifest, _) = note_manifest(&local.store, &selected.id);
    let snapshot = snapshot("org-a", "epoch-a", 0, zero_hash());
    let mut replies = context_replies(&snapshot, "member-a");
    replies.extend(context_replies(&snapshot, "member-a"));
    replies.extend(upload_replies(&manifest, error_reply(409, "STALE_BASE")));
    let (origin, server) = fixture(replies);
    let preview = cloud::test_support::push_with_token(
        &local.store,
        &origin,
        "fixture-token",
        "org-a",
        &[selected.id.clone()],
        &[],
        None,
    )
    .expect("create preview");
    let digest = preview.payload["preview_sha256"]
        .as_str()
        .expect("preview digest");
    let error = cloud::test_support::push_with_token(
        &local.store,
        &origin,
        "fixture-token",
        "org-a",
        &[selected.id.clone()],
        &[],
        Some(digest),
    )
    .expect_err("stale base is a conflict");
    assert_eq!(error.code, "STALE_BASE");
    assert_eq!(error.category, "conflict");
    let outbox_path = PathBuf::from(
        preview.payload["preview_path"]
            .as_str()
            .expect("preview path"),
    )
    .parent()
    .expect("outbox directory")
    .join("outbox.json");
    let outbox: Value =
        serde_json::from_slice(&fs::read(outbox_path).expect("read durable outbox"))
            .expect("parse outbox");
    assert_eq!(outbox["items"][0]["state"], "conflict");
    assert!(
        local.store.export_record(&selected.id).is_ok(),
        "local canonical bytes remain available"
    );
    assert_eq!(server.join().expect("fixture worker").len(), 13);
}

#[test]
fn local_status_remains_offline_without_cloud_configuration() {
    let local = LocalStore::new();
    let mut command = Command::new(env!("CARGO_BIN_EXE_noesora"));
    let output = command
        .env("NOESORA_HOME", &local.home)
        .env_remove("HOME")
        .env_remove("USERPROFILE")
        .args(["--json", "status"])
        .output()
        .expect("run local status");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("JSON local status");
    assert_eq!(value["ok"], true);
    assert!(
        !local.store.root().join("cloud/account.json").exists(),
        "local status must not create Cloud state"
    );
}

#[test]
fn verified_snapshot_is_applied_then_global_search_works_without_cloud() {
    let remote = LocalStore::new();
    let source = remote.note(
        "Offline recall",
        "verified phrase after cloud pull",
        "sync-project",
    );
    let (manifest, bytes) = note_manifest(&remote.store, &source.id);
    let mut event = sample_event("org-a", "epoch-a", 1);
    event.event_id = "remote-event-1".to_owned();
    event.object_id = manifest.object_id.clone();
    event.object_schema = manifest.object_schema.clone();
    event.sha256 = manifest.sha256.clone();
    let event_hash = event.event_hash().expect("derive event hash");
    let mut cut = snapshot("org-a", "epoch-a", 1, &event_hash);
    cut.objects.push(manifest.clone());
    cut.active_objects.push(ReplicaSnapshotHeadV1 {
        object_id: manifest.object_id.clone(),
        sha256: manifest.sha256.clone(),
        latest_event_id: event.event_id.clone(),
    });
    cut.sha256 = cut
        .calculated_sha256()
        .expect("calculate full snapshot hash");

    let replies = vec![
        Reply::json(identity("member-a")),
        Reply::json(organization_list("org-a")),
        Reply::json(org_status("org-a")),
        Reply::json(serde_json::to_value(&cut).expect("encode full snapshot")),
        Reply::json(page(
            "org-a",
            "epoch-a",
            1,
            vec![serde_json::to_value(&event).expect("encode event")],
            None,
            "noesora.sync-cursor/v1.tip",
            false,
        )),
        Reply::octets(bytes.clone()),
    ];
    let (origin, server) = fixture(replies);
    let local = LocalStore::new();
    let pulled =
        cloud::test_support::pull_with_token(&local.store, &origin, "fixture-token", "org-a")
            .expect("apply verified snapshot");
    assert_eq!(pulled.payload["seq"], 1);
    let checkpoint = local
        .store
        .replica_checkpoint("org-a")
        .expect("read Engine checkpoint")
        .expect("snapshot receipt");
    assert_eq!(checkpoint.last_event_hash, event_hash);
    let copied = local
        .store
        .read_replica_object("org-a", &manifest.object_id)
        .expect("read linked record");
    assert_eq!(copied.sha256, manifest.sha256);
    assert_eq!(copied.bytes, bytes);
    server.join().expect("fixture worker");

    let response = local
        .store
        .search(
            "verified phrase",
            SearchMode::Text,
            &SearchFilters {
                project: None,
                client: None,
                source_namespace: None,
                scope: ContentScope::All,
                limit: 10,
            },
        )
        .expect("search ordinary local Store offline");
    assert!(response
        .hits
        .iter()
        .any(|hit| hit.id.as_str() == manifest.object_id.as_str()));
}

#[test]
fn replay_catches_engine_receipt_ahead_of_wire_cursor_without_duplicate_or_skipped_records() {
    let source = LocalStore::new();
    let first = source.note("First remote", "first replay phrase", "sync-project");
    let second = source.note("Second remote", "second replay phrase", "sync-project");
    let (first_manifest, first_bytes) = note_manifest(&source.store, &first.id);
    let (mut second_manifest, second_bytes) = note_manifest(&source.store, &second.id);
    let first_object = source
        .store
        .export_record(&first.id)
        .expect("export first event bytes");
    let mut first_event = sample_event("org-a", "epoch-a", 1);
    first_event.event_id = "event-1".to_owned();
    first_event.object_id = first_manifest.object_id.clone();
    first_event.object_schema = first_manifest.object_schema.clone();
    first_event.sha256 = first_manifest.sha256.clone();
    first_event.evidence_refs = first_manifest.evidence_refs.clone();
    first_event.object = Some(first_object);
    let first_hash = first_event.event_hash().expect("first event hash");
    let reference = noesora_engine::ReplicaEvidenceRef {
        schema: "noesora.evidence-ref/v1".to_owned(),
        object_id: first_manifest.object_id.clone(),
        sha256: first_manifest.sha256.clone(),
        locator_kind: noesora_engine::ReplicaEvidenceKind::MarkdownBytes,
        byte_range: Some([0, 1]),
        json_pointer: None,
        text_range: None,
    };
    second_manifest.evidence_refs = vec![reference.clone()];
    let mut second_event = sample_event("org-a", "epoch-a", 2);
    second_event.event_id = "event-2".to_owned();
    second_event.object_id = second_manifest.object_id.clone();
    second_event.object_schema = second_manifest.object_schema.clone();
    second_event.sha256 = second_manifest.sha256.clone();
    second_event.evidence_refs = second_manifest.evidence_refs.clone();
    second_event.previous_event_hash = first_hash.clone();
    let second_hash = second_event.event_hash().expect("second event hash");

    let initial_snapshot = snapshot("org-a", "epoch-a", 0, zero_hash());
    let (initial_origin, initial_server) = fixture(context_replies(&initial_snapshot, "member-a"));
    let mut local = LocalStore::new();
    cloud::test_support::pull_with_token(&local.store, &initial_origin, "fixture-token", "org-a")
        .expect("apply initial empty snapshot");
    assert_eq!(
        cloud::test_support::wire_checkpoint_cut(&local.store, "org-a")
            .expect("wire checkpoint")
            .expect("wire cut")
            .0,
        0
    );
    let first_report = local
        .store
        .apply_replica_event(&first_event)
        .expect("durably apply event before wire cursor update");
    assert_eq!(first_report.checkpoint.seq, 1);
    assert_eq!(
        cloud::test_support::wire_checkpoint_cut(&local.store, "org-a")
            .expect("wire checkpoint")
            .expect("wire cut")
            .0,
        0
    );
    local.reopen();
    let durable = local
        .store
        .replica_checkpoint("org-a")
        .expect("read reopened Engine checkpoint")
        .expect("durable event receipt");
    assert_eq!(durable.seq, 1);
    assert_eq!(durable.last_event_hash, first_hash);
    assert_eq!(
        cloud::test_support::wire_checkpoint_cut(&local.store, "org-a")
            .expect("wire checkpoint after reopen")
            .expect("stale wire cursor")
            .0,
        0
    );
    initial_server.join().expect("initial fixture worker");

    let mut cut = snapshot("org-a", "epoch-a", 2, &second_hash);
    cut.objects = vec![first_manifest.clone(), second_manifest.clone()];
    cut.objects.sort_by(|left, right| {
        (&left.object_id, &left.sha256).cmp(&(&right.object_id, &right.sha256))
    });
    cut.active_objects = vec![
        ReplicaSnapshotHeadV1 {
            object_id: first_manifest.object_id.clone(),
            sha256: first_manifest.sha256.clone(),
            latest_event_id: first_event.event_id.clone(),
        },
        ReplicaSnapshotHeadV1 {
            object_id: second_manifest.object_id.clone(),
            sha256: second_manifest.sha256.clone(),
            latest_event_id: second_event.event_id.clone(),
        },
    ];
    cut.active_objects
        .sort_by(|left, right| left.object_id.cmp(&right.object_id));
    cut.sha256 = cut.calculated_sha256().expect("full snapshot digest");

    let cursor_zero = "noesora.sync-cursor/v1.fixture";
    let cursor_one = "noesora.sync-cursor/v1.one";
    let cursor_two = "noesora.sync-cursor/v1.two";
    let event_one = serde_json::to_value(&first_event).expect("encode first event");
    let event_two = serde_json::to_value(&second_event).expect("encode second event");
    let replies = vec![
        Reply::json(identity("member-a")),
        Reply::json(organization_list("org-a")),
        Reply::json(org_status("org-a")),
        Reply::json(serde_json::to_value(&cut).expect("encode forward snapshot")),
        Reply::json(page(
            "org-a",
            "epoch-a",
            2,
            vec![event_one.clone()],
            None,
            cursor_one,
            true,
        )),
        Reply::json(page(
            "org-a",
            "epoch-a",
            2,
            vec![event_two.clone()],
            Some(cursor_one),
            cursor_two,
            false,
        )),
        Reply::json(page(
            "org-a",
            "epoch-a",
            2,
            vec![event_one],
            Some(cursor_zero),
            cursor_one,
            true,
        )),
        Reply::json(serde_json::to_value(&first_manifest).expect("encode first manifest")),
        Reply::octets(first_bytes.clone()),
        Reply::json(page(
            "org-a",
            "epoch-a",
            2,
            vec![event_two],
            Some(cursor_one),
            cursor_two,
            false,
        )),
        Reply::json(serde_json::to_value(&second_manifest).expect("encode second manifest")),
        Reply::octets(second_bytes.clone()),
    ];
    let (origin, server) = fixture(replies);
    let pulled =
        cloud::test_support::pull_with_token(&local.store, &origin, "fixture-token", "org-a")
            .expect("replay exact events after Engine commit before wire cursor update");
    assert_eq!(pulled.payload["seq"], 2);
    assert_eq!(pulled.payload["applied_events"], 1);
    let checkpoint = local
        .store
        .replica_checkpoint("org-a")
        .expect("Engine checkpoint")
        .expect("Engine receipt");
    assert_eq!(checkpoint.seq, 2);
    assert_eq!(checkpoint.last_event_hash, second_hash);
    let wire = cloud::test_support::wire_checkpoint_cut(&local.store, "org-a")
        .expect("wire checkpoint")
        .expect("wire cursor");
    assert_eq!(wire.0, 2);
    assert_eq!(wire.1, second_hash);
    assert_eq!(wire.2, cursor_two);
    assert_eq!(
        local
            .store
            .read_replica_object("org-a", &first.id)
            .expect("first active copy")
            .bytes,
        first_bytes
    );
    assert_eq!(
        local
            .store
            .read_replica_object("org-a", &second.id)
            .expect("second active copy")
            .bytes,
        second_bytes
    );
    assert_eq!(
        local
            .store
            .export_record_manifest(&second.id)
            .expect("owner manifest after replay")
            .evidence_refs,
        vec![reference.clone()]
    );
    assert_eq!(
        local
            .store
            .read_replica_object_manifest("org-a", &second.id, &second_manifest.sha256)
            .expect("scoped manifest after replay")
            .evidence_refs,
        vec![reference]
    );
    let first_hits = local
        .store
        .search(
            "first replay phrase",
            SearchMode::Text,
            &SearchFilters {
                project: None,
                client: None,
                source_namespace: None,
                scope: ContentScope::All,
                limit: 10,
            },
        )
        .expect("search after cursor recovery")
        .hits;
    assert!(first_hits
        .iter()
        .any(|hit| hit.id.as_str() == first.id.as_str()));
    let requests = server.join().expect("replay fixture worker");
    assert_eq!(requests.len(), 12);
}

fn sample_event(org_id: &str, epoch: &str, seq: u64) -> ReplicaEvent {
    ReplicaEvent {
        schema: "noesora.sync-event/v1".to_owned(),
        org_id: org_id.to_owned(),
        restore_epoch: epoch.to_owned(),
        seq,
        event_id: format!("event-{seq}"),
        commit_id: format!("commit-{seq}"),
        proposal_id: format!("proposal-{seq}"),
        operation: ReplicaOperation::Add,
        object_id: format!("nte_fixture_{seq}"),
        object_schema: "noesora.record/v1".to_owned(),
        sha256: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            .to_owned(),
        parent_sha256: None,
        evidence_refs: Vec::new(),
        actor_id: "member-a".to_owned(),
        decision_id: format!("decision-{seq}"),
        tombstone: None,
        previous_event_hash: zero_hash().to_owned(),
        object: None,
    }
}

fn zero_hash() -> &'static str {
    "sha256:0000000000000000000000000000000000000000000000000000000000000000"
}
