use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use keyring::{Entry, Error as KeyringError};
use noesora_engine::Store;
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::api::{endpoint_is_secure, parse_origin, AuthMetadata, CloudApi};
use super::local;
use super::{CloudError, CloudOutput};

const KEYRING_SERVICE: &str = "noesora-cli-cloud";
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
const REFRESH_GRANT: &str = "refresh_token";
const AUTHORIZATION_PENDING: &str = "authorization_pending";
const SLOW_DOWN: &str = "slow_down";
const ACCESS_DENIED: &str = "access_denied";
const EXPIRED_TOKEN: &str = "expired_token";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CloudAccountV1 {
    pub(crate) schema: String,
    pub(crate) api_origin: String,
    pub(crate) issuer: String,
    pub(crate) client_id: String,
    pub(crate) required_scopes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredTokensV1 {
    schema: String,
    api_origin: String,
    issuer: String,
    client_id: String,
    access_token: String,
    refresh_token: Option<String>,
    access_expires_at: u64,
    refresh_expires_at: Option<u64>,
    token_type: String,
}

#[derive(Debug, Clone)]
struct OAuthMetadata {
    device_authorization_endpoint: String,
    token_endpoint: String,
    revocation_endpoint: Option<String>,
}

#[derive(Clone)]
struct DeviceAuthorization {
    // Process memory only. It is never serialized or included in diagnostics.
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: u64,
}

struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: u64,
    refresh_expires_in: Option<u64>,
    token_type: String,
}

pub(crate) trait CredentialStore {
    fn load(&self, api_origin: &str) -> Result<Option<StoredTokensV1>, CloudError>;
    fn save(&self, api_origin: &str, tokens: &StoredTokensV1) -> Result<(), CloudError>;
    fn remove(&self, api_origin: &str) -> Result<(), CloudError>;
}

struct OsCredentialStore;

impl CredentialStore for OsCredentialStore {
    fn load(&self, api_origin: &str) -> Result<Option<StoredTokensV1>, CloudError> {
        let entry = keyring_entry(api_origin)?;
        let secret = match entry.get_password() {
            Ok(secret) => secret,
            Err(KeyringError::NoEntry) => return Ok(None),
            Err(_) => return Err(secure_storage_error()),
        };
        let tokens: StoredTokensV1 = serde_json::from_str(&secret).map_err(|_| {
            CloudError::new(
                "credential",
                "CLOUD_CREDENTIAL_INVALID",
                "The OS secure store contains an invalid Cloud credential.",
            )
        })?;
        if tokens.schema != "noesora.cloud-credential/v1"
            || tokens.api_origin != api_origin
            || tokens.access_token.is_empty()
            || tokens.issuer.is_empty()
            || tokens.client_id.is_empty()
            || tokens.token_type != "Bearer"
        {
            return Err(CloudError::new(
                "credential",
                "CLOUD_CREDENTIAL_INVALID",
                "The OS secure store contains a credential for a different Cloud origin.",
            ));
        }
        Ok(Some(tokens))
    }

    fn save(&self, api_origin: &str, tokens: &StoredTokensV1) -> Result<(), CloudError> {
        let value = serde_json::to_string(tokens).map_err(|_| {
            CloudError::new(
                "credential",
                "CLOUD_SECURE_STORAGE_UNAVAILABLE",
                "Could not encode the Cloud credential for OS secure storage.",
            )
        })?;
        keyring_entry(api_origin)?
            .set_password(&value)
            .map_err(|_| secure_storage_error())
    }

    fn remove(&self, api_origin: &str) -> Result<(), CloudError> {
        match keyring_entry(api_origin)?.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
            Err(_) => Err(secure_storage_error()),
        }
    }
}

fn keyring_entry(api_origin: &str) -> Result<Entry, CloudError> {
    let username = local::hash_name(api_origin.as_bytes());
    Entry::new(KEYRING_SERVICE, &username).map_err(|_| secure_storage_error())
}

fn secure_storage_error() -> CloudError {
    CloudError::new("credential", "CLOUD_SECURE_STORAGE_UNAVAILABLE", "The operating system secure credential service is unavailable; no credential was written to a file or environment variable.")
}
fn validate_token_binding(
    account: &CloudAccountV1,
    tokens: &StoredTokensV1,
) -> Result<(), CloudError> {
    if tokens.api_origin != account.api_origin
        || tokens.issuer != account.issuer
        || tokens.client_id != account.client_id
    {
        return Err(CloudError::new("credential", "CLOUD_CREDENTIAL_MISMATCH", "The OS-stored Cloud credential does not match the selected API origin, issuer, and public client."));
    }
    Ok(())
}

