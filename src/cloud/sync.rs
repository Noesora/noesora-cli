use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use noesora_engine::{
    ReplicaCheckpoint, ReplicaEvent, ReplicaObject, ReplicaObjectKind, ReplicaObjectManifestV1,
    ReplicaOperation, ReplicaSnapshotObjectV1, ReplicaSnapshotV1, Store,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::api::{self, CloudApi, EventsPage, Identity, Organization, OrganizationStatus};
use super::auth::{self, CloudAccountV1};
use super::local;
use super::{CloudError, CloudOutput};

const WIRE_CHECKPOINT_SCHEMA: &str = "noesora.cli-wire-checkpoint/v1";
const OUTBOX_SCHEMA: &str = "noesora.cli-push-outbox/v1";
const PREVIEW_SCHEMA: &str = "noesora.cli-disclosure-preview/v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct WireCheckpointV1 {
    schema: String,
    org_id: String,
    restore_epoch: String,
    seq: u64,
    last_event_hash: String,
    cursor: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct SelectionV1 {
    records: Vec<String>,
    sessions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct BindingItemV1 {
    selection_kind: String,
    object_id: String,
    operation: String,
    manifest: ReplicaObjectManifestV1,
    target: Option<ObjectTargetV1>,
    source_refs: Vec<ObjectRefV1>,
    idempotency_key: String,
    upload_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ObjectRefV1 {
    object_id: String,
    sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ObjectTargetV1 {
    object_id: String,
    expected_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct PreviewBindingV1 {
    schema: String,
    api_origin: String,
    org_id: String,
    actor_user_id: String,
    active_org_id: Option<String>,
    membership: Organization,
    organization_status: OrganizationStatus,
    snapshot_sha256: String,
    restore_epoch: String,
    seq: u64,
    last_event_hash: String,
    base_cursor: String,
    selection: SelectionV1,
    items: Vec<BindingItemV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OutboxItemV1 {
    binding: BindingItemV1,
    payload_file: String,
    state: String,
    upload_id: Option<String>,
    proposal_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PushOutboxV1 {
    schema: String,
    preview_sha256: String,
    binding: PreviewBindingV1,
    items: Vec<OutboxItemV1>,
}

#[derive(Debug, Clone, Serialize)]
struct DisclosureItemV1 {
    selection_kind: String,
    object_id: String,
    operation: String,
    candidate_object: ObjectRefV1,
    target: Option<ObjectTargetV1>,
    source_refs: Vec<ObjectRefV1>,
    evidence_refs: Vec<noesora_engine::ReplicaEvidenceRef>,
    manifest: ReplicaObjectManifestV1,
    idempotency_key: String,
    upload_key: String,
    byte_length: usize,
    sha256: String,
    content_utf8: String,
}

#[derive(Debug, Clone, Serialize)]
struct DisclosureV1 {
    schema: String,
    proposal_schema: String,
    claimed_authors: Vec<String>,
    reference_scope: String,
    preview_sha256: String,
    recipient_org_id: String,
    actor_user_id: String,
    active_org_id: Option<String>,
    api_origin: String,
    membership: Organization,
    organization_status: OrganizationStatus,
    snapshot_sha256: String,
    restore_epoch: String,
    seq: u64,
    last_event_hash: String,
    base_cursor: String,
    warning: String,
    items: Vec<DisclosureItemV1>,
}

#[derive(Debug, Clone)]
struct SelectedObject {
    selection_kind: String,
    object: ReplicaObject,
    manifest: ReplicaObjectManifestV1,
    operation: String,
    target: Option<ObjectTargetV1>,
    source_refs: Vec<ObjectRefV1>,
}

struct CurrentContext {
    identity: Identity,
    membership: Organization,
    status: OrganizationStatus,
    snapshot: ReplicaSnapshotV1,
    cursor: String,
}

pub(crate) fn push(
    store: &Store,
    org_id: &str,
    records: &[String],
    sessions: &[String],
    confirm: Option<&str>,
) -> Result<CloudOutput, CloudError> {
    let (account, token) = auth::authenticated_session(store)?;
    push_with_token(store, &account, &token, org_id, records, sessions, confirm)
}

fn push_with_token(
    store: &Store,
    account: &CloudAccountV1,
    token: &str,
    org_id: &str,
    records: &[String],
    sessions: &[String],
    confirm: Option<&str>,
) -> Result<CloudOutput, CloudError> {
    let selection = normalized_selection(records, sessions)?;
    if selection.records.is_empty() && selection.sessions.is_empty() {
        return Err(CloudError::new(
            "validation",
            "CLOUD_SELECTION_REQUIRED",
            "Select at least one --record or --session ID.",
        ));
    }
    let api = CloudApi::new(&account.api_origin)?;
    if let Some(preview_digest) = confirm {
        confirm_push(
            store,
            &api,
            account,
            token,
            org_id,
            selection,
            preview_digest,
        )
    } else {
        prepare_push(store, &api, account, token, org_id, selection)
    }
}

fn normalized_selection(
    records: &[String],
    sessions: &[String],
) -> Result<SelectionV1, CloudError> {
    let mut record_ids = records.to_vec();
    let mut session_ids = sessions.to_vec();
    for id in record_ids.iter().chain(session_ids.iter()) {
        if id.trim().is_empty() || id.len() > 512 {
            return Err(CloudError::new(
                "validation",
                "INVALID_OBJECT",
                "Selected object IDs must be non-empty and within the shared identifier bound.",
            ));
        }
    }
    record_ids.sort();
    record_ids.dedup();
    session_ids.sort();
    session_ids.dedup();
    Ok(SelectionV1 {
        records: record_ids,
        sessions: session_ids,
    })
}

fn prepare_push(
    store: &Store,
    api: &CloudApi,
    account: &CloudAccountV1,
    token: &str,
    org_id: &str,
    selection: SelectionV1,
) -> Result<CloudOutput, CloudError> {
    let context = current_context(api, token, org_id)?;
    let selected = collect_selected_objects(store, org_id, &selection, &context.snapshot)?;
    ensure_reference_closure(&selected, &context.snapshot)?;
    let binding = make_binding(account, org_id, &context, &selection, &selected)?;
    let preview_sha256 = digest_json(&binding)?;
    let mut items = Vec::with_capacity(selected.len());
    let mut disclosures = Vec::with_capacity(selected.len());
    let mut payload_files = Vec::with_capacity(selected.len());
    for selected in selected {
        let payload_file = format!(
            "object-{}.bin",
            local::hash_name(
                format!("{}\0{}", selected.object.object_id, selected.object.sha256).as_bytes()
            )
        );
        let (idempotency_key, upload_key) = item_keys(&binding, &selected)?;
        let item_binding = BindingItemV1 {
            selection_kind: selected.selection_kind.clone(),
            object_id: selected.object.object_id.clone(),
            operation: selected.operation.clone(),
            manifest: selected.manifest.clone(),
            target: selected.target.clone(),
            source_refs: selected.source_refs.clone(),
            idempotency_key,
            upload_key,
        };
        let content_utf8 = std::str::from_utf8(&selected.object.bytes)
            .map_err(|_| {
                CloudError::integrity(
                    "Selected object bytes are not valid UTF-8 and cannot be reviewed as text.",
                )
            })?
            .to_owned();
        disclosures.push(DisclosureItemV1 {
            selection_kind: selected.selection_kind.clone(),
            object_id: selected.object.object_id.clone(),
            operation: selected.operation.clone(),
            candidate_object: ObjectRefV1 {
                object_id: selected.manifest.object_id.clone(),
                sha256: selected.manifest.sha256.clone(),
            },
            target: selected.target.clone(),
            source_refs: selected.source_refs.clone(),
            evidence_refs: selected.manifest.evidence_refs.clone(),
            manifest: selected.manifest.clone(),
            idempotency_key: item_binding.idempotency_key.clone(),
            upload_key: item_binding.upload_key.clone(),
            byte_length: selected.object.bytes.len(),
            sha256: selected.object.sha256.clone(),
            content_utf8,
        });
        items.push(OutboxItemV1 {
            binding: item_binding,
            payload_file: payload_file.clone(),
            state: if selected.operation == "already_current" {
                "already_current".to_owned()
            } else {
                "prepared".to_owned()
            },
            upload_id: None,
            proposal_id: None,
        });
        payload_files.push((payload_file, selected.object.bytes));
    }
    let disclosure = DisclosureV1 {
        schema: PREVIEW_SCHEMA.to_owned(),
        proposal_schema: "noesora.proposal/v1".to_owned(),
        claimed_authors: Vec::new(),
        reference_scope: "previously_verified_structured_references_only".to_owned(),
        preview_sha256: preview_sha256.clone(),
        recipient_org_id: org_id.to_owned(),
        actor_user_id: context.identity.user_id.clone(),
        active_org_id: context.identity.org_id.clone(),
        api_origin: account.api_origin.clone(),
        membership: context.membership.clone(),
        organization_status: context.status.clone(),
        snapshot_sha256: context.snapshot.sha256.clone(),
        restore_epoch: context.snapshot.restore_epoch.clone(),
        seq: context.snapshot.seq,
        last_event_hash: context.snapshot.last_event_hash.clone(),
        base_cursor: context.cursor.clone(),
        warning: "Only previously verified structured evidence references are included; prose and files are never parsed as citations, so no complete source-provenance claim is made. Selected bytes are sent to this organization. Embedded secrets may exist; no detector can guarantee their absence. Session selection discloses each complete raw snapshot.".to_owned(),
        items: disclosures,
    };
    let disclosure_bytes = serde_json::to_vec_pretty(&disclosure)
        .map_err(|_| CloudError::local_store("Could not encode the disclosure preview."))?;
    let outbox = PushOutboxV1 {
        schema: OUTBOX_SCHEMA.to_owned(),
        preview_sha256: preview_sha256.clone(),
        binding,
        items,
    };
    let directory = local::outbox_directory(store, &preview_sha256)?;
    let outbox_path = directory.join("outbox.json");
    match local::read_json::<PushOutboxV1>(&outbox_path)? {
        None => {
            for (name, bytes) in payload_files {
                local::write_file(&directory.join(name), &bytes)?;
            }
            local::write_file(&directory.join("disclosure.json"), &disclosure_bytes)?;
            local::write_json(&outbox_path, &outbox)?;
        }
        Some(existing) => {
            if existing.schema != OUTBOX_SCHEMA
                || existing.preview_sha256 != preview_sha256
                || existing.binding != outbox.binding
            {
                return Err(CloudError::integrity("An existing durable outbox does not match the newly computed disclosure binding."));
            }
            for (name, bytes) in payload_files {
                if local::read_file(&directory.join(name))? != bytes {
                    return Err(CloudError::integrity(
                        "An existing outbox payload differs from its immutable preview.",
                    ));
                }
            }
            if local::read_file(&directory.join("disclosure.json"))? != disclosure_bytes {
                return Err(CloudError::integrity(
                    "An existing disclosure preview differs from its immutable consent binding.",
                ));
            }
        }
    }
    let preview_path = directory.join("disclosure.json");
    let text = format!(
        "Prepared exact Cloud disclosure for organization {}. Preview SHA-256: {}\nReview the complete manifest and content at {}\n{}\nConfirm only after review: noesora cloud push --org {}{} --confirm {}",
        org_id,
        preview_sha256,
        preview_path.display(),
        if selection.sessions.is_empty() { "" } else { "WARNING: this includes complete raw session snapshots." },
        shell_quote(org_id),
        selection.records.iter().map(|id| format!(" --record {}", shell_quote(id))).chain(selection.sessions.iter().map(|id| format!(" --session {}", shell_quote(id)))).collect::<String>(),
        preview_sha256,
    );
    Ok(CloudOutput::success(
        "cloud push preview",
        json!({
            "organization_id": org_id,
            "actor_user_id": context.identity.user_id,
            "preview_sha256": preview_sha256,
            "preview_path": preview_path,
            "base_cursor": context.cursor,
            "items": outbox.items.iter().map(|item| json!({"kind":item.binding.selection_kind,"object_id":item.binding.object_id,"operation":item.binding.operation,"sha256":item.binding.manifest.sha256,"byte_length":item.binding.manifest.byte_length,"evidence_refs":item.binding.manifest.evidence_refs})).collect::<Vec<_>>(),
            "session_disclosure_warning": !selection.sessions.is_empty(),
            "confirmation_required": true,
        }),
        text,
    ))
}

fn confirm_push(
    store: &Store,
    api: &CloudApi,
    account: &CloudAccountV1,
    token: &str,
    org_id: &str,
    selection: SelectionV1,
    preview_digest: &str,
) -> Result<CloudOutput, CloudError> {
    if !api::valid_sha256(preview_digest) {
        return Err(CloudError::new(
            "validation",
            "INVALID_OBJECT",
            "Confirmation must be the exact `sha256:<64 lowercase hex>` preview digest.",
        ));
    }
    let directory = local::outbox_directory(store, preview_digest)?;
    let outbox_path = directory.join("outbox.json");
    let mut outbox = local::read_json::<PushOutboxV1>(&outbox_path)?.ok_or_else(|| {
        CloudError::confirmation(
            "No matching durable preview exists for this organization and digest.",
        )
    })?;
    if outbox.schema != OUTBOX_SCHEMA
        || outbox.preview_sha256 != preview_digest
        || outbox.binding.org_id != org_id
        || outbox.binding.api_origin != account.api_origin
        || outbox.binding.selection != selection
        || digest_json(&outbox.binding)? != preview_digest
    {
        return Err(CloudError::confirmation(
            "The confirmation digest, destination, selected IDs, or stored preview do not match.",
        ));
    }
    if outbox.items.iter().any(|item| item.state == "conflict") {
        return Err(CloudError::conflict("This outbox contains a preserved stale conflict; create and review a new base-bound preview before proposing again."));
    }
    if outbox.items.iter().all(|item| {
        matches!(
            item.state.as_str(),
            "already_current"
                | "review_pending"
                | "commit_pending"
                | "committed"
                | "rejected"
                | "withdrawn"
                | "applied_locally"
        )
    }) {
        return Ok(outbox_status_output(&outbox));
    }
    let context = current_context(api, token, org_id)?;
    if context.identity.user_id != outbox.binding.actor_user_id
        || context.identity.org_id != outbox.binding.active_org_id
        || context.membership != outbox.binding.membership
        || context.status != outbox.binding.organization_status
    {
        return Err(CloudError::confirmation("The authenticated actor, active organization, membership, or organization status changed after preview."));
    }
    let has_unknown_receipt = outbox
        .items
        .iter()
        .any(|item| item.state == "receipt_unknown");
    if !has_unknown_receipt {
        let refreshed_binding = make_binding(
            account,
            org_id,
            &context,
            &selection,
            &collect_selected_objects(store, org_id, &selection, &context.snapshot)?,
        )?;
        if digest_json(&refreshed_binding)? != preview_digest {
            return Err(CloudError::confirmation("Cloud destination, remote base cursor, manifest, or selected bytes changed after preview. Create and review a new preview."));
        }
    }
    let current_objects = reexport_selected(store, &selection)?;
    ensure_outbox_objects_unchanged(&directory, &outbox, &current_objects)?;
    ensure_reference_closure_from_bindings(&outbox.items, &context.snapshot, &selection)?;

    for index in 0..outbox.items.len() {
        if matches!(
            outbox.items[index].state.as_str(),
            "already_current"
                | "review_pending"
                | "commit_pending"
                | "committed"
                | "rejected"
                | "withdrawn"
                | "applied_locally"
        ) {
            continue;
        }
        let payload = local::read_file(&directory.join(&outbox.items[index].payload_file))?;
        let item = outbox.items[index].binding.clone();
        if payload.len() != item.manifest.byte_length
            || digest_bytes(&payload) != item.manifest.sha256
        {
            return Err(CloudError::integrity(
                "Durable outbox bytes no longer match their consented manifest.",
            ));
        }
        outbox.items[index].state = "receipt_unknown".to_owned();
        local::write_json(&outbox_path, &outbox)?;
        let upload = match api.create_object_upload(token, org_id, &item.manifest, &item.upload_key)
        {
            Ok(upload) => upload,
            Err(error) => return mark_receipt_unknown(&mut outbox, &outbox_path, index, error),
        };
        outbox.items[index].upload_id = Some(upload.upload_id.clone());
        outbox.items[index].state = "upload_admitted".to_owned();
        local::write_json(&outbox_path, &outbox)?;
        if upload.status == "awaiting_bytes" {
            outbox.items[index].state = "receipt_unknown".to_owned();
            local::write_json(&outbox_path, &outbox)?;
            match api.upload_object_bytes(token, org_id, &upload.upload_id, &item.manifest, payload)
            {
                Ok(()) => {
                    outbox.items[index].state = "bytes_uploaded".to_owned();
                    local::write_json(&outbox_path, &outbox)?;
                }
                Err(error) => return mark_receipt_unknown(&mut outbox, &outbox_path, index, error),
            }
        }
        let proposal = proposal_value(&item, &outbox.binding);
        outbox.items[index].state = "receipt_unknown".to_owned();
        local::write_json(&outbox_path, &outbox)?;
        match api.submit_proposal(token, org_id, &proposal) {
            Ok(view) => {
                let (proposal_id, state) = proposal_receipt(&view)?;
                outbox.items[index].proposal_id = Some(proposal_id);
                outbox.items[index].state = match state.as_str() {
                    "pending_review" => "review_pending".to_owned(),
                    "approved_pending_commit" => "commit_pending".to_owned(),
                    "committed" => "committed".to_owned(),
                    "stale" => "conflict".to_owned(),
                    "withdrawn" => "withdrawn".to_owned(),
                    "rejected" => "rejected".to_owned(),
                    _ => {
                        return Err(CloudError::integrity(
                            "Cloud returned an unsupported proposal state.",
                        ))
                    }
                };
                local::write_json(&outbox_path, &outbox)?;
                if outbox.items[index].state == "conflict" {
                    return Err(CloudError::conflict("Cloud accepted the proposal envelope but reported a stale causal base; the local bytes and remote head were preserved."));
                }
            }
            Err(error) => return mark_receipt_unknown(&mut outbox, &outbox_path, index, error),
        }
    }
    Ok(outbox_status_output(&outbox))
}

fn mark_receipt_unknown(
    outbox: &mut PushOutboxV1,
    path: &PathBuf,
    index: usize,
    error: CloudError,
) -> Result<CloudOutput, CloudError> {
    if matches!(
        error.code.as_str(),
        "STALE_BASE"
            | "OBJECT_ID_CONFLICT"
            | "IDEMPOTENCY_KEY_REUSE"
            | "PROPOSAL_REVISION_MISMATCH"
    ) {
        outbox.items[index].state = "conflict".to_owned();
        local::write_json(path, outbox)?;
        return Err(error);
    }
    if error.code == "CLOUD_OFFLINE" {
        outbox.items[index].state = "receipt_unknown".to_owned();
        local::write_json(path, outbox)?;
        let mut unknown = CloudError::new("receipt_unknown", "CLOUD_RECEIPT_UNKNOWN", "Cloud may have received this idempotent request but no receipt was observed. Re-run the same selected IDs with the same --confirm digest to replay the same keys.");
        unknown.http_status = error.http_status;
        return Err(unknown);
    }
    Err(error)
}

fn outbox_status_output(outbox: &PushOutboxV1) -> CloudOutput {
    let states = outbox
        .items
        .iter()
        .map(|item| {
            json!({
                "object_id": item.binding.object_id,
                "kind": item.binding.selection_kind,
                "state": item.state,
                "proposal_id": item.proposal_id,
            })
        })
        .collect::<Vec<_>>();
    let text = outbox
        .items
        .iter()
        .map(|item| {
            format!(
                "{} {}: {}{}",
                item.binding.selection_kind,
                item.binding.object_id,
                item.state,
                item.proposal_id
                    .as_ref()
                    .map(|id| format!(" ({id})"))
                    .unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    CloudOutput::success(
        "cloud push",
        json!({"organization_id": outbox.binding.org_id, "preview_sha256": outbox.preview_sha256, "items": states}),
        text,
    )
}

fn current_context(
    api: &CloudApi,
    token: &str,
    org_id: &str,
) -> Result<CurrentContext, CloudError> {
    if org_id.trim().is_empty() || org_id.len() > 512 {
        return Err(CloudError::new(
            "validation",
            "INVALID_OBJECT",
            "Organization ID is invalid.",
        ));
    }
    let identity = api.get_identity(token)?;
    let organizations = api.get_organizations(token)?;
    let membership = organizations
        .into_iter()
        .find(|item| item.id == org_id)
        .ok_or_else(|| {
            CloudError::new(
                "authorization",
                "ORG_MEMBERSHIP_REQUIRED",
                "The authenticated identity is not a current member of the requested organization.",
            )
        })?;
    let status = api.get_organization_status(token, org_id)?;
    if status.phase != "ready" || !status.customer_ready {
        let code = if status.phase == "restore_quarantined" {
            "TENANT_RESTORE_QUARANTINED"
        } else {
            "TENANT_NOT_READY"
        };
        return Err(CloudError::new("policy", code, "Organization data access is blocked until Cloud readiness and restore gates are satisfied."));
    }
    let snapshot = api.get_snapshot(token, org_id)?;
    let cursor = snapshot_tip_cursor(api, token, org_id, &snapshot)?;
    Ok(CurrentContext {
        identity,
        membership,
        status,
        snapshot,
        cursor,
    })
}

fn make_binding(
    account: &CloudAccountV1,
    org_id: &str,
    context: &CurrentContext,
    selection: &SelectionV1,
    selected: &[SelectedObject],
) -> Result<PreviewBindingV1, CloudError> {
    let mut items = Vec::with_capacity(selected.len());
    for selected in selected {
        let (idempotency_key, upload_key) = item_keys_for_parts(
            &account.api_origin,
            org_id,
            &context.identity.user_id,
            &context.cursor,
            &selected.manifest,
            &selected.operation,
            selected.target.as_ref(),
            &selected.source_refs,
        )?;
        items.push(BindingItemV1 {
            selection_kind: selected.selection_kind.clone(),
            object_id: selected.object.object_id.clone(),
            operation: selected.operation.clone(),
            manifest: selected.manifest.clone(),
            target: selected.target.clone(),
            source_refs: selected.source_refs.clone(),
            idempotency_key,
            upload_key,
        });
    }
    Ok(PreviewBindingV1 {
        schema: PREVIEW_SCHEMA.to_owned(),
        api_origin: account.api_origin.clone(),
        org_id: org_id.to_owned(),
        actor_user_id: context.identity.user_id.clone(),
        active_org_id: context.identity.org_id.clone(),
        membership: context.membership.clone(),
        organization_status: context.status.clone(),
        snapshot_sha256: context.snapshot.sha256.clone(),
        restore_epoch: context.snapshot.restore_epoch.clone(),
        seq: context.snapshot.seq,
        last_event_hash: context.snapshot.last_event_hash.clone(),
        base_cursor: context.cursor.clone(),
        selection: selection.clone(),
        items,
    })
}

fn item_keys(
    binding: &PreviewBindingV1,
    selected: &SelectedObject,
) -> Result<(String, String), CloudError> {
    item_keys_for_parts(
        &binding.api_origin,
        &binding.org_id,
        &binding.actor_user_id,
        &binding.base_cursor,
        &selected.manifest,
        &selected.operation,
        selected.target.as_ref(),
        &selected.source_refs,
    )
}

fn item_keys_for_parts(
    origin: &str,
    org: &str,
    actor: &str,
    cursor: &str,
    manifest: &ReplicaObjectManifestV1,
    operation: &str,
    target: Option<&ObjectTargetV1>,
    source_refs: &[ObjectRefV1],
) -> Result<(String, String), CloudError> {
    let proposal_seed = serde_json::to_vec(&(
        "noesora.cli-proposal-idempotency/v1",
        origin,
        org,
        actor,
        cursor,
        manifest,
        operation,
        target,
        source_refs,
    ))
    .map_err(|_| CloudError::integrity("Proposal idempotency binding could not be encoded."))?;
    let upload_seed = serde_json::to_vec(&(
        "noesora.cli-object-upload-idempotency/v1",
        origin,
        org,
        actor,
        cursor,
        manifest,
        operation,
        target,
        source_refs,
    ))
    .map_err(|_| {
        CloudError::integrity("Object upload idempotency binding could not be encoded.")
    })?;
    Ok((
        format!("noesora-cli-{}", local::hash_name(&proposal_seed)),
        format!("noesora-cli-{}", local::hash_name(&upload_seed)),
    ))
}

fn collect_selected_objects(
    store: &Store,
    org_id: &str,
    selection: &SelectionV1,
    snapshot: &ReplicaSnapshotV1,
) -> Result<Vec<SelectedObject>, CloudError> {
    let active = snapshot
        .active_objects
        .iter()
        .map(|head| (head.object_id.as_str(), head.sha256.as_str()))
        .collect::<BTreeMap<_, _>>();
    let historical = snapshot
        .objects
        .iter()
        .map(|manifest| (manifest.object_id.as_str(), manifest.sha256.as_str()))
        .collect::<BTreeSet<_>>();
    let mut selected = Vec::new();
    for id in &selection.records {
        let object = store.export_record(id).map_err(|_| {
            CloudError::new(
                "local",
                "LOCAL_OBJECT_UNAVAILABLE",
                "A selected local record could not be exported as canonical bytes.",
            )
        })?;
        let manifest = store.export_record_manifest(id).map_err(|_| {
            CloudError::new(
                "local",
                "LOCAL_OBJECT_UNAVAILABLE",
                "A selected record's verified structured manifest is unavailable.",
            )
        })?;
        validate_export_manifest(&object, &manifest)?;
        let (operation, target) = if active.get(id.as_str()).copied()
            == Some(object.sha256.as_str())
        {
            ("already_current".to_owned(), None)
        } else if let Some(parent) = &object.parent {
            if active.get(parent.object_id.as_str()).copied() != Some(parent.sha256.as_str()) {
                return Err(CloudError::conflict("A local correction parent is not the exact active organization head; no add or overwrite was inferred."));
            }
            let linked_parent = store.read_replica_object_manifest(org_id, &parent.object_id, &parent.sha256)
                .map_err(|_| CloudError::integrity("Engine could not prove the exact correction parent manifest was linked to this organization."))?;
            let remote_parent = snapshot.objects.iter().find(|candidate| {
                candidate.object_id == parent.object_id && candidate.sha256 == parent.sha256
            });
            if remote_parent != Some(&linked_parent) {
                return Err(CloudError::integrity("The locally linked correction parent manifest disagrees with the current organization snapshot."));
            }
            (
                "correct".to_owned(),
                Some(ObjectTargetV1 {
                    object_id: parent.object_id.clone(),
                    expected_sha256: parent.sha256.clone(),
                }),
            )
        } else if active.contains_key(id.as_str())
            || historical
                .iter()
                .any(|(object_id, _)| *object_id == id.as_str())
        {
            return Err(CloudError::conflict("A selected record already has different organization history and no causal correction metadata; no overwrite was inferred."));
        } else {
            ("add".to_owned(), None)
        };
        selected.push(SelectedObject {
            selection_kind: "record".to_owned(),
            object,
            manifest,
            operation,
            target,
            source_refs: Vec::new(),
        });
    }
    for id in &selection.sessions {
        let object = store.export_session(id).map_err(|_| {
            CloudError::new(
                "local",
                "LOCAL_OBJECT_UNAVAILABLE",
                "A selected session could not be exported as its complete raw snapshot.",
            )
        })?;
        let manifest = store.export_session_manifest(id).map_err(|_| {
            CloudError::new(
                "local",
                "LOCAL_OBJECT_UNAVAILABLE",
                "A selected session's verified structured manifest is unavailable.",
            )
        })?;
        validate_export_manifest(&object, &manifest)?;
        let operation = if active.get(id.as_str()).copied() == Some(object.sha256.as_str()) {
            "already_current"
        } else if active.contains_key(id.as_str())
            || historical
                .iter()
                .any(|(object_id, _)| *object_id == id.as_str())
        {
            return Err(CloudError::conflict("A selected session differs from organization history without explicit causal parent metadata; no overwrite was inferred."));
        } else {
            "share_session"
        };
        selected.push(SelectedObject {
            selection_kind: "session".to_owned(),
            object,
            manifest,
            operation: operation.to_owned(),
            target: None,
            source_refs: Vec::new(),
        });
    }
    selected.sort_by(|left, right| {
        (&left.selection_kind, &left.object.object_id)
            .cmp(&(&right.selection_kind, &right.object.object_id))
    });
    ensure_reference_closure(&selected, snapshot)?;
    Ok(selected)
}

fn reexport_selected(
    store: &Store,
    selection: &SelectionV1,
) -> Result<Vec<(String, String, ReplicaObjectManifestV1, Vec<u8>)>, CloudError> {
    let mut result = Vec::new();
    for id in &selection.records {
        let object = store.export_record(id).map_err(|_| {
            CloudError::new(
                "local",
                "LOCAL_OBJECT_UNAVAILABLE",
                "A selected local record could not be re-exported.",
            )
        })?;
        let manifest = store.export_record_manifest(id).map_err(|_| {
            CloudError::new(
                "local",
                "LOCAL_OBJECT_UNAVAILABLE",
                "A selected record's verified structured manifest is unavailable.",
            )
        })?;
        validate_export_manifest(&object, &manifest)?;
        result.push(("record".to_owned(), id.clone(), manifest, object.bytes));
    }
    for id in &selection.sessions {
        let object = store.export_session(id).map_err(|_| {
            CloudError::new(
                "local",
                "LOCAL_OBJECT_UNAVAILABLE",
                "A selected session snapshot could not be re-exported.",
            )
        })?;
        let manifest = store.export_session_manifest(id).map_err(|_| {
            CloudError::new(
                "local",
                "LOCAL_OBJECT_UNAVAILABLE",
                "A selected session's verified structured manifest is unavailable.",
            )
        })?;
        validate_export_manifest(&object, &manifest)?;
        result.push(("session".to_owned(), id.clone(), manifest, object.bytes));
    }
    result.sort_by(|left, right| (&left.0, &left.1).cmp(&(&right.0, &right.1)));
    Ok(result)
}

fn validate_export_manifest(
    object: &ReplicaObject,
    manifest: &ReplicaObjectManifestV1,
) -> Result<(), CloudError> {
    if object.schema != manifest.schema
        || object.kind != manifest.kind
        || object.object_id != manifest.object_id
        || object.object_schema != manifest.object_schema
        || object.relative_locator != manifest.relative_locator
        || object.sha256 != manifest.sha256
        || object.bytes.len() != manifest.byte_length
        || digest_bytes(&object.bytes) != manifest.sha256
        || object.source != manifest.source
        || object.parent != manifest.parent
    {
        return Err(CloudError::integrity(
            "Engine owner manifest does not match the exact canonical object export.",
        ));
    }
    api::parse_manifest(
        &serde_json::to_value(manifest)
            .map_err(|_| CloudError::integrity("Local object manifest could not be encoded."))?,
    )?;
    Ok(())
}

fn ensure_reference_closure(
    selected: &[SelectedObject],
    snapshot: &ReplicaSnapshotV1,
) -> Result<(), CloudError> {
    let selection = selected
        .iter()
        .map(|item| (item.object.object_id.as_str(), item.object.sha256.as_str()))
        .collect::<BTreeSet<_>>();
    let active = snapshot
        .active_objects
        .iter()
        .map(|head| (head.object_id.as_str(), head.sha256.as_str()))
        .collect::<BTreeSet<_>>();
    for item in selected {
        for reference in &item.manifest.evidence_refs {
            let selected_source =
                selection.contains(&(reference.object_id.as_str(), reference.sha256.as_str()));
            let linked_source =
                active.contains(&(reference.object_id.as_str(), reference.sha256.as_str()));
            if !selected_source && !linked_source {
                return Err(CloudError::new("integrity", "REFERENCE_CLOSURE_REQUIRED", "A selected object's evidence dependency is not selected or already linked to this organization; no object bytes were uploaded."));
            }
        }
    }
    Ok(())
}

fn ensure_reference_closure_from_bindings(
    items: &[OutboxItemV1],
    snapshot: &ReplicaSnapshotV1,
    selection: &SelectionV1,
) -> Result<(), CloudError> {
    let selected = selection
        .records
        .iter()
        .chain(selection.sessions.iter())
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let active = snapshot
        .active_objects
        .iter()
        .map(|head| (head.object_id.as_str(), head.sha256.as_str()))
        .collect::<BTreeSet<_>>();
    for item in items {
        for reference in &item.binding.manifest.evidence_refs {
            if !selected.contains(reference.object_id.as_str())
                && !active.contains(&(reference.object_id.as_str(), reference.sha256.as_str()))
            {
                return Err(CloudError::new("integrity", "REFERENCE_CLOSURE_REQUIRED", "A consented evidence dependency is neither explicitly selected nor already linked to this organization."));
            }
            if selected.contains(reference.object_id.as_str())
                && !items.iter().any(|candidate| {
                    candidate.binding.object_id == reference.object_id
                        && candidate.binding.manifest.sha256 == reference.sha256
                })
                && !active.contains(&(reference.object_id.as_str(), reference.sha256.as_str()))
            {
                return Err(CloudError::new("integrity", "REFERENCE_CLOSURE_REQUIRED", "A selected evidence source has a different digest than the referenced immutable version."));
            }
        }
    }
    Ok(())
}

fn ensure_outbox_objects_unchanged(
    directory: &PathBuf,
    outbox: &PushOutboxV1,
    current: &[(String, String, ReplicaObjectManifestV1, Vec<u8>)],
) -> Result<(), CloudError> {
    if outbox.items.len() != current.len() {
        return Err(CloudError::confirmation(
            "Selected object inventory changed after preview.",
        ));
    }
    for (item, (kind, id, manifest, bytes)) in outbox.items.iter().zip(current) {
        let saved = local::read_file(&directory.join(&item.payload_file))?;
        if item.binding.selection_kind != *kind
            || item.binding.object_id != *id
            || item.binding.manifest != *manifest
            || saved.as_slice() != bytes.as_slice()
        {
            return Err(CloudError::confirmation("A selected manifest or exact source byte changed after preview; no upload was started."));
        }
    }
    Ok(())
}

fn proposal_value(item: &BindingItemV1, binding: &PreviewBindingV1) -> Value {
    let operation = item.operation.as_str();
    let target = item.target.as_ref().map(
        |target| json!({"object_id": target.object_id, "expected_sha256": target.expected_sha256}),
    );
    json!({
        "schema": "noesora.proposal/v1",
        "idempotency_key": item.idempotency_key,
        "operation": operation,
        "candidate_object": {"object_id": item.manifest.object_id, "sha256": item.manifest.sha256},
        "target": target,
        "source_refs": item.source_refs,
        "evidence_refs": item.manifest.evidence_refs,
        "claimed_authors": [],
        "base_cursor": binding.base_cursor,
    })
}

fn proposal_receipt(value: &Value) -> Result<(String, String), CloudError> {
    api::exact_keys(
        value,
        &[
            "id",
            "revision",
            "state",
            "proposal",
            "submitted_by",
            "prepared_content",
            "base_content",
            "content_sha256",
            "approval",
            "decisions",
            "created_at",
            "updated_at",
            "commit_id",
            "error_code",
            "can_approve",
            "can_reject",
            "can_withdraw",
        ],
        "proposal view",
    )?;
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CloudError::integrity("Cloud proposal receipt omitted its ID."))?
        .to_owned();
    let state = value
        .get("state")
        .and_then(Value::as_str)
        .filter(|state| {
            matches!(
                *state,
                "pending_review"
                    | "approved_pending_commit"
                    | "committed"
                    | "stale"
                    | "rejected"
                    | "withdrawn"
            )
        })
        .ok_or_else(|| CloudError::integrity("Cloud proposal receipt has an unsupported state."))?
        .to_owned();
    Ok((id, state))
}

fn snapshot_tip_cursor(
    api: &CloudApi,
    token: &str,
    org_id: &str,
    snapshot: &ReplicaSnapshotV1,
) -> Result<String, CloudError> {
    let mut after: Option<String> = None;
    let mut previous_event_hash: Option<String> = None;
    let mut last_seq = 0u64;
    let mut last_hash = None;
    let mut seen = BTreeSet::new();
    loop {
        let page = api.get_events_page(token, org_id, after.as_deref())?;
        verify_page_cut(&page, snapshot, after.as_deref())?;
        for event in &page.events {
            let expected = last_seq.saturating_add(1);
            if event.seq != expected || event.seq > snapshot.seq {
                return Err(CloudError::integrity(
                    "Cloud event history is not contiguous through the snapshot cut.",
                ));
            }
            if previous_event_hash
                .as_deref()
                .is_some_and(|hash| event.previous_event_hash != hash)
            {
                return Err(CloudError::integrity("Cloud event hash chain is broken."));
            }
            previous_event_hash = Some(event.event_hash().map_err(CloudError::integrity)?);
            last_hash = previous_event_hash.clone();
            last_seq = event.seq;
        }
        if !page.has_more {
            if last_seq != snapshot.seq
                || (snapshot.seq > 0
                    && last_hash.as_deref() != Some(snapshot.last_event_hash.as_str()))
            {
                return Err(CloudError::conflict(
                    "Cloud event history changed while binding the snapshot base cursor.",
                ));
            }
            return Ok(page.next_cursor);
        }
        if page.next_cursor == after.clone().unwrap_or_default()
            || !seen.insert(page.next_cursor.clone())
        {
            return Err(CloudError::integrity(
                "Cloud event cursor repeated during pagination.",
            ));
        }
        after = Some(page.next_cursor);
    }
}

fn verify_page_cut(
    page: &EventsPage,
    snapshot: &ReplicaSnapshotV1,
    requested_after: Option<&str>,
) -> Result<(), CloudError> {
    if page.org_id != snapshot.org_id
        || page.restore_epoch != snapshot.restore_epoch
        || page.high_water_seq != snapshot.seq
        || page.after.as_deref() != requested_after
    {
        return Err(CloudError::conflict(
            "Cloud events changed during this operation; retry with a fresh preview or pull.",
        ));
    }
    Ok(())
}

fn digest_json<T: Serialize>(value: &T) -> Result<String, CloudError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| CloudError::integrity("Consent binding could not be canonically encoded."))?;
    Ok(digest_bytes(&bytes))
}

fn digest_bytes(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn shell_quote(value: &str) -> String {
    if value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'/'))
    {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

pub(crate) fn pull(store: &Store, org_id: &str) -> Result<CloudOutput, CloudError> {
    let (account, token) = auth::authenticated_session(store)?;
    pull_with_token(store, &account, &token, org_id)
}

fn pull_with_token(
    store: &Store,
    account: &CloudAccountV1,
    token: &str,
    org_id: &str,
) -> Result<CloudOutput, CloudError> {
    let api = CloudApi::new(&account.api_origin)?;
    let context = current_context(&api, token, org_id)?;
    let engine_checkpoint = store.replica_checkpoint(org_id).map_err(|_| {
        CloudError::local_store("Could not inspect the local organization sync receipt.")
    })?;
    let wire_path = local::checkpoint_path(store, org_id)?;
    let wire = local::read_json::<WireCheckpointV1>(&wire_path)?;
    if let Some(wire) = &wire {
        if wire.schema != WIRE_CHECKPOINT_SCHEMA
            || wire.org_id != org_id
            || wire.org_id.is_empty()
            || wire.org_id.len() > 512
            || wire.restore_epoch.is_empty()
            || wire.restore_epoch.len() > 512
            || wire.seq > noesora_engine::MAX_SAFE_SEQUENCE
            || !api::valid_sha256(&wire.last_event_hash)
        {
            return Err(CloudError::integrity(
                "Local Cloud wire checkpoint is malformed or belongs to another organization.",
            ));
        }
        api::validate_cursor(&wire.cursor)?;
    }
    if engine_checkpoint.is_none() && wire.is_none() {
        return replace_from_snapshot(store, &api, &token, &context, &wire_path);
    }
    let engine_checkpoint = engine_checkpoint.ok_or_else(|| {
        CloudError::integrity(
            "A wire checkpoint exists without its durable Engine application checkpoint.",
        )
    })?;
    if engine_checkpoint.restore_epoch == context.snapshot.restore_epoch
        && engine_checkpoint.seq == context.snapshot.seq
        && engine_checkpoint.last_event_hash == context.snapshot.last_event_hash
    {
        if let Some(wire_checkpoint) = &wire {
            if wire_checkpoint.restore_epoch == context.snapshot.restore_epoch
                && (wire_checkpoint.seq > engine_checkpoint.seq
                    || (wire_checkpoint.seq == engine_checkpoint.seq
                        && wire_checkpoint.last_event_hash != engine_checkpoint.last_event_hash))
            {
                return Err(CloudError::integrity("Local wire cursor is ahead of or inconsistent with the durable Engine receipt."));
            }
        }
        return seed_wire_checkpoint(&context, &wire_path);
    }
    if engine_checkpoint.restore_epoch != context.snapshot.restore_epoch {
        return replace_from_snapshot(store, &api, token, &context, &wire_path);
    }
    if let Some(wire_checkpoint) = wire {
        if wire_checkpoint.restore_epoch != context.snapshot.restore_epoch {
            return replace_from_snapshot(store, &api, token, &context, &wire_path);
        }
        if wire_checkpoint.seq > engine_checkpoint.seq || wire_checkpoint.seq > context.snapshot.seq
        {
            return Err(CloudError::integrity(
                "Local wire checkpoint is ahead of durable Engine or current Cloud history.",
            ));
        }
        if wire_checkpoint.seq == engine_checkpoint.seq
            && wire_checkpoint.last_event_hash != engine_checkpoint.last_event_hash
        {
            return Err(CloudError::integrity(
                "Local wire cursor and Engine receipt disagree at the same sequence.",
            ));
        }
        if wire_checkpoint.seq == context.snapshot.seq
            && wire_checkpoint.last_event_hash != context.snapshot.last_event_hash
        {
            return Err(CloudError::integrity(
                "Local wire cursor does not match the current Cloud snapshot cut.",
            ));
        }
        pull_events(
            store,
            &api,
            token,
            &context,
            &wire_path,
            Some(wire_checkpoint),
        )
    } else {
        // Rebuild only the wire cursor by exact replay; an existing Engine checkpoint is never reinitialized.
        let replay = replay_wire_start(org_id, &context.snapshot, &engine_checkpoint);
        pull_events(store, &api, token, &context, &wire_path, Some(replay))
    }
}

fn seed_wire_checkpoint(
    context: &CurrentContext,
    wire_path: &PathBuf,
) -> Result<CloudOutput, CloudError> {
    let wire = WireCheckpointV1 {
        schema: WIRE_CHECKPOINT_SCHEMA.to_owned(),
        org_id: context.snapshot.org_id.clone(),
        restore_epoch: context.snapshot.restore_epoch.clone(),
        seq: context.snapshot.seq,
        last_event_hash: context.snapshot.last_event_hash.clone(),
        cursor: context.cursor.clone(),
    };
    local::write_json(wire_path, &wire)?;
    Ok(CloudOutput::success(
        "cloud pull",
        json!({
            "organization_id": wire.org_id,
            "restore_epoch": wire.restore_epoch,
            "seq": wire.seq,
            "last_event_hash": wire.last_event_hash,
            "cursor": wire.cursor,
            "applied_events": 0,
            "status": "applied_locally",
            "cursor_recovered": true,
        }),
        format!(
            "Recovered the wire cursor from the durable Engine receipt through sequence {}.",
            wire.seq
        ),
    ))
}
fn replay_wire_start(
    org_id: &str,
    snapshot: &ReplicaSnapshotV1,
    checkpoint: &ReplicaCheckpoint,
) -> WireCheckpointV1 {
    WireCheckpointV1 {
        schema: WIRE_CHECKPOINT_SCHEMA.to_owned(),
        org_id: org_id.to_owned(),
        restore_epoch: snapshot.restore_epoch.clone(),
        seq: 0,
        last_event_hash: if checkpoint.seq == 0 {
            checkpoint.last_event_hash.clone()
        } else {
            String::new()
        },
        cursor: String::new(),
    }
}

fn replace_from_snapshot(
    store: &Store,
    api: &CloudApi,
    token: &str,
    context: &CurrentContext,
    wire_path: &PathBuf,
) -> Result<CloudOutput, CloudError> {
    let mut objects = Vec::with_capacity(context.snapshot.objects.len());
    for manifest in &context.snapshot.objects {
        let bytes = api.get_object_bytes(
            token,
            &context.snapshot.org_id,
            &manifest.object_id,
            &manifest.sha256,
        )?;
        if bytes.len() != manifest.byte_length || digest_bytes(&bytes) != manifest.sha256 {
            return Err(CloudError::integrity(
                "Snapshot object bytes did not match their exact manifest length and SHA-256.",
            ));
        }
        objects.push(ReplicaSnapshotObjectV1 {
            manifest: manifest.clone(),
            bytes,
        });
    }
    let cursor = context.cursor.clone();
    let checkpoint = store
        .replace_replica_snapshot(&context.snapshot, &objects)
        .map_err(|_| {
            CloudError::integrity(
                "Engine refused the verified snapshot; the local wire cursor was not advanced.",
            )
        })?;
    if checkpoint.org_id != context.snapshot.org_id
        || checkpoint.restore_epoch != context.snapshot.restore_epoch
        || checkpoint.seq != context.snapshot.seq
        || checkpoint.last_event_hash != context.snapshot.last_event_hash
    {
        return Err(CloudError::integrity(
            "Engine snapshot receipt did not match the verified Cloud cut.",
        ));
    }
    let wire = WireCheckpointV1 {
        schema: WIRE_CHECKPOINT_SCHEMA.to_owned(),
        org_id: context.snapshot.org_id.clone(),
        restore_epoch: context.snapshot.restore_epoch.clone(),
        seq: context.snapshot.seq,
        last_event_hash: context.snapshot.last_event_hash.clone(),
        cursor,
    };
    local::write_json(wire_path, &wire)?;
    Ok(CloudOutput::success("cloud pull", json!({"organization_id": wire.org_id, "restore_epoch": wire.restore_epoch, "seq": wire.seq, "last_event_hash": wire.last_event_hash, "applied": "snapshot"}), format!("Verified and applied organization snapshot through sequence {}. Local outbox was retained.", wire.seq)))
}

fn pull_events(
    store: &Store,
    api: &CloudApi,
    token: &str,
    context: &CurrentContext,
    wire_path: &PathBuf,
    mut checkpoint: Option<WireCheckpointV1>,
) -> Result<CloudOutput, CloudError> {
    let mut after = checkpoint.as_ref().and_then(|value| {
        if value.cursor.is_empty() {
            None
        } else {
            Some(value.cursor.clone())
        }
    });
    let mut seq = checkpoint.as_ref().map_or(0, |value| value.seq);
    let mut last_hash = checkpoint.as_ref().and_then(|value| {
        if value.last_event_hash.is_empty() {
            None
        } else {
            Some(value.last_event_hash.clone())
        }
    });
    let mut applied = 0usize;
    let mut seen_cursors = BTreeSet::new();
    loop {
        let page = match api.get_events_page(token, &context.snapshot.org_id, after.as_deref()) {
            Ok(page) => page,
            Err(error) if error.code == "CURSOR_EXPIRED" => {
                return replace_from_snapshot(store, api, token, context, wire_path)
            }
            Err(error) => return Err(error),
        };
        verify_page_cut(&page, &context.snapshot, after.as_deref())?;
        if page.high_water_seq < seq {
            return Err(CloudError::integrity(
                "Cloud event history is behind the durable local cursor.",
            ));
        }
        let mut page_events = Vec::with_capacity(page.events.len());
        let mut expected_seq = seq;
        let mut page_hash = last_hash.clone();
        for mut event in page.events.clone() {
            if event.seq != expected_seq.saturating_add(1) {
                return Err(CloudError::integrity(
                    "Cloud event sequence is not contiguous with the last complete local cursor.",
                ));
            }
            if page_hash
                .as_deref()
                .is_some_and(|hash| event.previous_event_hash != hash)
            {
                return Err(CloudError::integrity(
                    "Cloud event previous hash does not match the verified chain.",
                ));
            }
            let hash = event.event_hash().map_err(CloudError::integrity)?;
            if event.operation != ReplicaOperation::Remove {
                event.object = Some(download_event_object(
                    api,
                    token,
                    &context.snapshot.org_id,
                    &event,
                )?);
            } else if event.tombstone.is_none() {
                return Err(CloudError::integrity(
                    "Cloud removal event omitted its tombstone.",
                ));
            }
            page_hash = Some(hash);
            expected_seq = event.seq;
            page_events.push(event);
        }
        // Every event and its exact referenced object bytes are validated before this page mutates Engine state.
        for event in &page_events {
            let report = store.apply_replica_event(event).map_err(|_| CloudError::integrity("Engine refused a Cloud event because its causal or evidence validation failed."))?;
            if report.checkpoint.org_id != context.snapshot.org_id
                || report.checkpoint.restore_epoch != context.snapshot.restore_epoch
                || report.checkpoint.seq < event.seq
            {
                return Err(CloudError::integrity(
                    "Engine application receipt did not cover the verified Cloud event.",
                ));
            }
            if !report.duplicate {
                applied += 1;
            }
        }
        if let Some(hash) = page_hash.clone() {
            if expected_seq > seq {
                seq = expected_seq;
                last_hash = Some(hash);
            }
        }
        let next = page.next_cursor.clone();
        if page.has_more
            && (next == after.clone().unwrap_or_default() || !seen_cursors.insert(next.clone()))
        {
            return Err(CloudError::integrity(
                "Cloud event pagination did not advance.",
            ));
        }
        if !page.events.is_empty() || !page.has_more {
            let next_hash = last_hash
                .clone()
                .unwrap_or_else(|| context.snapshot.last_event_hash.clone());
            let wire = WireCheckpointV1 {
                schema: WIRE_CHECKPOINT_SCHEMA.to_owned(),
                org_id: context.snapshot.org_id.clone(),
                restore_epoch: context.snapshot.restore_epoch.clone(),
                seq,
                last_event_hash: next_hash,
                cursor: next.clone(),
            };
            local::write_json(wire_path, &wire)?;
            checkpoint = Some(wire);
        }
        if !page.has_more {
            break;
        }
        after = Some(next);
    }
    let final_checkpoint = store
        .replica_checkpoint(&context.snapshot.org_id)
        .map_err(|_| {
            CloudError::local_store("Could not verify the durable Engine checkpoint after pull.")
        })?
        .ok_or_else(|| {
            CloudError::integrity("Engine application checkpoint disappeared during pull.")
        })?;
    if final_checkpoint.restore_epoch != context.snapshot.restore_epoch
        || final_checkpoint.seq != context.snapshot.seq
        || final_checkpoint.last_event_hash != context.snapshot.last_event_hash
        || seq != context.snapshot.seq
        || last_hash
            .as_deref()
            .is_some_and(|value| value != context.snapshot.last_event_hash)
    {
        return Err(CloudError::conflict("Pull did not reach the verified high-water snapshot cut; the last complete wire cursor was retained."));
    }
    let cursor = checkpoint
        .as_ref()
        .map(|value| value.cursor.clone())
        .unwrap_or_default();
    Ok(CloudOutput::success("cloud pull", json!({
        "organization_id": context.snapshot.org_id,
        "restore_epoch": context.snapshot.restore_epoch,
        "seq": seq,
        "last_event_hash": context.snapshot.last_event_hash,
        "cursor": cursor,
        "applied_events": applied,
        "status": "applied_locally",
    }), format!("Applied {applied} verified Cloud event(s); local organization cursor is at sequence {seq}.")))
}

fn download_event_object(
    api: &CloudApi,
    token: &str,
    org_id: &str,
    event: &ReplicaEvent,
) -> Result<ReplicaObject, CloudError> {
    let manifest = api.get_object_manifest(token, org_id, &event.object_id, &event.sha256)?;
    if manifest.object_id != event.object_id
        || manifest.sha256 != event.sha256
        || manifest.object_schema != event.object_schema
        || manifest
            .parent
            .as_ref()
            .map(|parent| parent.sha256.as_str())
            != event.parent_sha256.as_deref()
        || manifest.evidence_refs != event.evidence_refs
    {
        return Err(CloudError::integrity(
            "Cloud event and immutable object manifest disagree.",
        ));
    }
    match (event.operation, manifest.kind) {
        (
            ReplicaOperation::Add | ReplicaOperation::Correct | ReplicaOperation::Combine,
            ReplicaObjectKind::Record,
        ) => {}
        (
            ReplicaOperation::ShareSession,
            ReplicaObjectKind::NativeSession | ReplicaObjectKind::NeutralSession,
        ) => {}
        _ => {
            return Err(CloudError::integrity(
                "Cloud event operation is incompatible with its object kind.",
            ))
        }
    }
    let bytes = api.get_object_bytes(token, org_id, &event.object_id, &event.sha256)?;
    if bytes.len() != manifest.byte_length || digest_bytes(&bytes) != manifest.sha256 {
        return Err(CloudError::integrity(
            "Cloud object bytes did not match their exact manifest length and SHA-256.",
        ));
    }
    Ok(ReplicaObject {
        schema: manifest.schema,
        kind: manifest.kind,
        object_id: manifest.object_id,
        object_schema: manifest.object_schema,
        relative_locator: manifest.relative_locator,
        sha256: manifest.sha256,
        source: manifest.source,
        parent: manifest.parent,
        bytes,
    })
}
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    fn account(api_origin: &str) -> CloudAccountV1 {
        CloudAccountV1 {
            schema: "noesora.cloud-account/v1".to_owned(),
            api_origin: api_origin.to_owned(),
            issuer: api_origin.to_owned(),
            client_id: "fixture-public-client".to_owned(),
            required_scopes: vec!["user:org:read".to_owned()],
        }
    }

    pub(crate) fn push_with_token(
        store: &Store,
        api_origin: &str,
        token: &str,
        org_id: &str,
        records: &[String],
        sessions: &[String],
        confirm: Option<&str>,
    ) -> Result<CloudOutput, CloudError> {
        super::push_with_token(
            store,
            &account(api_origin),
            token,
            org_id,
            records,
            sessions,
            confirm,
        )
    }

    pub(crate) fn pull_with_token(
        store: &Store,
        api_origin: &str,
        token: &str,
        org_id: &str,
    ) -> Result<CloudOutput, CloudError> {
        super::pull_with_token(store, &account(api_origin), token, org_id)
    }
    pub(crate) fn wire_checkpoint_cut(
        store: &Store,
        org_id: &str,
    ) -> Result<Option<(u64, String, String)>, CloudError> {
        let checkpoint =
            local::read_json::<WireCheckpointV1>(&local::checkpoint_path(store, org_id)?)?;
        Ok(checkpoint.map(|checkpoint| {
            (
                checkpoint.seq,
                checkpoint.last_event_hash,
                checkpoint.cursor,
            )
        }))
    }

    pub(crate) fn reference_closure_refusal(
        store: &Store,
        source_id: &str,
        source_sha256: &str,
        evidence_kind: noesora_engine::ReplicaEvidenceKind,
    ) -> Result<(), CloudError> {
        let object = store
            .export_record(source_id)
            .map_err(|_| CloudError::local_store("Test object was unavailable."))?;
        let mut manifest = store
            .export_record_manifest(source_id)
            .map_err(|_| CloudError::local_store("Test record manifest was unavailable."))?;
        validate_export_manifest(&object, &manifest)?;
        manifest
            .evidence_refs
            .push(noesora_engine::ReplicaEvidenceRef {
                schema: "noesora.evidence-ref/v1".to_owned(),
                object_id: "unselected-session".to_owned(),
                sha256: source_sha256.to_owned(),
                locator_kind: evidence_kind,
                byte_range: Some([0, 1]),
                json_pointer: (evidence_kind == noesora_engine::ReplicaEvidenceKind::SessionEvent)
                    .then(String::new),
                text_range: (evidence_kind == noesora_engine::ReplicaEvidenceKind::SessionEvent)
                    .then_some([0, 1]),
            });
        let selected = vec![SelectedObject {
            selection_kind: "record".to_owned(),
            object,
            manifest,
            operation: "add".to_owned(),
            target: None,
            source_refs: Vec::new(),
        }];
        let snapshot = ReplicaSnapshotV1 {
            schema: "noesora.sync-snapshot/v1".to_owned(),
            org_id: "fixture-org".to_owned(),
            restore_epoch: "fixture-epoch".to_owned(),
            seq: 0,
            last_event_hash:
                "sha256:0000000000000000000000000000000000000000000000000000000000000000".to_owned(),
            objects: Vec::new(),
            active_objects: Vec::new(),
            tombstones: Vec::new(),
            sha256: String::new(),
        };
        ensure_reference_closure(&selected, &snapshot)
    }
}
