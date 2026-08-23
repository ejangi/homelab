use std::{env, io::Cursor, sync::Arc, time::Duration};

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use image::{codecs::jpeg::JpegEncoder, imageops::FilterType, GenericImageView, ImageReader};
use matrix_sdk::{
    config::SyncSettings,
    room::Joined,
    ruma::{
        api::client::message::send_message_event,
        events::room::{message::{ImageMessageEventContent, MessageType, RoomMessageEventContent}, ImageInfo},
        serde::Raw,
        OwnedRoomId, OwnedTransactionId, UInt,
    },
    Client,
};
use pulldown_cmark::{html, Options, Parser};
use rocket::{
    fairing::{Fairing, Info, Kind},
    get,
    http::{ContentType, Status},
    post,
    request::{FromRequest, Outcome, Request},
    response::{self, content::RawHtml, Responder, Response},
    serde::{json::Json, Deserialize, Serialize},
    State,
};
use sha2::{Digest, Sha256};
use sqlx::{postgres::PgPoolOptions, PgPool};
use subtle::ConstantTimeEq;
use tokio::sync::{Mutex, RwLock};
use tracing::{error, info, warn};
use uuid::Uuid;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

#[derive(Clone)]
struct Config {
    homeserver_url: String,
    user_id: String,
    password: String,
    default_room_id: String,
    service_api_key: String,
    store_encryption_key: String,
    database_url: String,
    idempotency_retention_days: i64,
}

impl Config {
    fn from_env() -> Result<Self> {
        Ok(Self {
            homeserver_url: required_env("MATRIX_HOMESERVER_URL")?,
            user_id: required_env("MATRIX_USER_ID")?,
            password: env::var("MATRIX_PASSWORD").unwrap_or_default(),
            default_room_id: required_env("MATRIX_DEFAULT_ROOM_ID")?,
            service_api_key: required_env("MATRIX_SERVICE_API_KEY")?,
            store_encryption_key: required_env("MATRIX_STORE_ENCRYPTION_KEY")?,
            database_url: required_env("MATRIX_DATABASE_URL")?,
            idempotency_retention_days: env::var("MATRIX_IDEMPOTENCY_RETENTION_DAYS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(30),
        })
    }
}

fn required_env(name: &str) -> Result<String> {
    env::var(name).with_context(|| format!("{name} must be configured"))
}

fn database_url_with_search_path(database_url: &str, schema: &str) -> String {
    let separator = if database_url.contains('?') { '&' } else { '?' };
    format!("{database_url}{separator}options%5Bsearch_path%5D={schema}")
}

async fn install_sdk_store_compatibility(pool: &PgPool) -> Result<()> {
    // matrix-sdk-sql 0.1.0-beta.2 saves an outbound Megolm session with a
    // plain INSERT even though a room has a single, replaceable session. Its
    // own writes therefore fail on the second save. This trigger implements
    // the intended update-or-insert behavior without modifying crypto data.
    sqlx::query(
        r#"
        CREATE OR REPLACE FUNCTION matrix_sdk.replace_outbound_group_session()
        RETURNS TRIGGER AS $$
        BEGIN
            UPDATE matrix_sdk.cryptostore_outbound_group_session
            SET session_data = NEW.session_data
            WHERE room_id = NEW.room_id;

            IF FOUND THEN
                RETURN NULL;
            END IF;

            RETURN NEW;
        END;
        $$ LANGUAGE plpgsql
        "#,
    )
    .execute(pool)
    .await
    .context("creating Matrix SDK outbound-session compatibility function")?;

    sqlx::query(
        "DROP TRIGGER IF EXISTS replace_outbound_group_session ON matrix_sdk.cryptostore_outbound_group_session",
    )
    .execute(pool)
    .await
    .context("replacing Matrix SDK outbound-session compatibility trigger")?;

    sqlx::query(
        r#"
        CREATE TRIGGER replace_outbound_group_session
        BEFORE INSERT ON matrix_sdk.cryptostore_outbound_group_session
        FOR EACH ROW EXECUTE FUNCTION matrix_sdk.replace_outbound_group_session()
        "#,
    )
    .execute(pool)
    .await
    .context("creating Matrix SDK outbound-session compatibility trigger")?;

    Ok(())
}

struct AppState {
    config: Config,
    pool: PgPool,
    sdk_pool: Arc<PgPool>,
    client: RwLock<Option<Client>>,
    lifecycle_lock: Mutex<()>,
    send_lock: Mutex<()>,
}

impl AppState {
    async fn bootstrap(&self) -> Result<SetupStatus> {
        let _guard = self.lifecycle_lock.lock().await;

        if let Some(client) = self.client.read().await.clone() {
            return Ok(self.setup_status(Some(client)).await);
        }

        if self.config.password.is_empty() {
            return Err(anyhow!("MATRIX_PASSWORD is required before Matrix setup can run"));
        }

        let store_config = matrix_sdk_sql::store_config(
            &self.sdk_pool,
            Some(self.config.store_encryption_key.as_str()),
        )
        .await
        .context("opening Matrix Postgres store")?;
        install_sdk_store_compatibility(&self.sdk_pool)
            .await
            .context("installing Matrix SDK Postgres compatibility")?;

        let client = Client::builder()
            .homeserver_url(self.config.homeserver_url.as_str())
            .store_config(store_config)
            .build()
            .await
            .context("creating Matrix client")?;

        let existing_device_id: Option<String> = sqlx::query_scalar(
            "SELECT device_id FROM matrix_service.client_state WHERE singleton = TRUE",
        )
        .fetch_optional(&self.pool)
        .await
        .context("loading Matrix device state")?;

        let login_response = client
            .login(
                self.config.user_id.as_str(),
                self.config.password.as_str(),
                existing_device_id.as_deref(),
                Some("n8n Matrix Service"),
            )
            .await
            .context("logging into Matrix")?;

        let device_id = login_response.device_id.to_string();
        sqlx::query(
            "INSERT INTO matrix_service.client_state (singleton, device_id, updated_at) \
             VALUES (TRUE, $1, NOW()) \
             ON CONFLICT (singleton) DO UPDATE SET device_id = EXCLUDED.device_id, updated_at = NOW()",
        )
        .bind(&device_id)
        .execute(&self.pool)
        .await
        .context("persisting Matrix device ID")?;

        client
            .sync_once(SyncSettings::default())
            .await
            .context("performing initial Matrix sync")?;

        let sync_client = client.clone();
        let sync_token = client
            .sync_token()
            .await
            .ok_or_else(|| anyhow!("initial Matrix sync did not return a token"))?;
        let sync_settings = SyncSettings::default().token(sync_token);
        tokio::spawn(async move {
            sync_client.sync(sync_settings).await;
            warn!("Matrix sync loop stopped");
        });

        *self.client.write().await = Some(client.clone());
        info!("Matrix client initialized");
        Ok(self.setup_status(Some(client)).await)
    }