pub(crate) fn login(store: &Store, origin: &str) -> Result<CloudOutput, CloudError> {
    login_with(
        store,
        origin,
        &OsCredentialStore,
        |duration| std::thread::sleep(duration),
        Instant::now,
    )
}

fn login_with<C, S, N>(
    store: &Store,
    origin: &str,
    credentials: &C,
    mut sleep: S,
    mut now: N,
) -> Result<CloudOutput, CloudError>
where
    C: CredentialStore,
    S: FnMut(Duration),
    N: FnMut() -> Instant,
{
    let api = CloudApi::new(origin)?;
    let api_origin = api.origin().to_owned();
    let existing_account = load_account(store)?;
    if existing_account
        .as_ref()
        .is_some_and(|account| account.api_origin != api_origin)
    {
        return Err(CloudError::new(
            "credential",
            "CLOUD_ORIGIN_MISMATCH",
            "Log out of the selected Cloud origin before switching the stored account.",
        ));
    }
    if let Some(tokens) = credentials.load(&api_origin)? {
        let Some(account) = existing_account.as_ref() else {
            return Err(CloudError::new(
                "credential",
                "CLOUD_CREDENTIAL_MISMATCH",
                "A secure Cloud credential exists without matching non-secret account metadata.",
            ));
        };
        validate_token_binding(account, &tokens)?;
        return Err(CloudError::new("credential", "CLOUD_ALREADY_LOGGED_IN", "A Cloud credential is already stored for this origin. Log out before starting another device authorization flow."));
    }
    let metadata = api.get_auth_metadata()?;
    let account = CloudAccountV1 {
        schema: "noesora.cloud-account/v1".to_owned(),
        api_origin: api_origin.clone(),
        issuer: metadata.issuer.clone(),
        client_id: metadata.client_id.clone(),
        required_scopes: metadata.required_scopes.clone(),
    };
    let oauth = discover_oauth(&metadata.issuer)?;
    let device = request_device_authorization(&oauth, &metadata)?;
    let safe_uri = validate_verification_uri(&device.verification_uri)?;
    eprintln!("Noesora Cloud sign-in\nOpen this verification URI on your browser: {}\nEnter this code: {}\nThe device code is kept only in process memory.", super::clean_terminal(&safe_uri), super::clean_terminal(&device.user_code));
    let token = poll_device_tokens(&oauth, &metadata.client_id, &device, &mut sleep, &mut now)?;
    let stored = stored_tokens(&api_origin, &metadata, token)?;
    save_account(store, &account)?;
    if let Err(error) = credentials.save(&api_origin, &stored) {
        let _ = local::remove_file(&local::account_path(store)?);
        return Err(error);
    }
    let identity = api.get_identity(&stored.access_token)?;
    Ok(CloudOutput::success(
        "cloud login",
        json!({"authenticated": true, "user_id": identity.user_id, "org_id": identity.org_id, "api_origin": api_origin}),
        format!(
            "Signed in to Noesora Cloud as {}.",
            super::clean_terminal(&identity.user_id)
        ),
    ))
}

pub(crate) fn logout(store: &Store) -> Result<CloudOutput, CloudError> {
    logout_with(store, &OsCredentialStore)
}

