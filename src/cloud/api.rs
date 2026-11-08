use std::time::Duration;

use noesora_engine::{ReplicaObjectManifestV1, ReplicaSnapshotV1};
use reqwest::blocking::{Client, Response};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Method, Url};
use serde_json::Value;

use super::CloudError;

const MAX_METADATA_BYTES: usize = 64 * 1024;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const EVENT_PAGE_LIMIT: usize = 256;

#[derive(Clone)]
pub(crate) struct CloudApi {
    origin: Url,
    client: Client,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct AuthMetadata {
    pub(crate) schema: String,
    pub(crate) issuer: String,
    pub(crate) client_id: String,
    pub(crate) api_origin: String,
    pub(crate) required_scopes: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Identity {
    pub(crate) schema: String,
    pub(crate) user_id: String,
    pub(crate) org_id: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Organization {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) status: String,
    pub(crate) region: String,
    pub(crate) steward: bool,
    pub(crate) can_manage: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct OrganizationStatus {
    pub(crate) org_id: String,
    pub(crate) phase: String,
    pub(crate) customer_ready: bool,
    pub(crate) region: String,
    pub(crate) restore_verified_at: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EventsPage {
    pub(crate) schema: String,
    pub(crate) org_id: String,
    pub(crate) restore_epoch: String,
    pub(crate) after: Option<String>,
    pub(crate) high_water_seq: u64,
    pub(crate) events: Vec<noesora_engine::ReplicaEvent>,
    pub(crate) next_cursor: String,
    pub(crate) has_more: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct UploadResponse {
    pub(crate) upload_id: String,
    pub(crate) status: String,
    pub(crate) sha256: String,
    pub(crate) expires_at: String,
}

impl CloudApi {
    pub(crate) fn new(origin: &str) -> Result<Self, CloudError> {
        let origin = parse_origin(origin)?;
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("noesora/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| {
                CloudError::new(
                    "transport",
                    "CLOUD_CLIENT_FAILED",
                    "Could not initialize the Cloud HTTPS client.",
                )
            })?;
        Ok(Self { origin, client })
    }

    pub(crate) fn origin(&self) -> &str {
        self.origin.as_str().trim_end_matches('/')
    }

    pub(crate) fn endpoint(&self, path: &str) -> Result<Url, CloudError> {
        self.origin
            .join(path.trim_start_matches('/'))
            .map_err(|_| CloudError::integrity("Cloud returned an invalid endpoint."))
    }

    pub(crate) fn get_auth_metadata(&self) -> Result<AuthMetadata, CloudError> {
        let value = self.request_json(Method::GET, "/v1/auth/metadata", None, None, None)?;
        exact_keys(
            &value,
            &[
                "schema",
                "issuer",
                "client_id",
                "api_origin",
                "required_scopes",
            ],
            "authentication metadata",
        )?;
        let metadata: AuthMetadata = decode(value, "authentication metadata")?;
        if metadata.schema != "noesora.auth-metadata/v1"
            || metadata.issuer.is_empty()
            || metadata.client_id.is_empty()
            || metadata.required_scopes.len() != 1
            || metadata
                .required_scopes
                .first()
                .is_none_or(|scope| scope.as_str() != "user:org:read")
            || parse_origin(&metadata.api_origin)?.as_str() != self.origin.as_str()
        {
            return Err(CloudError::integrity("Cloud authentication metadata did not match the selected API origin or required scope."));
        }
        Ok(metadata)
    }

    pub(crate) fn get_identity(&self, token: &str) -> Result<Identity, CloudError> {
        let value = self.request_json(Method::GET, "/v1/me", Some(token), None, None)?;
        exact_keys(&value, &["schema", "user_id", "org_id"], "identity")?;
        let identity: Identity = decode(value, "identity")?;
        if identity.schema != "noesora.identity/v1" || identity.user_id.trim().is_empty() {
            return Err(CloudError::integrity("Cloud identity response is invalid."));
        }
        Ok(identity)
    }

    pub(crate) fn get_organizations(&self, token: &str) -> Result<Vec<Organization>, CloudError> {
        let mut organizations = Vec::new();
        let mut offset: Option<usize> = None;
        let mut seen_offsets = std::collections::HashSet::new();
        let mut seen_ids = std::collections::HashSet::new();
        loop {
            let path = match offset {
                Some(value) => format!("/v1/me/organizations?offset={value}"),
                None => "/v1/me/organizations".to_owned(),
            };
            let value = self.request_json(Method::GET, &path, Some(token), None, None)?;
            exact_keys(
                &value,
                &["organizations", "next_offset"],
                "organization list",
            )?;
            let object = value.as_object().ok_or_else(|| {
                CloudError::integrity("Cloud organization list is not an object.")
            })?;
            let items = object
                .get("organizations")
                .and_then(Value::as_array)
                .ok_or_else(|| CloudError::integrity("Cloud organization list is not an array."))?;
            for item in items {
                exact_keys(
                    item,
                    &["id", "name", "status", "region", "steward", "can_manage"],
                    "organization",
                )?;
                let organization: Organization = decode(item.clone(), "organization")?;
                if organization.id.is_empty()
                    || !matches!(
                        organization.status.as_str(),
                        "invited" | "provisioning" | "ready" | "failed" | "restore_quarantined"
                    )
                    || !seen_ids.insert(organization.id.clone())
                {
                    return Err(CloudError::integrity(
                        "Cloud organization list contains an invalid or repeated organization.",
                    ));
                }
                organizations.push(organization);
            }
            let next = object.get("next_offset").ok_or_else(|| {
                CloudError::integrity("Cloud organization list omitted its pagination cursor.")
            })?;
            if next.is_null() {
                return Ok(organizations);
            }
            let next = next
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| {
                    CloudError::integrity("Cloud organization pagination cursor is invalid.")
                })?;
            if Some(next) == offset || !seen_offsets.insert(next) {
                return Err(CloudError::integrity(
                    "Cloud organization pagination did not advance.",
                ));
            }
            offset = Some(next);
        }
    }

    pub(crate) fn get_organization_status(
        &self,
        token: &str,
        org_id: &str,
    ) -> Result<OrganizationStatus, CloudError> {
        let path = format!("/v1/orgs/{}/status", path_segment(org_id));
        let value = self.request_json(Method::GET, &path, Some(token), None, None)?;
        exact_keys(
            &value,
            &[
                "org_id",
                "phase",
                "customer_ready",
                "region",
                "restore_verified_at",
            ],
            "organization status",
        )?;
        let status: OrganizationStatus = decode(value, "organization status")?;
        if status.org_id != org_id
            || !matches!(
                status.phase.as_str(),
                "provisioning" | "ready" | "failed" | "restore_quarantined" | "not_configured"
            )
        {
            return Err(CloudError::integrity(
                "Cloud organization status does not match the requested organization.",
            ));
        }
        Ok(status)
    }

    pub(crate) fn get_snapshot(
        &self,
        token: &str,
        org_id: &str,
    ) -> Result<ReplicaSnapshotV1, CloudError> {
        let path = format!("/v1/orgs/{}/sync/snapshot", path_segment(org_id));
        let value = self.request_json(Method::GET, &path, Some(token), None, None)?;
        strict_snapshot(&value)?;
        let snapshot: ReplicaSnapshotV1 = decode(value, "sync snapshot")?;
        snapshot.validate().map_err(CloudError::integrity)?;
        if snapshot.org_id != org_id {
            return Err(CloudError::integrity(
                "Cloud sync snapshot belongs to another organization.",
            ));
        }
        Ok(snapshot)
    }

    pub(crate) fn get_events_page(
        &self,
        token: &str,
        org_id: &str,
        after: Option<&str>,
    ) -> Result<EventsPage, CloudError> {
        let mut path = format!(
            "/v1/orgs/{}/sync/events?limit={EVENT_PAGE_LIMIT}",
            path_segment(org_id)
        );
        if let Some(after) = after {
            path.push_str("&after=");
            path.push_str(&query_value(after));
        }
        let value = self.request_json(Method::GET, &path, Some(token), None, None)?;
        strict_event_page(&value)?;
        let page: EventsPage = decode(value, "sync event page")?;
        if let Some(after) = page.after.as_deref() {
            validate_cursor(after)?;
        }
        validate_cursor(&page.next_cursor)?;
        for (index, event) in page.events.iter().enumerate() {
            if event.schema != "noesora.sync-event/v1"
                || event.org_id != page.org_id
                || event.restore_epoch != page.restore_epoch
                || event.seq > page.high_water_seq
                || (index > 0 && event.seq != page.events[index - 1].seq.saturating_add(1))
            {
                return Err(CloudError::integrity(
                    "Cloud sync event page has invalid scope or ordering.",
                ));
            }
            validate_event_schema(event)?;
        }
        if page.schema != "noesora.sync-page/v1"
            || page.org_id != org_id
            || page.after.as_deref() != after
            || page.events.len() > EVENT_PAGE_LIMIT
            || (after.is_none()
                && page.high_water_seq > 0
                && page.events.first().is_some_and(|event| event.seq != 1))
            || (page.has_more
                && (page.events.is_empty()
                    || page
                        .events
                        .last()
                        .is_some_and(|event| event.seq >= page.high_water_seq)
                    || Some(page.next_cursor.as_str()) == after))
            || (!page.has_more
                && page
                    .events
                    .last()
                    .is_some_and(|event| event.seq != page.high_water_seq))
            || (after.is_none() && page.high_water_seq > 0 && page.events.is_empty())
        {
            return Err(CloudError::integrity(
                "Cloud sync event page did not match the requested cursor or high-water cut.",
            ));
        }
        Ok(page)
    }

    pub(crate) fn get_object_manifest(
        &self,
        token: &str,
        org_id: &str,
        object_id: &str,
        sha256: &str,
    ) -> Result<ReplicaObjectManifestV1, CloudError> {
        let path = format!(
            "/v1/orgs/{}/objects/{}/manifest?sha256={}",
            path_segment(org_id),
            path_segment(object_id),
            query_value(sha256),
        );
        let value = self.request_json(Method::GET, &path, Some(token), None, None)?;
        let manifest = parse_manifest(&value)?;
        if manifest.object_id != object_id || manifest.sha256 != sha256 {
            return Err(CloudError::integrity(
                "Cloud object manifest identity did not match the requested ID and digest.",
            ));
        }
        Ok(manifest)
    }

    pub(crate) fn get_object_bytes(
        &self,
        token: &str,
        org_id: &str,
        object_id: &str,
        sha256: &str,
    ) -> Result<Vec<u8>, CloudError> {
        let path = format!(
            "/v1/orgs/{}/objects/{}?sha256={}",
            path_segment(org_id),
            path_segment(object_id),
            query_value(sha256),
        );
        let response = self.request(Method::GET, &path, Some(token), None, None, None)?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let value: Value = response.json().map_err(|_| {
                CloudError::new(
                    "http",
                    "CLOUD_HTTP_ERROR",
                    "Cloud object download failed with an invalid error response.",
                )
                .with_http(status, None)
            })?;
            return Err(public_error(status, value));
        }
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        if content_type
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .eq_ignore_ascii_case("application/octet-stream")
        {
            response
                .bytes()
                .map(|bytes| bytes.to_vec())
                .map_err(|_| CloudError::offline("Cloud object download was interrupted."))
        } else {
            Err(CloudError::integrity(
                "Cloud object response was not application/octet-stream.",
            ))
        }
    }

    pub(crate) fn create_object_upload(
        &self,
        token: &str,
        org_id: &str,
        manifest: &ReplicaObjectManifestV1,
        key: &str,
    ) -> Result<UploadResponse, CloudError> {
        let path = format!("/v1/orgs/{}/sync/objects", path_segment(org_id));
        let value =
            self.request_json(
                Method::POST,
                &path,
                Some(token),
                Some(serde_json::to_value(manifest).map_err(|_| {
                    CloudError::integrity("Object manifest could not be serialized.")
                })?),
                Some(key),
            )?;
        exact_keys(
            &value,
            &["upload_id", "status", "sha256", "expires_at"],
            "object upload",
        )?;
        let upload: UploadResponse = decode(value, "object upload")?;
        if upload.upload_id.is_empty()
            || !matches!(upload.status.as_str(), "awaiting_bytes" | "already_present")
            || upload.sha256 != manifest.sha256
        {
            return Err(CloudError::integrity(
                "Cloud object admission did not match the selected manifest.",
            ));
        }
        Ok(upload)
    }

    pub(crate) fn upload_object_bytes(
        &self,
        token: &str,
        org_id: &str,
        upload_id: &str,
        manifest: &ReplicaObjectManifestV1,
        bytes: Vec<u8>,
    ) -> Result<(), CloudError> {
        let path = format!(
            "/v1/orgs/{}/sync/objects/{}/bytes",
            path_segment(org_id),
            path_segment(upload_id)
        );
        let response = self.request(Method::PUT, &path, Some(token), None, None, Some(bytes))?;
        let value = response_json(response)?;
        exact_keys(&value, &["object", "manifest"], "object upload receipt")?;
        let object = value.get("object").ok_or_else(|| {
            CloudError::integrity("Cloud byte receipt omitted its object reference.")
        })?;
        exact_keys(object, &["object_id", "sha256"], "object reference")?;
        if object.get("object_id").and_then(Value::as_str) != Some(manifest.object_id.as_str())
            || object.get("sha256").and_then(Value::as_str) != Some(manifest.sha256.as_str())
        {
            return Err(CloudError::integrity(
                "Cloud byte receipt belongs to a different object.",
            ));
        }
        let received: ReplicaObjectManifestV1 =
            parse_manifest(value.get("manifest").ok_or_else(|| {
                CloudError::integrity("Cloud byte receipt omitted its manifest.")
            })?)?;
        if &received != manifest {
            return Err(CloudError::integrity(
                "Cloud byte receipt manifest changed after upload.",
            ));
        }
        Ok(())
    }

    pub(crate) fn submit_proposal(
        &self,
        token: &str,
        org_id: &str,
        proposal: &Value,
    ) -> Result<Value, CloudError> {
        let path = format!("/v1/orgs/{}/proposals", path_segment(org_id));
        let value = self.request_json(
            Method::POST,
            &path,
            Some(token),
            Some(proposal.clone()),
            None,
        )?;
        exact_keys(
            &value,
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
        let returned_proposal = value
            .get("proposal")
            .ok_or_else(|| CloudError::integrity("Cloud proposal receipt omitted its proposal."))?;
        exact_keys(
            returned_proposal,
            &[
                "schema",
                "idempotency_key",
                "operation",
                "candidate_object",
                "target",
                "source_refs",
                "evidence_refs",
                "claimed_authors",
                "base_cursor",
            ],
            "proposal receipt",
        )?;
        if returned_proposal != proposal {
            return Err(CloudError::integrity(
                "Cloud proposal receipt did not preserve the exact submitted proposal.",
            ));
        }
        Ok(value)
    }

    pub(crate) fn request_json(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
        idempotency_key: Option<&str>,
    ) -> Result<Value, CloudError> {
        let response = self.request(method, path, token, body, idempotency_key, None)?;
        response_json(response)
    }

    fn request(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
        idempotency_key: Option<&str>,
        raw: Option<Vec<u8>>,
    ) -> Result<Response, CloudError> {
        let endpoint = self.endpoint(path)?;
        let mut request = self
            .client
            .request(method, endpoint)
            .header(ACCEPT, "application/json");
        if let Some(token) = token {
            request = request.header(AUTHORIZATION, format!("Bearer {token}"));
        }
        if let Some(key) = idempotency_key {
            request = request.header("Idempotency-Key", key);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        if let Some(raw) = raw {
            request = request
                .header(CONTENT_TYPE, "application/octet-stream")
                .body(raw);
        }
        let response = request.send().map_err(|error| {
            if error.is_timeout() {
                CloudError::offline("Cloud request timed out; its remote receipt may be unknown.")
            } else {
                CloudError::offline("Cloud could not be reached; no success was assumed.")
            }
        })?;
        Ok(response)
    }
}
fn response_json(response: Response) -> Result<Value, CloudError> {
    let status = response.status();
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if !content_type.to_ascii_lowercase().contains("json") {
        return Err(CloudError::integrity(
            "Cloud returned a non-JSON response for a JSON endpoint.",
        ));
    }
    let bytes = response
        .bytes()
        .map_err(|_| CloudError::offline("Cloud response was interrupted."))?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| CloudError::integrity("Cloud returned malformed JSON."))?;
    if !status.is_success() {
        return Err(public_error(status.as_u16(), value));
    }
    Ok(value)
}

pub(crate) fn parse_origin(value: &str) -> Result<Url, CloudError> {
    let url = Url::parse(value).map_err(|_| {
        CloudError::new(
            "validation",
            "INVALID_CLOUD_ORIGIN",
            "Cloud origin must be an absolute HTTPS origin.",
        )
    })?;
    let loopback = url.host_str().is_some_and(|host| {
        host == "localhost" || host == "127.0.0.1" || host == "[::1]" || host == "::1"
    });
    if (url.scheme() != "https" && !(url.scheme() == "http" && loopback))
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || (url.path() != "/" && !url.path().is_empty())
    {
        return Err(CloudError::new(
            "validation",
            "INVALID_CLOUD_ORIGIN",
            "Cloud origin must use HTTPS and contain no credentials, path, query, or fragment.",
        ));
    }
    Ok(url)
}

pub(crate) fn endpoint_is_secure(value: &str) -> bool {
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    let loopback = url.host_str().is_some_and(|host| {
        host == "localhost" || host == "127.0.0.1" || host == "::1" || host == "[::1]"
    });
    (url.scheme() == "https" || (url.scheme() == "http" && loopback))
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
}

pub(crate) fn parse_manifest(value: &Value) -> Result<ReplicaObjectManifestV1, CloudError> {
    exact_keys(
        value,
        &[
            "schema",
            "kind",
            "object_id",
            "object_schema",
            "relative_locator",
            "sha256",
            "byte_length",
            "source",
            "parent",
            "evidence_refs",
        ],
        "object manifest",
    )?;
    if serde_json::to_vec(value)
        .map_err(|_| CloudError::integrity("Object manifest could not be encoded."))?
        .len()
        > MAX_METADATA_BYTES
    {
        return Err(CloudError::integrity(
            "Object manifest exceeds the shared metadata bound.",
        ));
    }
    let object = value
        .as_object()
        .ok_or_else(|| CloudError::integrity("Object manifest is not an object."))?;
    if object.get("schema").and_then(Value::as_str) != Some("noesora.sync-object/v1") {
        return Err(CloudError::integrity(
            "Object manifest schema is unsupported.",
        ));
    }
    let kind = object
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| CloudError::integrity("Object manifest kind is invalid."))?;
    if !matches!(kind, "record" | "native_session" | "neutral_session") {
        return Err(CloudError::integrity(
            "Object manifest kind is unsupported.",
        ));
    }
    let locator = object
        .get("relative_locator")
        .and_then(Value::as_str)
        .ok_or_else(|| CloudError::integrity("Object locator is invalid."))?;
    let id = object
        .get("object_id")
        .and_then(Value::as_str)
        .ok_or_else(|| CloudError::integrity("Object ID is invalid."))?;
    let digest = object
        .get("sha256")
        .and_then(Value::as_str)
        .ok_or_else(|| CloudError::integrity("Object digest is invalid."))?;
    let schema = object
        .get("object_schema")
        .and_then(Value::as_str)
        .ok_or_else(|| CloudError::integrity("Object schema is invalid."))?;
    let byte_length = object
        .get("byte_length")
        .and_then(Value::as_u64)
        .filter(|length| *length <= MAX_SAFE_INTEGER)
        .and_then(|length| usize::try_from(length).ok())
        .ok_or_else(|| {
            CloudError::integrity("Object manifest byte length is not a safe integer.")
        })?;
    if id.is_empty() || id.len() > 512 || !valid_sha256(digest) || !safe_locator(locator) {
        return Err(CloudError::integrity(
            "Object manifest identity or locator is invalid.",
        ));
    }
    match kind {
        "record" if !matches!(schema, "noesora.record/v1" | "noesora.record/v2") => {
            return Err(CloudError::integrity(
                "Record object schema is unsupported.",
            ))
        }
        "neutral_session" if schema != "noesora.session/v1" => {
            return Err(CloudError::integrity(
                "Neutral session object schema is unsupported.",
            ))
        }
        "native_session"
            if !schema.ends_with("/v1")
                || schema.starts_with("noesora.record/")
                || schema == "noesora.session/v1" =>
        {
            return Err(CloudError::integrity(
                "Native session object schema is unsupported.",
            ))
        }
        _ => {}
    }
    let source = object
        .get("source")
        .ok_or_else(|| CloudError::integrity("Object manifest omitted its source."))?;
    exact_keys(
        source,
        &[
            "client",
            "source_namespace",
            "external_id",
            "project",
            "created_at",
            "updated_at",
        ],
        "object source",
    )?;
    let project = source
        .get("project")
        .and_then(Value::as_str)
        .ok_or_else(|| CloudError::integrity("Object source project is invalid."))?;
    if project.is_empty() || project.len() > 16_384 {
        return Err(CloudError::integrity("Object source project is invalid."));
    }
    for field in [
        "client",
        "source_namespace",
        "external_id",
        "created_at",
        "updated_at",
    ] {
        if source.get(field).is_some_and(|value| {
            !value.is_null() && value.as_str().is_none_or(|text| text.len() > 4096)
        }) {
            return Err(CloudError::integrity(
                "Object source metadata exceeds its shared string bound.",
            ));
        }
    }
    if kind == "record"
        && [
            "client",
            "source_namespace",
            "external_id",
            "created_at",
            "updated_at",
        ]
        .iter()
        .any(|field| source.get(*field).is_none_or(|value| !value.is_null()))
    {
        return Err(CloudError::integrity(
            "Record source metadata must remain explicitly null.",
        ));
    }
    if kind == "neutral_session"
        && (source.get("client").is_none_or(|value| !value.is_null())
            || source
                .get("source_namespace")
                .and_then(Value::as_str)
                .is_none_or(|value| value.is_empty())
            || source
                .get("external_id")
                .and_then(Value::as_str)
                .is_none_or(|value| value.is_empty()))
    {
        return Err(CloudError::integrity(
            "Neutral session source metadata is invalid.",
        ));
    }
    if kind == "native_session"
        && source
            .get("client")
            .and_then(Value::as_str)
            .is_none_or(|value| value.is_empty())
    {
        return Err(CloudError::integrity(
            "Native session source metadata is invalid.",
        ));
    }
    if let Some(parent) = object.get("parent").filter(|value| !value.is_null()) {
        exact_keys(parent, &["object_id", "sha256"], "object parent")?;
        if parent
            .get("object_id")
            .and_then(Value::as_str)
            .is_none_or(|value| value.is_empty() || value.len() > 512)
            || !parent
                .get("sha256")
                .and_then(Value::as_str)
                .is_some_and(valid_sha256)
        {
            return Err(CloudError::integrity("Object parent reference is invalid."));
        }
    }
    let evidence = object
        .get("evidence_refs")
        .and_then(Value::as_array)
        .ok_or_else(|| CloudError::integrity("Object evidence references are invalid."))?;
    if evidence.len() > 256 {
        return Err(CloudError::integrity(
            "Object manifest has too many evidence references.",
        ));
    }
    for reference in evidence {
        strict_evidence(reference)?;
    }
    let manifest: ReplicaObjectManifestV1 = decode(value.clone(), "object manifest")?;
    if manifest.byte_length != byte_length {
        return Err(CloudError::integrity(
            "Object manifest byte length is inconsistent.",
        ));
    }
    Ok(manifest)
}

pub(crate) fn strict_evidence(value: &Value) -> Result<(), CloudError> {
    let object = value
        .as_object()
        .ok_or_else(|| CloudError::integrity("Evidence reference is not an object."))?;
    let kind = object
        .get("locator_kind")
        .and_then(Value::as_str)
        .ok_or_else(|| CloudError::integrity("Evidence locator kind is invalid."))?;
    let fields: &[&str] = match kind {
        "markdown_bytes" => &[
            "schema",
            "object_id",
            "sha256",
            "locator_kind",
            "byte_range",
        ],
        "session_event" => &[
            "schema",
            "object_id",
            "sha256",
            "locator_kind",
            "byte_range",
            "json_pointer",
            "text_range",
        ],
        _ => {
            return Err(CloudError::integrity(
                "Evidence locator kind is unsupported.",
            ))
        }
    };
    exact_keys(value, fields, "evidence reference")?;
    if object.get("schema").and_then(Value::as_str) != Some("noesora.evidence-ref/v1")
        || object
            .get("object_id")
            .and_then(Value::as_str)
            .is_none_or(|value| value.is_empty() || value.len() > 512)
        || !object
            .get("sha256")
            .and_then(Value::as_str)
            .is_some_and(valid_sha256)
    {
        return Err(CloudError::integrity(
            "Evidence reference identity is invalid.",
        ));
    }
    valid_range(object.get("byte_range"), "evidence byte range")?;
    if kind == "session_event" {
        let pointer = object
            .get("json_pointer")
            .and_then(Value::as_str)
            .ok_or_else(|| CloudError::integrity("Session evidence JSON pointer is invalid."))?;
        if pointer.len() > 1024
            || (!pointer.is_empty() && !pointer.starts_with('/'))
            || pointer
                .as_bytes()
                .windows(2)
                .any(|pair| pair[0] == b'~' && !matches!(pair[1], b'0' | b'1'))
        {
            return Err(CloudError::integrity(
                "Session evidence JSON pointer is invalid.",
            ));
        }
        valid_range(object.get("text_range"), "evidence text range")?;
    }
    Ok(())
}

fn strict_snapshot(value: &Value) -> Result<(), CloudError> {
    exact_keys(
        value,
        &[
            "schema",
            "org_id",
            "restore_epoch",
            "seq",
            "last_event_hash",
            "objects",
            "active_objects",
            "tombstones",
            "sha256",
        ],
        "sync snapshot",
    )?;
    let object = value
        .as_object()
        .ok_or_else(|| CloudError::integrity("Sync snapshot is not an object."))?;
    if object.get("schema").and_then(Value::as_str) != Some("noesora.sync-snapshot/v1")
        || object
            .get("org_id")
            .and_then(Value::as_str)
            .is_none_or(|value| value.is_empty() || value.len() > 512)
        || object
            .get("restore_epoch")
            .and_then(Value::as_str)
            .is_none_or(|value| value.is_empty() || value.len() > 512)
        || object
            .get("seq")
            .and_then(Value::as_u64)
            .is_none_or(|value| value > MAX_SAFE_INTEGER)
        || !object
            .get("last_event_hash")
            .and_then(Value::as_str)
            .is_some_and(valid_sha256)
        || !object
            .get("sha256")
            .and_then(Value::as_str)
            .is_some_and(valid_sha256)
    {
        return Err(CloudError::integrity(
            "Sync snapshot identity or digest is invalid.",
        ));
    }
    for field in ["objects", "active_objects", "tombstones"] {
        if !object.get(field).is_some_and(Value::is_array) {
            return Err(CloudError::integrity(
                "Sync snapshot inventory is not an array.",
            ));
        }
    }
    for manifest in object["objects"].as_array().expect("checked array") {
        parse_manifest(manifest)?;
    }
    for head in object["active_objects"].as_array().expect("checked array") {
        exact_keys(
            head,
            &["object_id", "sha256", "latest_event_id"],
            "snapshot head",
        )?;
        if head
            .get("object_id")
            .and_then(Value::as_str)
            .is_none_or(|value| value.is_empty() || value.len() > 512)
            || !head
                .get("sha256")
                .and_then(Value::as_str)
                .is_some_and(valid_sha256)
            || head
                .get("latest_event_id")
                .and_then(Value::as_str)
                .is_none_or(|value| value.is_empty() || value.len() > 512)
        {
            return Err(CloudError::integrity(
                "Snapshot head identity or digest is invalid.",
            ));
        }
    }
    for tombstone in object["tombstones"].as_array().expect("checked array") {
        exact_keys(
            tombstone,
            &[
                "schema",
                "object_id",
                "last_sha256",
                "prior_event_id",
                "actor_id",
                "decision_id",
                "seq",
            ],
            "snapshot tombstone",
        )?;
        let valid_id = |field: &str| {
            tombstone
                .get(field)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty() && value.len() <= 512)
        };
        if tombstone.get("schema").and_then(Value::as_str) != Some("noesora.record-tombstone/v1")
            || !valid_id("object_id")
            || !valid_id("prior_event_id")
            || !valid_id("actor_id")
            || !valid_id("decision_id")
            || !tombstone
                .get("last_sha256")
                .and_then(Value::as_str)
                .is_some_and(valid_sha256)
            || tombstone
                .get("seq")
                .and_then(Value::as_u64)
                .is_none_or(|value| value == 0 || value > MAX_SAFE_INTEGER)
        {
            return Err(CloudError::integrity(
                "Snapshot tombstone identity or digest is invalid.",
            ));
        }
    }
    Ok(())
}

fn strict_event_page(value: &Value) -> Result<(), CloudError> {
    exact_keys(
        value,
        &[
            "schema",
            "org_id",
            "restore_epoch",
            "after",
            "high_water_seq",
            "events",
            "next_cursor",
            "has_more",
        ],
        "sync event page",
    )?;
    let object = value
        .as_object()
        .ok_or_else(|| CloudError::integrity("Sync event page is not an object."))?;
    let events = object
        .get("events")
        .and_then(Value::as_array)
        .ok_or_else(|| CloudError::integrity("Sync event page event list is invalid."))?;
    if events.len() > EVENT_PAGE_LIMIT {
        return Err(CloudError::integrity(
            "Sync event page exceeds its shared event limit.",
        ));
    }
    if object.get("schema").and_then(Value::as_str) != Some("noesora.sync-page/v1")
        || object
            .get("org_id")
            .and_then(Value::as_str)
            .is_none_or(|value| value.is_empty() || value.len() > 512)
        || object
            .get("restore_epoch")
            .and_then(Value::as_str)
            .is_none_or(|value| value.is_empty() || value.len() > 512)
        || object
            .get("high_water_seq")
            .and_then(Value::as_u64)
            .is_none_or(|value| value > MAX_SAFE_INTEGER)
        || object.get("has_more").and_then(Value::as_bool).is_none()
        || object.get("next_cursor").and_then(Value::as_str).is_none()
    {
        return Err(CloudError::integrity(
            "Sync event page identity or cursor is invalid.",
        ));
    }
    for event in events {
        if serde_json::to_vec(event)
            .map_err(|_| CloudError::integrity("Cloud event could not be encoded."))?
            .len()
            > MAX_METADATA_BYTES
        {
            return Err(CloudError::integrity(
                "Cloud event exceeds the shared metadata bound.",
            ));
        }
        exact_keys(
            event,
            &[
                "schema",
                "org_id",
                "restore_epoch",
                "seq",
                "event_id",
                "commit_id",
                "proposal_id",
                "operation",
                "object_id",
                "object_schema",
                "sha256",
                "parent_sha256",
                "evidence_refs",
                "actor_id",
                "decision_id",
                "tombstone",
                "previous_event_hash",
            ],
            "sync event",
        )?;
        let object = event
            .as_object()
            .ok_or_else(|| CloudError::integrity("Sync event is not an object."))?;
        let operation = object
            .get("operation")
            .and_then(Value::as_str)
            .ok_or_else(|| CloudError::integrity("Sync event operation is invalid."))?;
        if !matches!(
            operation,
            "add" | "correct" | "combine" | "remove" | "share_session"
        ) {
            return Err(CloudError::integrity(
                "Sync event operation is unsupported.",
            ));
        }
        let evidence = object
            .get("evidence_refs")
            .and_then(Value::as_array)
            .ok_or_else(|| CloudError::integrity("Sync event evidence list is invalid."))?;
        if evidence.len() > 256 {
            return Err(CloudError::integrity(
                "Cloud event has too many evidence references.",
            ));
        }
        for reference in evidence {
            strict_evidence(reference)?;
        }
        let tombstone = object
            .get("tombstone")
            .ok_or_else(|| CloudError::integrity("Sync event omitted its tombstone field."))?;
        if operation == "remove" {
            exact_keys(
                tombstone,
                &[
                    "schema",
                    "object_id",
                    "last_sha256",
                    "prior_event_id",
                    "actor_id",
                    "decision_id",
                    "seq",
                ],
                "sync tombstone",
            )?;
        } else if !tombstone.is_null() {
            return Err(CloudError::integrity(
                "A non-removal event carried a tombstone.",
            ));
        }
        for field in ["sha256", "previous_event_hash"] {
            if !object
                .get(field)
                .and_then(Value::as_str)
                .is_some_and(valid_sha256)
            {
                return Err(CloudError::integrity("Sync event digest is invalid."));
            }
        }
    }
    Ok(())
}

fn validate_event_schema(event: &noesora_engine::ReplicaEvent) -> Result<(), CloudError> {
    if event.schema != "noesora.sync-event/v1"
        || event.seq == 0
        || event.seq > MAX_SAFE_INTEGER
        || event.event_id.is_empty()
        || event.event_id.len() > 512
        || event.commit_id.is_empty()
        || event.commit_id.len() > 512
        || event.proposal_id.is_empty()
        || event.proposal_id.len() > 512
        || event.actor_id.is_empty()
        || event.actor_id.len() > 512
        || event.decision_id.is_empty()
        || event.decision_id.len() > 512
        || event.object_id.is_empty()
        || event.object_id.len() > 512
        || event.object_schema.is_empty()
        || event.object_schema.len() > 128
        || !valid_sha256(&event.sha256)
        || !valid_sha256(&event.previous_event_hash)
        || event
            .parent_sha256
            .as_deref()
            .is_some_and(|value| !valid_sha256(value))
    {
        return Err(CloudError::integrity(
            "Cloud sync event identity or digest is invalid.",
        ));
    }
    let is_record = matches!(
        event.object_schema.as_str(),
        "noesora.record/v1" | "noesora.record/v2"
    );
    let is_session = event.object_schema == "noesora.session/v1"
        || (event.object_schema.ends_with("/v1")
            && !event.object_schema.starts_with("noesora.record/"));
    match event.operation {
        noesora_engine::ReplicaOperation::Add | noesora_engine::ReplicaOperation::Combine
            if !is_record || event.parent_sha256.is_some() =>
        {
            return Err(CloudError::integrity(
                "Cloud add/combine event has an incompatible object schema or parent.",
            ));
        }
        noesora_engine::ReplicaOperation::Correct
            if !is_record || event.parent_sha256.is_none() =>
        {
            return Err(CloudError::integrity(
                "Cloud correction event has no record parent.",
            ));
        }
        noesora_engine::ReplicaOperation::ShareSession if !is_session => {
            return Err(CloudError::integrity(
                "Cloud session-share event has a non-session schema.",
            ));
        }
        noesora_engine::ReplicaOperation::Remove if !is_record => {
            return Err(CloudError::integrity(
                "Cloud removal event has a non-record schema.",
            ));
        }
        _ => {}
    }
    if event.operation == noesora_engine::ReplicaOperation::Remove {
        let tombstone = event
            .tombstone
            .as_ref()
            .ok_or_else(|| CloudError::integrity("Cloud removal event omitted its tombstone."))?;
        if tombstone.schema != "noesora.record-tombstone/v1"
            || tombstone.object_id != event.object_id
            || tombstone.last_sha256 != event.sha256
            || tombstone.actor_id != event.actor_id
            || tombstone.decision_id != event.decision_id
            || tombstone.seq != event.seq
            || event.parent_sha256.as_deref() != Some(event.sha256.as_str())
        {
            return Err(CloudError::integrity(
                "Cloud removal tombstone does not match its event.",
            ));
        }
    } else if event.tombstone.is_some() {
        return Err(CloudError::integrity(
            "A non-removal Cloud event carried a tombstone.",
        ));
    }
    let calculated = event.event_hash().map_err(CloudError::integrity)?;
    if !valid_sha256(&calculated) {
        return Err(CloudError::integrity(
            "Cloud sync event hash could not be derived.",
        ));
    }
    Ok(())
}

pub(crate) fn exact_keys(value: &Value, expected: &[&str], label: &str) -> Result<(), CloudError> {
    let object = value
        .as_object()
        .ok_or_else(|| CloudError::integrity(format!("{label} is not an object.")))?;
    if object.len() != expected.len() || expected.iter().any(|key| !object.contains_key(*key)) {
        return Err(CloudError::integrity(format!(
            "{label} does not match the strict shared schema."
        )));
    }
    Ok(())
}

fn decode<T: serde::de::DeserializeOwned>(value: Value, label: &str) -> Result<T, CloudError> {
    serde_json::from_value(value).map_err(|_| {
        CloudError::integrity(format!("Cloud {label} did not match its shared schema."))
    })
}

fn public_error(status: u16, value: Value) -> CloudError {
    let Some(object) = value.as_object() else {
        return CloudError::new(
            "http",
            "CLOUD_HTTP_ERROR",
            "Cloud returned an invalid error response.",
        )
        .with_http(status, None);
    };
    if exact_keys(
        &value,
        &[
            "schema",
            "code",
            "message",
            "request_id",
            "retryable",
            "details",
        ],
        "Cloud error",
    )
    .is_err()
        || object.get("schema").and_then(Value::as_str) != Some("noesora.error/v1")
        || !object.get("retryable").is_some_and(Value::is_boolean)
    {
        return CloudError::new(
            "http",
            "CLOUD_HTTP_ERROR",
            "Cloud returned an invalid typed error response.",
        )
        .with_http(status, None);
    }
    let Some(code) = object
        .get("code")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return CloudError::new(
            "http",
            "CLOUD_HTTP_ERROR",
            "Cloud returned an invalid typed error response.",
        )
        .with_http(status, None);
    };
    let Some(message) = object
        .get("message")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return CloudError::new(
            "http",
            "CLOUD_HTTP_ERROR",
            "Cloud returned an invalid typed error response.",
        )
        .with_http(status, None);
    };
    let request_id = object
        .get("request_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let Some(details) = object.get("details") else {
        return CloudError::new(
            "http",
            "CLOUD_HTTP_ERROR",
            "Cloud returned an invalid typed error response.",
        )
        .with_http(status, request_id);
    };
    if exact_keys(
        details,
        &[
            "current_cursor",
            "current_hash",
            "proposal_revision",
            "retry_after",
        ],
        "Cloud error details",
    )
    .is_err()
    {
        return CloudError::new(
            "http",
            "CLOUD_HTTP_ERROR",
            "Cloud returned invalid typed error details.",
        )
        .with_http(status, request_id);
    }
    let category = match code {
        "AUTHENTICATION_REQUIRED" | "ORG_MEMBERSHIP_REQUIRED" | "STEWARD_REQUIRED" => {
            "authorization"
        }
        "STALE_BASE"
        | "PROPOSAL_REVISION_MISMATCH"
        | "IDEMPOTENCY_KEY_REUSE"
        | "OBJECT_ID_CONFLICT"
        | "CURSOR_EXPIRED"
        | "RESTORE_EPOCH_CHANGED" => "conflict",
        _ => "http",
    };
    let mut error = CloudError::new(category, code, message).with_http(status, request_id);
    error.details = Some(details.clone());
    error
}