    async fn setup_status(&self, known_client: Option<Client>) -> SetupStatus {
        let client = match known_client {
            Some(client) => Some(client),
            None => self.client.read().await.clone(),
        };

        let initialized = client.is_some();
        let device_id: Option<String> = if initialized {
            sqlx::query_scalar(
                "SELECT device_id FROM matrix_service.client_state WHERE singleton = TRUE",
            )
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
        } else {
            None
        };

        SetupStatus {
            initialized,
            device_id,
            default_room_id: self.config.default_room_id.clone(),
        }
    }

    async fn ready_client(&self) -> Result<Client, ApiError> {
        self.client
            .read()
            .await
            .clone()
            .ok_or_else(|| ApiError::service_unavailable("Matrix setup has not completed"))
    }
}

#[derive(Serialize)]
#[serde(crate = "rocket::serde")]
struct HealthResponse {
    status: &'static str,
}

#[derive(Serialize)]
#[serde(crate = "rocket::serde")]
struct SetupStatus {
    initialized: bool,
    device_id: Option<String>,
    default_room_id: String,
}

#[derive(Deserialize)]
#[serde(crate = "rocket::serde")]
struct EnableEncryptionRequest {
    confirm: bool,
}

#[derive(Deserialize)]
#[serde(crate = "rocket::serde")]
struct SendMessageRequest {
    message: String,
    room_id: Option<String>,
    format: Option<MessageFormat>,
    encrypted: Option<bool>,
    request_id: Option<String>,
    image_url: Option<String>,
    image_alt: Option<String>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(crate = "rocket::serde", rename_all = "lowercase")]
enum MessageFormat {
    Text,
    Markdown,
    Html,
}

impl MessageFormat {
    fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Markdown => "markdown",
            Self::Html => "html",
        }
    }
}