fn logout_with<C: CredentialStore>(
    store: &Store,
    credentials: &C,
) -> Result<CloudOutput, CloudError> {
    let Some(account) = load_account(store)? else {
        return Ok(CloudOutput::success(
            "cloud logout",
            json!({"authenticated": false, "remote_revocation": "not_attempted"}),
            "No Cloud account is selected.",
        ));
    };
    let tokens = credentials.load(&account.api_origin)?;
    let mut remote_revocation = "not_available";
    if let Some(tokens) = &tokens {
        if validate_token_binding(&account, tokens).is_err() {
            remote_revocation = "unknown";
        } else {
            match discover_oauth(&account.issuer) {
                Ok(oauth) if oauth.revocation_endpoint.is_some() => {
                    let endpoint = oauth.revocation_endpoint.as_deref().unwrap_or_default();
                    let mut revoked = true;
                    if let Some(refresh) = tokens.refresh_token.as_deref() {
                        revoked &=
                            revoke_token(endpoint, &account.client_id, refresh, "refresh_token")
                                .is_ok();
                    }
                    revoked &= revoke_token(
                        endpoint,
                        &account.client_id,
                        &tokens.access_token,
                        "access_token",
                    )
                    .is_ok();
                    remote_revocation = if revoked { "requested" } else { "unknown" };
                }
                Ok(_) => remote_revocation = "unsupported",
                Err(_) => remote_revocation = "unknown",
            }
        }
    }
    credentials.remove(&account.api_origin)?;
    local::remove_file(&local::account_path(store)?)?;
    let text = match remote_revocation {
        "requested" => "Cloud credential removed from OS secure storage; provider revocation was requested. Local synced data was retained.",
        "unsupported" => "Cloud credential removed from OS secure storage; provider revocation is unsupported. Local synced data was retained.",
        "unknown" => "Cloud credential removed from OS secure storage; remote revocation could not be confirmed. Local synced data was retained.",
        _ => "Cloud credential removed from OS secure storage. Local synced data was retained.",
    };
    Ok(CloudOutput::success(
        "cloud logout",
        json!({"authenticated": false, "remote_revocation": remote_revocation, "local_synced_data_retained": true}),
        text,
    ))
}

pub(crate) fn status(store: &Store) -> Result<CloudOutput, CloudError> {
    status_with(store, &OsCredentialStore)
}