pub(crate) fn valid_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    })
}

pub(crate) fn validate_cursor(value: &str) -> Result<(), CloudError> {
    if value
        .strip_prefix("noesora.sync-cursor/v1.")
        .is_some_and(|rest| {
            !rest.is_empty()
                && rest.len() <= 2048
                && rest
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        })
    {
        Ok(())
    } else {
        Err(CloudError::integrity(
            "Cloud returned a malformed sync cursor.",
        ))
    }
}

fn valid_range(value: Option<&Value>, label: &str) -> Result<(), CloudError> {
    let range = value
        .and_then(Value::as_array)
        .filter(|range| range.len() == 2)
        .ok_or_else(|| CloudError::integrity(format!("{label} is invalid.")))?;
    let start = range[0]
        .as_u64()
        .ok_or_else(|| CloudError::integrity(format!("{label} is invalid.")))?;
    let end = range[1]
        .as_u64()
        .ok_or_else(|| CloudError::integrity(format!("{label} is invalid.")))?;
    if end <= start || end > 9_007_199_254_740_991 {
        return Err(CloudError::integrity(format!("{label} is invalid.")));
    }
    Ok(())
}

fn safe_locator(value: &str) -> bool {
    value.len() <= 1024
        && !value.is_empty()
        && !value.starts_with('/')
        && !value.starts_with('~')
        && !value.ends_with('/')
        && !value.contains('\\')
        && !value.contains(':')
        && value
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

fn path_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

fn query_value(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}