#[derive(Serialize)]
#[serde(crate = "rocket::serde")]
struct SendMessageResponse {
    event_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    image_event_id: Option<String>,
    room_id: String,
    encrypted: bool,
    idempotent_replay: bool,
    excluded_device_count: u32,
}

#[derive(Serialize)]
#[serde(crate = "rocket::serde")]
struct ErrorBody {
    error: ErrorDetails,
}

#[derive(Serialize)]
#[serde(crate = "rocket::serde")]
struct ErrorDetails {
    code: &'static str,
    message: String,
}

#[derive(Debug)]
struct ApiError {
    status: Status,
    code: &'static str,
    message: String,
}

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self { status: Status::BadRequest, code: "INVALID_REQUEST", message: message.into() }
    }

    fn unauthorized() -> Self {
        Self { status: Status::Unauthorized, code: "UNAUTHORIZED", message: "Missing or invalid API key".into() }
    }

    fn conflict(code: &'static str, message: impl Into<String>) -> Self {
        Self { status: Status::Conflict, code, message: message.into() }
    }

    fn service_unavailable(message: impl Into<String>) -> Self {
        Self { status: Status::ServiceUnavailable, code: "SETUP_REQUIRED", message: message.into() }
    }

    fn internal(error: anyhow::Error) -> Self {
        error!(error = ?error, "Matrix Service request failed");
        Self { status: Status::InternalServerError, code: "MATRIX_DELIVERY_FAILED", message: "Matrix delivery failed".into() }
    }
}

impl<'r> Responder<'r, 'static> for ApiError {
    fn respond_to(self, _: &'r Request<'_>) -> response::Result<'static> {
        let body = serde_json::to_string(&ErrorBody {
            error: ErrorDetails { code: self.code, message: self.message },
        })
        .expect("error body is serializable");

        Response::build()
            .status(self.status)
            .header(ContentType::JSON)
            .sized_body(body.len(), std::io::Cursor::new(body))
            .ok()
    }
}

struct ApiKey;

#[rocket::async_trait]
impl<'r> FromRequest<'r> for ApiKey {
    type Error = ApiError;

    async fn from_request(request: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        let Some(state) = request.rocket().state::<AppState>() else {
            return Outcome::Error((Status::InternalServerError, ApiError::service_unavailable("Service state is unavailable")));
        };

        let provided = request
            .headers()
            .get_one("Authorization")
            .and_then(|header| header.strip_prefix("Bearer "));

        match provided {
            Some(provided)
                if provided.as_bytes().ct_eq(state.config.service_api_key.as_bytes()).into() =>
            {
                Outcome::Success(ApiKey)
            }
            _ => Outcome::Error((Status::Unauthorized, ApiError::unauthorized())),
        }
    }
}

#[get("/healthz")]
fn healthz() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

#[get("/v1/setup/status")]
async fn setup_status(_key: ApiKey, state: &State<AppState>) -> Json<SetupStatus> {
    Json(state.setup_status(None).await)
}

#[post("/v1/setup/bootstrap")]
async fn bootstrap(_key: ApiKey, state: &State<AppState>) -> Result<Json<SetupStatus>, ApiError> {
    state.bootstrap().await.map(Json).map_err(ApiError::internal)
}

#[post("/v1/setup/rooms/<room_id>/enable-encryption", format = "json", data = "<request>")]
async fn enable_encryption(
    _key: ApiKey,
    room_id: &str,
    request: Json<EnableEncryptionRequest>,
    state: &State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !request.confirm {
        return Err(ApiError::bad_request("Set confirm to true to enable room encryption"));
    }

    let room_id: OwnedRoomId = room_id
        .parse()
        .map_err(|_| ApiError::bad_request("room_id must be a canonical Matrix room ID"))?;
    let client = state.ready_client().await?;
    let room = client
        .get_joined_room(&room_id)
        .ok_or_else(|| ApiError::bad_request("The Matrix account has not joined this room"))?;

    room.enable_encryption()
        .await
        .map_err(|error| {
            error!(room_id = %room_id, error = ?error, "Unable to enable Matrix room encryption");
            ApiError {
                status: Status::InternalServerError,
                code: "MATRIX_ENCRYPTION_ENABLE_FAILED",
                message: "Matrix could not enable room encryption; check Matrix Service logs".into(),
            }
        })?;
    Ok(Json(serde_json::json!({ "room_id": room_id, "encrypted": true })))
}