fn status_with<C: CredentialStore>(
    store: &Store,
    credentials: &C,
) -> Result<CloudOutput, CloudError> {
    let Some(account) = load_account(store)? else {
        return Ok(CloudOutput::success(
            "cloud status",
            json!({"authenticated": false, "api_origin": null, "identity": null, "organizations": []}),
            "Not signed in to Noesora Cloud.",
        ));
    };
    let Some(tokens) = credentials.load(&account.api_origin)? else {
        return Ok(CloudOutput::success(
            "cloud status",
            json!({"authenticated": false, "api_origin": account.api_origin, "identity": null, "organizations": []}),
            "No Cloud credential is present in OS secure storage.",
        ));
    };
    let tokens = refresh_if_needed(credentials, &account, tokens)?;
    let api = CloudApi::new(&account.api_origin)?;
    let identity = api.get_identity(&tokens.access_token)?;
    let organizations = api.get_organizations(&tokens.access_token)?;
    let text = if organizations.is_empty() {
        format!(
            "Signed in as {}. No organization memberships were returned.",
            super::clean_terminal(&identity.user_id)
        )
    } else {
        let rows = organizations
            .iter()
            .map(|org| {
                format!(
                    "  {} — {} ({}){}",
                    super::clean_terminal(&org.name),
                    super::clean_terminal(&org.id),
                    org.status,
                    if org.steward { ", steward" } else { "" }
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "Signed in as {}.\nOrganizations:\n{}",
            super::clean_terminal(&identity.user_id),
            rows
        )
    };
    Ok(CloudOutput::success(
        "cloud status",
        json!({"authenticated": true, "api_origin": account.api_origin, "identity": identity, "organizations": organizations}),
        text,
    ))
}

pub(crate) fn authenticated_session(store: &Store) -> Result<(CloudAccountV1, String), CloudError> {
    let account = load_account(store)?.ok_or_else(|| {
        CloudError::new(
            "credential",
            "CLOUD_LOGIN_REQUIRED",
            "Run `noesora cloud login --url <https-origin>` before using Cloud.",
        )
    })?;
    let tokens = OsCredentialStore
        .load(&account.api_origin)?
        .ok_or_else(|| {
            CloudError::new(
                "credential",
                "CLOUD_LOGIN_REQUIRED",
                "No Cloud credential is present in OS secure storage; log in again.",
            )
        })?;
    let tokens = refresh_if_needed(&OsCredentialStore, &account, tokens)?;
    Ok((account, tokens.access_token))
}

fn load_account(store: &Store) -> Result<Option<CloudAccountV1>, CloudError> {
    let path = local::account_path(store)?;
    let account = local::read_json::<CloudAccountV1>(&path)?;
    if let Some(account) = &account {
        if account.schema != "noesora.cloud-account/v1"
            || parse_origin(&account.api_origin).is_err()
            || account.issuer.is_empty()
            || account.client_id.is_empty()
            || account.required_scopes.len() != 1
            || account
                .required_scopes
                .first()
                .is_none_or(|scope| scope.as_str() != "user:org:read")
        {
            return Err(CloudError::integrity(
                "Cloud account sidecar does not match the supported schema.",
            ));
        }
    }
    Ok(account)
}

fn save_account(store: &Store, account: &CloudAccountV1) -> Result<(), CloudError> {
    local::write_json(&local::account_path(store)?, account)
}

fn refresh_if_needed<C: CredentialStore>(
    credentials: &C,
    account: &CloudAccountV1,
    tokens: StoredTokensV1,
) -> Result<StoredTokensV1, CloudError> {
    validate_token_binding(account, &tokens)?;
    if tokens.access_expires_at > unix_now().saturating_add(15) {
        return Ok(tokens);
    }
    let refresh = tokens
        .refresh_token
        .as_deref()
        .filter(|_| {
            tokens
                .refresh_expires_at
                .is_none_or(|expires| expires > unix_now())
        })
        .ok_or_else(|| {
            CloudError::new(
                "credential",
                "CLOUD_TOKEN_EXPIRED",
                "Cloud access expired; run the explicit device login again.",
            )
        })?;
    let oauth = discover_oauth(&account.issuer)?;
    let refreshed = request_refresh_token(&oauth, &account.client_id, refresh)?;
    let old_refresh = tokens.refresh_token.clone();
    let old_refresh_expiry = tokens.refresh_expires_at;
    let mut updated = stored_tokens(
        &account.api_origin,
        &AuthMetadata {
            schema: "noesora.auth-metadata/v1".to_owned(),
            issuer: account.issuer.clone(),
            client_id: account.client_id.clone(),
            api_origin: account.api_origin.clone(),
            required_scopes: account.required_scopes.clone(),
        },
        refreshed,
    )?;
    if updated.refresh_token.is_none() {
        updated.refresh_token = old_refresh;
    }
    if updated.refresh_expires_at.is_none() {
        updated.refresh_expires_at = old_refresh_expiry;
    }
    credentials.save(&account.api_origin, &updated)?;
    Ok(updated)
}

fn stored_tokens(
    api_origin: &str,
    metadata: &AuthMetadata,
    response: TokenResponse,
) -> Result<StoredTokensV1, CloudError> {
    if response.token_type.to_ascii_lowercase() != "bearer"
        || response.access_token.is_empty()
        || response.expires_in == 0
    {
        return Err(CloudError::integrity(
            "OAuth token response did not contain a usable bearer token.",
        ));
    }
    let now = unix_now();
    Ok(StoredTokensV1 {
        schema: "noesora.cloud-credential/v1".to_owned(),
        api_origin: api_origin.to_owned(),
        issuer: metadata.issuer.clone(),
        client_id: metadata.client_id.clone(),
        access_token: response.access_token,
        refresh_token: response.refresh_token,
        access_expires_at: now.saturating_add(response.expires_in),
        refresh_expires_at: response
            .refresh_expires_in
            .map(|seconds| now.saturating_add(seconds)),
        token_type: "Bearer".to_owned(),
    })
}

fn discover_oauth(issuer: &str) -> Result<OAuthMetadata, CloudError> {
    let issuer_url = parse_issuer(issuer)?;
    let discovery = discovery_url(&issuer_url)?;
    let response = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| {
            CloudError::new(
                "transport",
                "CLOUD_CLIENT_FAILED",
                "Could not initialize the OAuth HTTPS client.",
            )
        })?
        .get(discovery)
        .header(ACCEPT, "application/json")
        .send()
        .map_err(|_| CloudError::offline("OAuth authorization metadata could not be reached."))?;
    if !response.status().is_success() {
        return Err(CloudError::new(
            "oauth",
            "OAUTH_METADATA_UNAVAILABLE",
            "OAuth authorization metadata request failed.",
        ));
    }
    if !response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase()
        .contains("json")
    {
        return Err(CloudError::integrity(
            "OAuth authorization metadata was not JSON.",
        ));
    }
    let value: Value = response
        .json()
        .map_err(|_| CloudError::integrity("OAuth authorization metadata was malformed."))?;
    let object = value
        .as_object()
        .ok_or_else(|| CloudError::integrity("OAuth authorization metadata is not an object."))?;
    let issuer_value = string_field(object, "issuer")?;
    let token_endpoint = string_field(object, "token_endpoint")?;
    let device_endpoint = string_field(object, "device_authorization_endpoint")?;
    let grants = object
        .get("grant_types_supported")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            CloudError::new(
                "oauth",
                "DEVICE_GRANT_UNSUPPORTED",
                "OAuth metadata does not advertise supported grant types.",
            )
        })?;
    if issuer_value != issuer
        || !grants
            .iter()
            .any(|grant| grant.as_str() == Some(DEVICE_GRANT))
    {
        return Err(CloudError::new(
            "oauth",
            "DEVICE_GRANT_UNSUPPORTED",
            "OAuth issuer or public-client Device Authorization Grant metadata did not match.",
        ));
    }
    if !endpoint_is_secure(&token_endpoint) || !endpoint_is_secure(&device_endpoint) {
        return Err(CloudError::integrity(
            "OAuth metadata contained a non-TLS or non-loopback endpoint.",
        ));
    }
    let revocation_endpoint = object
        .get("revocation_endpoint")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if revocation_endpoint
        .as_deref()
        .is_some_and(|endpoint| !endpoint_is_secure(endpoint))
    {
        return Err(CloudError::integrity(
            "OAuth metadata contained an insecure revocation endpoint.",
        ));
    }
    Ok(OAuthMetadata {
        device_authorization_endpoint: device_endpoint,
        token_endpoint,
        revocation_endpoint,
    })
}

fn request_device_authorization(
    oauth: &OAuthMetadata,
    metadata: &AuthMetadata,
) -> Result<DeviceAuthorization, CloudError> {
    let scope = metadata.required_scopes.join(" ");
    let response = oauth_client()?
        .post(&oauth.device_authorization_endpoint)
        .form(&[
            ("client_id", metadata.client_id.as_str()),
            ("scope", scope.as_str()),
        ])
        .send()
        .map_err(|_| {
            CloudError::offline("OAuth device authorization endpoint could not be reached.")
        })?;
    if !response.status().is_success() {
        return Err(oauth_response_error(response));
    }
    let value: Value = response
        .json()
        .map_err(|_| CloudError::integrity("OAuth device authorization response was malformed."))?;
    let object = value.as_object().ok_or_else(|| {
        CloudError::integrity("OAuth device authorization response is not an object.")
    })?;
    let device_code = string_field(object, "device_code")?;
    let user_code = string_field(object, "user_code")?;
    let verification_uri = string_field(object, "verification_uri")?;
    let expires_in = object
        .get("expires_in")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .ok_or_else(|| CloudError::integrity("OAuth device authorization expiry is invalid."))?;
    let interval = match object.get("interval") {
        None => 5,
        Some(value) => value
            .as_u64()
            .ok_or_else(|| CloudError::integrity("OAuth polling interval is invalid."))?,
    };
    if device_code.is_empty() || user_code.is_empty() {
        return Err(CloudError::integrity(
            "OAuth device authorization response is invalid.",
        ));
    }
    Ok(DeviceAuthorization {
        device_code,
        user_code,
        verification_uri,
        expires_in,
        interval,
    })
}

fn poll_device_tokens<S, N>(
    oauth: &OAuthMetadata,
    client_id: &str,
    device: &DeviceAuthorization,
    sleep: &mut S,
    now: &mut N,
) -> Result<TokenResponse, CloudError>
where
    S: FnMut(Duration),
    N: FnMut() -> Instant,
{
    let started = now();
    let deadline = started
        .checked_add(Duration::from_secs(device.expires_in))
        .ok_or_else(|| {
            CloudError::integrity("OAuth device authorization expiry is out of range.")
        })?;
    let mut interval = Duration::from_secs(device.interval);
    loop {
        let current = now();
        if current >= deadline {
            return Err(CloudError::new(
                "oauth",
                "OAUTH_DEVICE_EXPIRED",
                "The device authorization code expired before authorization completed.",
            ));
        }
        sleep(interval.min(deadline.saturating_duration_since(current)));
        if now() >= deadline {
            return Err(CloudError::new(
                "oauth",
                "OAUTH_DEVICE_EXPIRED",
                "The device authorization code expired before authorization completed.",
            ));
        }
        let response = oauth_client()?
            .post(&oauth.token_endpoint)
            .form(&[
                ("grant_type", DEVICE_GRANT),
                ("device_code", device.device_code.as_str()),
                ("client_id", client_id),
            ])
            .send()
            .map_err(|_| {
                CloudError::offline(
                    "OAuth token endpoint could not be reached; no token success was assumed.",
                )
            })?;
        match token_endpoint_response(response)? {
            Ok(token) => return Ok(token),
            Err(code) if code == AUTHORIZATION_PENDING => continue,
            Err(code) if code == SLOW_DOWN => {
                interval = interval.saturating_add(Duration::from_secs(5));
            }
            Err(code) if code == ACCESS_DENIED => {
                return Err(CloudError::new(
                    "oauth",
                    "OAUTH_ACCESS_DENIED",
                    "Cloud device authorization was denied.",
                ))
            }
            Err(code) if code == EXPIRED_TOKEN => {
                return Err(CloudError::new(
                    "oauth",
                    "OAUTH_DEVICE_EXPIRED",
                    "The device authorization code expired.",
                ))
            }
            Err(_) => {
                return Err(CloudError::new(
                    "oauth",
                    "OAUTH_TOKEN_FAILED",
                    "OAuth device authorization failed.",
                ))
            }
        }
    }
}