#[post("/v1/messages", format = "json", data = "<request>")]
async fn send_message(
    _key: ApiKey,
    request: Json<SendMessageRequest>,
    state: &State<AppState>,
) -> Result<Json<SendMessageResponse>, ApiError> {
    let request = request.into_inner();
    if request.message.trim().is_empty() {
        return Err(ApiError::bad_request("message must not be empty"));
    }

    let _send_guard = state.send_lock.lock().await;
    let format = request.format.unwrap_or(MessageFormat::Markdown);
    let encrypted = request.encrypted.unwrap_or(true);
    let room_id = request.room_id.unwrap_or_else(|| state.config.default_room_id.clone());
    let room_id: OwnedRoomId = room_id
        .parse()
        .map_err(|_| ApiError::bad_request("room_id must be a canonical Matrix room ID"))?;
    let client = state.ready_client().await?;
    let room = client
        .get_joined_room(&room_id)
        .ok_or_else(|| ApiError::bad_request("The Matrix account has not joined this room"))?;

    if encrypted && !room.is_encrypted() {
        return Err(ApiError {
            status: Status::Conflict,
            code: "ROOM_ENCRYPTION_REQUIRED",
            message: "The room is not configured for end-to-end encryption".into(),
        });
    }

    let request_hash = request_hash(
        &request.message,
        room_id.as_str(),
        format,
        encrypted,
        request.image_url.as_deref(),
        request.image_alt.as_deref(),
    );
    let request_key = request
        .request_id
        .as_deref()
        .map(|request_id| idempotency_key(request_id, room_id.as_str()));

    if let Some(request_key) = request_key.as_deref() {
        if let Some(existing) = load_idempotency_record(&state.pool, request_key).await.map_err(ApiError::internal)? {
            if existing.request_hash != request_hash {
                return Err(ApiError::conflict(
                    "IDEMPOTENCY_CONFLICT",
                    "request_id was already used with different delivery content or options",
                ));
            }

            if let Some(event_id) = existing.event_id {
                return Ok(Json(SendMessageResponse {
                    event_id,
                    image_event_id: None,
                    room_id: existing.room_id,
                    encrypted: existing.encrypted,
                    idempotent_replay: true,
                    excluded_device_count: 0,
                }));
            }
        }

        save_in_progress_record(&state.pool, request_key, &request_hash, room_id.as_str(), encrypted)
            .await
            .map_err(ApiError::internal)?;
    }

    let image_event_id = if let Some(image_url) = request.image_url.as_deref() {
        let image_alt = request
            .image_alt
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("Image attachment");
        let image = match fetch_and_resize_public_image(&client, image_url, image_alt).await {
            Ok(image) => image,
            Err(error) => {
                warn!(error = ?error, "Unable to prepare Matrix image attachment");
                delete_in_progress_record(&state.pool, request_key.as_deref()).await;
                return Err(ApiError::bad_request(
                    "image_url must be a reachable HTTPS image hosted by an approved public CDN",
                ));
            }
        };
        let image_transaction_id = stable_transaction_id(request_key.as_deref(), &request_hash, "image")
            .map_err(ApiError::internal)?;
        let response = if encrypted {
            send_encrypted(&room, image, &image_transaction_id).await
        } else {
            send_plaintext(&client, &room_id, image, &image_transaction_id).await
        };
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                delete_in_progress_record(&state.pool, request_key.as_deref()).await;
                return Err(ApiError::internal(error));
            }
        };
        Some(response.event_id.to_string())
    } else {
        None
    };

    let transaction_id = stable_transaction_id(request_key.as_deref(), &request_hash, "message")
        .map_err(ApiError::internal)?;
    let content = format_content(&request.message, format);
    let result = if encrypted {
        send_encrypted(&room, content, &transaction_id).await
    } else {
        send_plaintext(&client, &room_id, content, &transaction_id).await
    };

    let response = match result {
        Ok(response) => response,
        Err(error) => {
            delete_in_progress_record(&state.pool, request_key.as_deref()).await;
            return Err(ApiError::internal(error));
        }
    };

    let event_id = response.event_id.to_string();
    if let Some(request_key) = request_key.as_deref() {
        complete_idempotency_record(&state.pool, request_key, &event_id)
            .await
            .map_err(ApiError::internal)?;
    }

    Ok(Json(SendMessageResponse {
        event_id,
        image_event_id,
        room_id: room_id.to_string(),
        encrypted,
        idempotent_replay: false,
        excluded_device_count: 0,
    }))
}