fn request_refresh_token(
    oauth: &OAuthMetadata,
    client_id: &str,
    refresh_token: &str,
) -> Result<TokenResponse, CloudError> {
    let response = oauth_client()?
        .post(&oauth.token_endpoint)
        .form(&[
            ("grant_type", REFRESH_GRANT),
            ("refresh_token", refresh_token),
            ("client_id", client_id),
        ])
        .send()
        .map_err(|_| CloudError::offline("OAuth refresh endpoint could not be reached."))?;
    match token_endpoint_response(response)? {
        Ok(token) => Ok(token),
        Err(_) => Err(CloudError::new(
            "credential",
            "CLOUD_TOKEN_EXPIRED",
            "Cloud refresh was rejected; run the explicit device login again.",
        )),
    }
}

fn token_endpoint_response(
    response: reqwest::blocking::Response,
) -> Result<Result<TokenResponse, String>, CloudError> {
    let success = response.status().is_success();
    let value: Value = response
        .json()
        .map_err(|_| CloudError::integrity("OAuth token response was malformed."))?;
    let object = value
        .as_object()
        .ok_or_else(|| CloudError::integrity("OAuth token response is not an object."))?;
    if !success || object.contains_key("error") {
        return Ok(Err(object
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("oauth_error")
            .to_owned()));
    }
    let access_token = string_field(object, "access_token")?;
    let token_type = string_field(object, "token_type")?;
    let expires_in = object
        .get("expires_in")
        .and_then(Value::as_u64)
        .ok_or_else(|| CloudError::integrity("OAuth access token expiry is invalid."))?;
    let refresh_token = object
        .get("refresh_token")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let refresh_expires_in = object.get("refresh_expires_in").and_then(Value::as_u64);
    Ok(Ok(TokenResponse {
        access_token,
        refresh_token,
        expires_in,
        refresh_expires_in,
        token_type,
    }))
}

fn revoke_token(
    endpoint: &str,
    client_id: &str,
    token: &str,
    hint: &str,
) -> Result<(), CloudError> {
    let response = oauth_client()?
        .post(endpoint)
        .form(&[
            ("token", token),
            ("client_id", client_id),
            ("token_type_hint", hint),
        ])
        .send()
        .map_err(|_| CloudError::offline("OAuth revocation endpoint could not be reached."))?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(CloudError::new(
            "oauth",
            "OAUTH_REVOCATION_FAILED",
            "OAuth provider did not confirm token revocation.",
        ))
    }
}

fn oauth_client() -> Result<Client, CloudError> {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| {
            CloudError::new(
                "transport",
                "CLOUD_CLIENT_FAILED",
                "Could not initialize the OAuth HTTPS client.",
            )
        })
}

fn oauth_response_error(response: reqwest::blocking::Response) -> CloudError {
    let status = response.status().as_u16();
    let value: Result<Value, _> = response.json();
    let code = value
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "OAUTH_DEVICE_AUTHORIZATION_FAILED".to_owned());
    CloudError::new("oauth", &code, "OAuth device authorization request failed.")
        .with_http(status, None)
}

fn validate_verification_uri(uri: &str) -> Result<String, CloudError> {
    if !endpoint_is_secure(uri) {
        return Err(CloudError::integrity(
            "OAuth verification URI is not a TLS provider URI.",
        ));
    }
    let url = Url::parse(uri)
        .map_err(|_| CloudError::integrity("OAuth verification URI is malformed."))?;
    Ok(url.to_string())
}