#[get("/setup")]
fn setup_page() -> RawHtml<&'static str> {
    RawHtml(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>Matrix Service setup</title>
<style>
body{font-family:system-ui,sans-serif;max-width:48rem;margin:3rem auto;padding:0 1rem;line-height:1.5}
label{display:block;margin-top:1rem;font-weight:600}input{font:inherit;padding:.6rem;margin-top:.25rem;width:100%;box-sizing:border-box}
button{font:inherit;padding:.7rem 1rem;margin-top:1rem;border:1px solid #000;border-radius:.25rem;cursor:pointer}
button.primary{background:#000;color:#fff;font-weight:700}button.primary:hover{background:#262626}button:disabled{opacity:.6;cursor:wait}
#result{margin-top:1rem;padding:1rem;border-radius:.25rem;white-space:pre-wrap}#result[hidden]{display:none}.success{background:#e8f7eb;color:#124d20}.failure{background:#fdebec;color:#7d1017}.pending{background:#f2f2f2;color:#222}
</style>
</head><body><h1>Matrix Service setup</h1>
<p>This page performs no action until you supply the service API key and choose an action.</p>
<label>API key<input id="api-key" type="password" autocomplete="off"></label>
<button type="button" onclick="bootstrap()">Initialize Matrix device</button>
<label>Room ID<input id="room-id" placeholder="!room:server"></label>
<button id="enable-button" class="primary" type="button" onclick="enableEncryption()">Confirm and enable encryption</button>
<div id="result" role="status" aria-live="polite" hidden></div>
<script>
const apiRoot=window.location.pathname.startsWith('/matrix/')?'/matrix/v1':'/v1';
const result=document.getElementById('result');
function showResult(kind,message){result.className=kind;result.textContent=message;result.hidden=false;}
function errorMessage(response,body){const error=body&&body.error;const code=error&&error.code?error.code:`HTTP ${response.status}`;const message=error&&error.message?error.message:'The Matrix Service returned an unexpected response.';return `${code}: ${message}`;}
async function callApi(path, method, body) {
 const key=document.getElementById('api-key').value;
 if(!key){showResult('failure','Enter the Matrix Service API key before continuing.');return null;}
 showResult('pending','Working…');
 try {
  const response=await fetch(path,{method,headers:{Authorization:'Bearer '+key,'Content-Type':'application/json'},body:body?JSON.stringify(body):undefined});
  const text=await response.text();let payload=null;try{payload=text?JSON.parse(text):null;}catch{payload=null;}
  if(!response.ok){showResult('failure',`Setup failed. ${errorMessage(response,payload)}\n\nFor server-side details, run: docker compose logs matrix`);return null;}
  return payload;
 } catch(error) {
  console.error('Matrix setup request failed',error);
  showResult('failure','Setup could not reach the Matrix Service. Check your Tailscale connection, then run: docker compose logs matrix');
  return null;
 }
}
async function bootstrap(){const payload=await callApi(apiRoot+'/setup/bootstrap','POST');if(payload)showResult('success','Matrix device is ready. Enter the room ID below and confirm encryption if the room is not already encrypted.');}
async function enableEncryption(){const room=document.getElementById('room-id').value.trim();if(!room){showResult('failure','Enter the Matrix room ID to enable encryption.');return;}const button=document.getElementById('enable-button');button.disabled=true;try{const payload=await callApi(apiRoot+'/setup/rooms/'+encodeURIComponent(room)+'/enable-encryption','POST',{confirm:true});if(payload&&payload.encrypted)showResult('success',`Encryption is enabled for ${payload.room_id}. Matrix Sender can now deliver encrypted notifications to this room. You can close this window.`);}finally{button.disabled=false;}}
</script></body></html>"#,
    )
}

async fn send_encrypted(
    room: &Joined,
    content: RoomMessageEventContent,
    transaction_id: &OwnedTransactionId,
) -> Result<matrix_sdk::ruma::api::client::message::send_message_event::v3::Response> {
    room.send(content, Some(transaction_id))
        .await
        .map_err(|error| anyhow!("sending encrypted Matrix event: {error:?}"))
}

async fn send_plaintext(
    client: &Client,
    room_id: &OwnedRoomId,
    content: RoomMessageEventContent,
    transaction_id: &OwnedTransactionId,
) -> Result<matrix_sdk::ruma::api::client::message::send_message_event::v3::Response> {
    let raw_content = Raw::new(&content).context("serializing plaintext Matrix event")?.cast();
    let request = send_message_event::v3::Request::new_raw(
        room_id,
        transaction_id,
        "m.room.message".into(),
        raw_content,
    );
    client
        .send(request, None)
        .await
        .map_err(|error| anyhow!("sending plaintext Matrix event: {error:?}"))
}

fn format_content(message: &str, format: MessageFormat) -> RoomMessageEventContent {
    match format {
        MessageFormat::Text => RoomMessageEventContent::text_plain(message),
        MessageFormat::Markdown => {
            let mut html_body = String::new();
            html::push_html(
                &mut html_body,
                Parser::new_ext(message, Options::all()),
            );
            RoomMessageEventContent::text_html(message, ammonia::clean(&html_body))
        }
        MessageFormat::Html => {
            let html_body = ammonia::clean(message);
            let plain_body = html2text::from_read(html_body.as_bytes(), usize::MAX);
            RoomMessageEventContent::text_html(plain_body.trim(), html_body)
        }
    }
}

const PRODUCT_IMAGE_MAX_DIMENSION: u32 = 256;
const PRODUCT_IMAGE_MAX_BYTES: u64 = 10 * 1024 * 1024;

/// Fetches images only from the public CDNs used by configured notification
/// sources. Keeping this list explicit avoids turning the authenticated
/// delivery API into a general network-fetch endpoint.
async fn fetch_and_resize_public_image(
    matrix_client: &Client,
    image_url: &str,
    alt_text: &str,
) -> Result<RoomMessageEventContent> {
    let parsed = reqwest::Url::parse(image_url).context("parsing image URL")?;
    if parsed.scheme() != "https" || !is_allowed_image_host(parsed.host_str()) {
        return Err(anyhow!("image URL must use HTTPS and an approved public CDN"));
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("creating image download client")?;
    let response = client
        .get(parsed)
        .send()
        .await
        .context("downloading product image")?
        .error_for_status()
        .context("product image host returned an error")?;

    if response
        .content_length()
        .is_some_and(|length| length > PRODUCT_IMAGE_MAX_BYTES)
    {
        return Err(anyhow!("product image exceeds the 10 MiB download limit"));
    }

    let mut body = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.context("reading product image")?;
        if bytes.len() + chunk.len() > PRODUCT_IMAGE_MAX_BYTES as usize {
            return Err(anyhow!("product image exceeds the 10 MiB download limit"));
        }
        bytes.extend_from_slice(&chunk);
    }

    let source = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .context("detecting product image format")?
        .decode()
        .context("decoding product image")?;
    let resized = source.resize(
        PRODUCT_IMAGE_MAX_DIMENSION,
        PRODUCT_IMAGE_MAX_DIMENSION,
        FilterType::Lanczos3,
    );
    let (width, height) = resized.dimensions();
    let mut encoded = Vec::new();
    JpegEncoder::new_with_quality(&mut encoded, 85)
        .encode_image(&resized)
        .context("encoding resized product image")?;

    let size = UInt::try_from(encoded.len()).context("recording resized image size")?;
    let width = UInt::try_from(width).context("recording resized image width")?;
    let height = UInt::try_from(height).context("recording resized image height")?;
    let mut upload = Cursor::new(encoded);
    let mxc_uri = matrix_client
        .upload(&mime::IMAGE_JPEG, &mut upload)
        .await
        .context("uploading resized product image to Matrix")?
        .content_uri;
    let mut info = ImageInfo::new();
    info.width = Some(width);
    info.height = Some(height);
    info.mimetype = Some("image/jpeg".into());
    info.size = Some(size);
    Ok(RoomMessageEventContent::new(MessageType::Image(
        ImageMessageEventContent::plain(alt_text.to_owned(), mxc_uri, Some(Box::new(info))),
    )))
}

fn is_allowed_image_host(host: Option<&str>) -> bool {
    matches!(
        host,
        Some("cdn.shopify.com")
            | Some("images.puma.com")
            | Some("res.cloudinary.com")
            | Some("i.gr-assets.com")
    )
}

#[derive(sqlx::FromRow)]
struct IdempotencyRecord {
    request_hash: String,
    room_id: String,
    encrypted: bool,
    event_id: Option<String>,
}

async fn load_idempotency_record(pool: &PgPool, request_key: &str) -> Result<Option<IdempotencyRecord>> {
    sqlx::query_as(
        "SELECT request_hash, room_id, encrypted, event_id \
         FROM matrix_service.idempotency_records WHERE request_key = $1",
    )
    .bind(request_key)
    .fetch_optional(pool)
    .await
    .context("loading idempotency record")
}

async fn save_in_progress_record(
    pool: &PgPool,
    request_key: &str,
    request_hash: &str,
    room_id: &str,
    encrypted: bool,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO matrix_service.idempotency_records \
         (request_key, request_hash, room_id, encrypted, status) \
         VALUES ($1, $2, $3, $4, 'in_progress') \
         ON CONFLICT (request_key) DO NOTHING",
    )
    .bind(request_key)
    .bind(request_hash)
    .bind(room_id)
    .bind(encrypted)
    .execute(pool)
    .await
    .context("saving idempotency record")?;
    Ok(())
}