fn parse_issuer(value: &str) -> Result<Url, CloudError> {
    let url = Url::parse(value).map_err(|_| CloudError::integrity("OAuth issuer is malformed."))?;
    let loopback = url.host_str().is_some_and(|host| {
        host == "localhost" || host == "127.0.0.1" || host == "::1" || host == "[::1]"
    });
    if (url.scheme() != "https" && !(url.scheme() == "http" && loopback))
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(CloudError::integrity(
            "OAuth issuer must be a secure absolute URI.",
        ));
    }
    Ok(url)
}

fn discovery_url(issuer: &Url) -> Result<Url, CloudError> {
    let mut url = issuer.clone();
    let path = issuer.path().trim_matches('/');
    let well_known = if path.is_empty() {
        "/.well-known/oauth-authorization-server".to_owned()
    } else {
        format!("/.well-known/oauth-authorization-server/{path}")
    };
    url.set_path(&well_known);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

fn string_field(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<String, CloudError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| CloudError::integrity(format!("OAuth metadata omitted a valid {field}.")))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct MemoryCredentialStore(
    std::sync::Mutex<std::collections::HashMap<String, StoredTokensV1>>,
);

#[cfg(test)]
impl CredentialStore for MemoryCredentialStore {
    fn load(&self, api_origin: &str) -> Result<Option<StoredTokensV1>, CloudError> {
        Ok(self
            .0
            .lock()
            .expect("memory credential lock")
            .get(api_origin)
            .cloned())
    }
    fn save(&self, api_origin: &str, tokens: &StoredTokensV1) -> Result<(), CloudError> {
        self.0
            .lock()
            .expect("memory credential lock")
            .insert(api_origin.to_owned(), tokens.clone());
        Ok(())
    }
    fn remove(&self, api_origin: &str) -> Result<(), CloudError> {
        self.0
            .lock()
            .expect("memory credential lock")
            .remove(api_origin);
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    pub(crate) use super::MemoryCredentialStore;
    use super::{
        login_with, save_account, status_with, CloudAccountV1, CloudError, CloudOutput,
        CredentialStore, Duration, Instant, Store, StoredTokensV1,
    };

    pub(crate) fn login_with_fake_clock<C, S, N>(
        store: &Store,
        origin: &str,
        credentials: &C,
        sleep: S,
        now: N,
    ) -> Result<CloudOutput, CloudError>
    where
        C: CredentialStore,
        S: FnMut(Duration),
        N: FnMut() -> Instant,
    {
        login_with(store, origin, credentials, sleep, now)
    }

    pub(crate) fn write_account_for_test(
        store: &Store,
        api_origin: &str,
        issuer: &str,
        client_id: &str,
    ) -> Result<(), CloudError> {
        save_account(
            store,
            &CloudAccountV1 {
                schema: "noesora.cloud-account/v1".to_owned(),
                api_origin: api_origin.to_owned(),
                issuer: issuer.to_owned(),
                client_id: client_id.to_owned(),
                required_scopes: vec!["user:org:read".to_owned()],
            },
        )
    }

    pub(crate) fn seed_memory_credential(
        credentials: &MemoryCredentialStore,
        api_origin: &str,
        issuer: &str,
        client_id: &str,
        access_expires_at: u64,
    ) -> Result<(), CloudError> {
        credentials.save(
            api_origin,
            &StoredTokensV1 {
                schema: "noesora.cloud-credential/v1".to_owned(),
                api_origin: api_origin.to_owned(),
                issuer: issuer.to_owned(),
                client_id: client_id.to_owned(),
                access_token: "fixture-access-token".to_owned(),
                refresh_token: Some("fixture-refresh-token".to_owned()),
                access_expires_at,
                refresh_expires_at: Some(u64::MAX),
                token_type: "Bearer".to_owned(),
            },
        )
    }

    pub(crate) fn status_with_credentials<C: CredentialStore>(
        store: &Store,
        credentials: &C,
    ) -> Result<CloudOutput, CloudError> {
        status_with(store, credentials)
    }

    pub(crate) fn memory_token_present(credentials: &MemoryCredentialStore, origin: &str) -> bool {
        credentials
            .0
            .lock()
            .expect("memory credential lock")
            .contains_key(origin)
    }
}