async fn complete_idempotency_record(pool: &PgPool, request_key: &str, event_id: &str) -> Result<()> {
    sqlx::query(
        "UPDATE matrix_service.idempotency_records \
         SET event_id = $2, status = 'complete', completed_at = NOW() \
         WHERE request_key = $1",
    )
    .bind(request_key)
    .bind(event_id)
    .execute(pool)
    .await
    .context("completing idempotency record")?;
    Ok(())
}

async fn delete_in_progress_record(pool: &PgPool, request_key: Option<&str>) {
    let Some(request_key) = request_key else {
        return;
    };
    let _ = sqlx::query(
        "DELETE FROM matrix_service.idempotency_records WHERE request_key = $1 AND status = 'in_progress'",
    )
    .bind(request_key)
    .execute(pool)
    .await;
}

fn request_hash(
    message: &str,
    room_id: &str,
    format: MessageFormat,
    encrypted: bool,
    image_url: Option<&str>,
    image_alt: Option<&str>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(message.as_bytes());
    hasher.update([0]);
    hasher.update(room_id.as_bytes());
    hasher.update([0]);
    hasher.update(format.as_str().as_bytes());
    hasher.update([0]);
    hasher.update([u8::from(encrypted)]);
    hasher.update([0]);
    hasher.update(image_url.unwrap_or_default().as_bytes());
    hasher.update([0]);
    hasher.update(image_alt.unwrap_or_default().as_bytes());
    hex::encode(hasher.finalize())
}

fn idempotency_key(request_id: &str, room_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(request_id.as_bytes());
    hasher.update([0]);
    hasher.update(room_id.as_bytes());
    hex::encode(hasher.finalize())
}

fn stable_transaction_id(request_key: Option<&str>, request_hash: &str, kind: &str) -> Result<OwnedTransactionId> {
    let value = request_key
        .map(|key| format!("n8n-{kind}-{}", &key[..28]))
        .unwrap_or_else(|| format!("n8n-{kind}-{}", Uuid::new_v4().simple()));
    value
        .try_into()
        .map_err(|_| anyhow!("unable to create Matrix transaction ID from {request_hash}"))
}

#[derive(Clone)]
struct CleanupFairing;

#[rocket::async_trait]
impl Fairing for CleanupFairing {
    fn info(&self) -> Info {
        Info { name: "idempotency cleanup", kind: Kind::Ignite }
    }

    async fn on_ignite(&self, rocket: rocket::Rocket<rocket::Build>) -> rocket::fairing::Result {
        let Some(state) = rocket.state::<AppState>() else {
            return Err(rocket);
        };
        let pool = state.pool.clone();
        let retention_days = state.config.idempotency_retention_days;
        tokio::spawn(async move {
            loop {
                if let Err(error) = sqlx::query(
                    "DELETE FROM matrix_service.idempotency_records \
                     WHERE status = 'complete' \
                     AND completed_at < NOW() - ($1 * INTERVAL '1 day')",
                )
                .bind(retention_days)
                .execute(&pool)
                .await
                {
                    warn!(%error, "Unable to clean Matrix idempotency records");
                }
                tokio::time::sleep(std::time::Duration::from_secs(24 * 60 * 60)).await;
            }
        });
        Ok(rocket)
    }
}

#[rocket::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_target(false)
        .init();

    let config = Config::from_env()?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&config.database_url)
        .await
        .context("connecting to Postgres")?;
    MIGRATOR.run(&pool).await.context("running Matrix Service migrations")?;

    // matrix-sdk-sql uses SQLx's standard migration table. Keep it in its own
    // schema so its migration history cannot collide with this service's
    // application migrations.
    let sdk_database_url = database_url_with_search_path(&config.database_url, "matrix_sdk");
    let sdk_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&sdk_database_url)
        .await
        .context("connecting Matrix SDK Postgres pool")?;

    let state = AppState {
        config,
        sdk_pool: Arc::new(sdk_pool),
        pool,
        client: RwLock::new(None),
        lifecycle_lock: Mutex::new(()),
        send_lock: Mutex::new(()),
    };

    match tokio::time::timeout(Duration::from_secs(30), state.bootstrap()).await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => {
            warn!(error = ?error, "Matrix client bootstrap did not complete; use the setup endpoint to retry");
        }
        Err(_) => {
            warn!("Matrix client bootstrap timed out; use the setup endpoint to retry");
        }
    }

    rocket::build()
        .manage(state)
        .attach(CleanupFairing)
        .mount(
            "/",
            rocket::routes![
                healthz,
                setup_status,
                bootstrap,
                enable_encryption,
                send_message,
                setup_page
            ],
        )
        .launch()
        .await
        .context("running Matrix Service")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_hash_changes_when_encryption_mode_changes() {
        let encrypted = request_hash("hello", "!room:example.org", MessageFormat::Markdown, true, None, None);
        let plaintext = request_hash("hello", "!room:example.org", MessageFormat::Markdown, false, None, None);
        assert_ne!(encrypted, plaintext);
    }

    #[test]
    fn request_hash_changes_when_image_changes() {
        let first = request_hash(
            "hello",
            "!room:example.org",
            MessageFormat::Markdown,
            true,
            Some("https://cdn.shopify.com/one.jpg"),
            Some("First image"),
        );
        let second = request_hash(
            "hello",
            "!room:example.org",
            MessageFormat::Markdown,
            true,
            Some("https://cdn.shopify.com/two.jpg"),
            Some("First image"),
        );
        assert_ne!(first, second);
    }

    #[test]
    fn public_image_hosts_are_explicitly_allowlisted() {
        assert!(is_allowed_image_host(Some("cdn.shopify.com")));
        assert!(is_allowed_image_host(Some("images.puma.com")));
        assert!(is_allowed_image_host(Some("res.cloudinary.com")));
        assert!(is_allowed_image_host(Some("i.gr-assets.com")));
        assert!(!is_allowed_image_host(Some("proton.me")));
        assert!(!is_allowed_image_host(Some("res.cloudinary.com.example.com")));
    }

    #[test]
    fn idempotency_key_is_scoped_to_the_room() {
        assert_ne!(
            idempotency_key("request-1", "!one:example.org"),
            idempotency_key("request-1", "!two:example.org"),
        );
    }

    #[test]
    fn markdown_content_has_html_representation() {
        let content = format_content("**bold**", MessageFormat::Markdown);
        assert_eq!(content.body(), "**bold**");
    }

    #[test]
    fn sdk_database_url_uses_its_own_search_path() {
        assert_eq!(
            database_url_with_search_path("postgresql://user:pass@db/app", "matrix_sdk"),
            "postgresql://user:pass@db/app?options%5Bsearch_path%5D=matrix_sdk"
        );
    }
}
