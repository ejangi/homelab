#![recursion_limit = "256"]

use std::{env, fs, io::Cursor, path::{Path, PathBuf}, time::Duration};

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use image::{codecs::jpeg::JpegEncoder, imageops::FilterType, GenericImageView, ImageReader};
use matrix_sdk::{
    config::SyncSettings,
    deserialized_responses::EncryptionInfo,
    room::Room,
    ruma::{
        events::room::{
            message::{
                ImageMessageEventContent, MessageType, OriginalSyncRoomMessageEvent,
                RoomMessageEventContent,
            },
            ImageInfo,
        },
        OwnedRoomId, OwnedTransactionId, OwnedUserId, UInt,
    },
    Client, RoomState,
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
use sqlx_core::{
    pool::Pool,
    postgres::{PgPoolOptions, Postgres},
    query::query,
    query_as::query_as,
    query_scalar::query_scalar,
};
use subtle::ConstantTimeEq;
use tokio::sync::{Mutex, RwLock};
use tracing::{error, info, warn};
use uuid::Uuid;

type PgPool = Pool<Postgres>;

async fn run_migrations(pool: &PgPool) -> Result<()> {
    query("CREATE SCHEMA IF NOT EXISTS matrix_service")
        .execute(pool)
        .await
        .context("creating Matrix Service schema")?;
    query(
        "CREATE TABLE IF NOT EXISTS matrix_service.service_migrations (version TEXT PRIMARY KEY, applied_at TIMESTAMPTZ NOT NULL DEFAULT NOW())",
    )
    .execute(pool)
    .await
    .context("creating Matrix Service migration ledger")?;

    const MIGRATIONS: [(&str, &str); 6] = [
        ("0001", include_str!("../migrations/0001_matrix_service.sql")),
        ("0002", include_str!("../migrations/0002_matrix_sdk_schema.sql")),
        ("0003", include_str!("../migrations/0003_matrix_delivery_monitor.sql")),
        ("0004", include_str!("../migrations/0004_matrix_monitor_identity.sql")),
        ("0005", include_str!("../migrations/0005_matrix_delivery_failures.sql")),
        ("0006", include_str!("../migrations/0006_matrix_idempotency_monitor_status.sql")),
    ];

    for (version, sql) in MIGRATIONS {
        let already_applied: bool = query_scalar(
            "SELECT EXISTS(SELECT 1 FROM matrix_service.service_migrations WHERE version = $1)",
        )
        .bind(version)
        .fetch_one(pool)
        .await
        .context("checking Matrix Service migration ledger")?;
        if already_applied {
            continue;
        }
        // sqlx-core's prepared-statement API accepts one PostgreSQL command at
        // a time. Service migrations deliberately contain only ordinary DDL
        // (no function bodies), so splitting their statement terminators is
        // safe and lets this small runner remain independent of sqlx macros.
        for statement in sql.split(';').map(str::trim).filter(|statement| !statement.is_empty()) {
            query(statement)
                .execute(pool)
                .await
                .with_context(|| format!("applying Matrix Service migration {version}"))?;
        }
        query("INSERT INTO matrix_service.service_migrations (version) VALUES ($1)")
            .bind(version)
            .execute(pool)
            .await
            .with_context(|| format!("recording Matrix Service migration {version}"))?;
    }
    Ok(())
}

#[derive(Clone)]
struct Config {
    homeserver_url: String,
    user_id: String,
    password: String,
    monitor_user_id: String,
    monitor_password: String,
    default_room_id: String,
    service_api_key: String,
    store_encryption_key: String,
    store_dir: PathBuf,
    database_url: String,
    idempotency_retention_days: i64,
}

impl Config {
    fn from_env() -> Result<Self> {
        Ok(Self {
            homeserver_url: required_env("MATRIX_HOMESERVER_URL")?,
            user_id: required_env("MATRIX_USER_ID")?,
            password: env::var("MATRIX_PASSWORD").unwrap_or_default(),
            monitor_user_id: required_env("MATRIX_MONITOR_USER_ID")?,
            monitor_password: required_env("MATRIX_MONITOR_PASSWORD")?,
            default_room_id: required_env("MATRIX_DEFAULT_ROOM_ID")?,
            service_api_key: required_env("MATRIX_SERVICE_API_KEY")?,
            store_encryption_key: required_env("MATRIX_STORE_ENCRYPTION_KEY")?,
            store_dir: env::var("MATRIX_STORE_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/matrix-store")),
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

fn monitor_store_directory(store_dir: &Path, user_id: &str) -> PathBuf {
    let mut hasher = Sha256::new();
    hasher.update(user_id.as_bytes());
    let digest = hex::encode(hasher.finalize());
    store_dir.join("monitor").join(&digest[..16])
}

fn joined_room(client: &Client, room_id: &OwnedRoomId) -> Option<Room> {
    client
        .get_room(room_id)
        .filter(|room| room.state() == RoomState::Joined)
}

fn start_matrix_sync_loop(client: Client, role: &'static str) {
    tokio::spawn(async move {
        loop {
            match client.sync(SyncSettings::default()).await {
                Ok(()) => warn!(role, "Matrix sync loop stopped unexpectedly; restarting"),
                Err(error) => warn!(role, error = ?error, "Matrix sync loop stopped; retrying"),
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
}

struct AppState {
    config: Config,
    pool: PgPool,
    client: RwLock<Option<Client>>,
    monitor_client: RwLock<Option<Client>>,
    lifecycle_lock: Mutex<()>,
    send_lock: Mutex<()>,
}

impl AppState {
    async fn bootstrap(&self) -> Result<SetupStatus> {
        let _guard = self.lifecycle_lock.lock().await;

        if let Some(client) = self.client.read().await.clone() {
            let monitor_client = match self.monitor_client.read().await.clone() {
                Some(monitor_client) => monitor_client,
                None => self.bootstrap_monitor().await?,
            };
            *self.monitor_client.write().await = Some(monitor_client.clone());
            return Ok(self.setup_status(Some(client), Some(monitor_client)).await);
        }

        if self.config.password.is_empty() {
            return Err(anyhow!(
                "MATRIX_PASSWORD is required before Matrix setup can run"
            ));
        }

        let sender_store_dir = self.config.store_dir.join("sender");
        fs::create_dir_all(&sender_store_dir)
            .context("creating Matrix sender SQLite store directory")?;
        let client = Client::builder()
            .homeserver_url(self.config.homeserver_url.as_str())
            .sqlite_store(&sender_store_dir, Some(self.config.store_encryption_key.as_str()))
            .build()
            .await
            .context("creating Matrix client")?;

        // The legacy Postgres crypto store is deliberately not reused. Passing
        // a device ID from another crypto store would claim keys we no longer
        // possess. The persisted ID belongs to this service's encrypted SDK
        // store, so it must be reused when a saved login session expires.
        if !client.matrix_auth().logged_in() {
            let sender_device_id: Option<String> = query_scalar(
                "SELECT device_id FROM matrix_service.client_state WHERE singleton = TRUE",
            )
            .fetch_optional(&self.pool)
            .await
            .context("loading persisted Matrix sender device ID")?;
            let mut login = client
                .matrix_auth()
                .login_username(self.config.user_id.as_str(), self.config.password.as_str())
                .initial_device_display_name("n8n Matrix Service");
            if let Some(device_id) = sender_device_id.as_deref() {
                login = login.device_id(device_id);
            }
            login
                .send()
                .await
                .context("logging into Matrix")?;
        }
        let device_id = client
            .device_id()
            .ok_or_else(|| anyhow!("Matrix sender login did not return a device ID"))?
            .to_string();
        query(
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

        start_matrix_sync_loop(client.clone(), "sender");

        *self.client.write().await = Some(client.clone());
        info!("Matrix sender client initialized");

        let monitor_client = self.bootstrap_monitor().await?;
        *self.monitor_client.write().await = Some(monitor_client.clone());
        info!("Matrix delivery monitor initialized");
        Ok(self.setup_status(Some(client), Some(monitor_client)).await)
    }

    fn monitor_store_encryption_key(&self) -> String {
        // The monitor must not share the sender's SDK store. Deriving a
        // separate at-rest key avoids another operator-managed secret while
        // keeping its private crypto state cryptographically distinct.
        let mut hasher = Sha256::new();
        hasher.update(self.config.store_encryption_key.as_bytes());
        hasher.update(b"\0matrix-delivery-monitor-store-v1");
        hasher.update(self.config.monitor_user_id.as_bytes());
        hex::encode(hasher.finalize())
    }

    async fn bootstrap_monitor(&self) -> Result<Client> {
        let monitor_store_key = self.monitor_store_encryption_key();
        let monitor_store_dir = monitor_store_directory(
            &self.config.store_dir,
            self.config.monitor_user_id.as_str(),
        );
        fs::create_dir_all(&monitor_store_dir)
            .context("creating Matrix delivery-monitor SQLite store directory")?;
        let client = Client::builder()
            .homeserver_url(self.config.homeserver_url.as_str())
            .sqlite_store(&monitor_store_dir, Some(monitor_store_key.as_str()))
            .build()
            .await
            .context("creating Matrix delivery-monitor client")?;

        if !client.matrix_auth().logged_in() {
            let monitor_device_id: Option<String> = query_scalar(
                "SELECT monitor_device_id FROM matrix_service.client_state \
                 WHERE singleton = TRUE AND monitor_user_id = $1",
            )
            .bind(&self.config.monitor_user_id)
            .fetch_optional(&self.pool)
            .await
            .context("loading persisted Matrix delivery-monitor device ID")?;
            let mut login = client
                .matrix_auth()
                .login_username(
                    self.config.monitor_user_id.as_str(),
                    self.config.monitor_password.as_str(),
                )
                .initial_device_display_name("n8n Matrix Delivery Monitor");
            if let Some(device_id) = monitor_device_id.as_deref() {
                login = login.device_id(device_id);
            }
            login
                .send()
                .await
                .context("logging in Matrix delivery monitor")?;
        }
        let device_id = client
            .device_id()
            .ok_or_else(|| anyhow!("Matrix delivery-monitor login did not return a device ID"))?
            .to_string();
        query(
            "UPDATE matrix_service.client_state \
             SET monitor_user_id = $1, monitor_device_id = $2, updated_at = NOW() \
             WHERE singleton = TRUE",
        )
        .bind(&self.config.monitor_user_id)
        .bind(&device_id)
        .execute(&self.pool)
        .await
        .context("persisting Matrix delivery-monitor device ID")?;

        let receipt_pool = self.pool.clone();
        let sender_user_id = self.config.user_id.clone();
        client.add_event_handler(
                move |event: OriginalSyncRoomMessageEvent,
                      room: Room,
                      encryption_info: Option<EncryptionInfo>| {
                    let receipt_pool = receipt_pool.clone();
                    let sender_user_id = sender_user_id.clone();
                    async move {
                        if encryption_info.is_none() || event.sender.as_str() != sender_user_id {
                            return;
                        }
                        if let Err(error) = query(
                            "INSERT INTO matrix_service.monitor_receipts \
                             (event_id, room_id, sender) VALUES ($1, $2, $3) \
                             ON CONFLICT (event_id) DO NOTHING",
                        )
                        .bind(event.event_id.as_str())
                        .bind(room.room_id().as_str())
                        .bind(event.sender.as_str())
                        .execute(&receipt_pool)
                        .await
                        {
                            error!(error = ?error, event_id = %event.event_id, "Unable to persist Matrix delivery-monitor receipt");
                        }
                    }
                },
            );
        client
            .sync_once(SyncSettings::default())
            .await
            .context("performing initial Matrix delivery-monitor sync")?;
        let default_room_id: OwnedRoomId = self
            .config
            .default_room_id
            .parse()
            .context("parsing default Matrix room ID for delivery monitor")?;
        if joined_room(&client, &default_room_id).is_none() {
            return Err(anyhow!(
                "Matrix delivery-monitor account has not joined the default room; invite {} and accept the invitation",
                self.config.monitor_user_id
            ));
        }

        start_matrix_sync_loop(client.clone(), "delivery monitor");
        Ok(client)
    }

    async fn setup_status(
        &self,
        known_client: Option<Client>,
        known_monitor_client: Option<Client>,
    ) -> SetupStatus {
        let client = match known_client {
            Some(client) => Some(client),
            None => self.client.read().await.clone(),
        };

        let initialized = client.is_some();
        let monitor_initialized =
            known_monitor_client.is_some() || self.monitor_client.read().await.is_some();
        let device_id: Option<String> = if initialized {
            query_scalar(
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
            monitor_initialized,
            monitor_device_id: if monitor_initialized {
                query_scalar(
                    "SELECT monitor_device_id FROM matrix_service.client_state WHERE singleton = TRUE",
                )
                .fetch_optional(&self.pool)
                .await
                .ok()
                .flatten()
            } else {
                None
            },
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

    async fn ready_monitor_client(&self) -> Result<Client, ApiError> {
        self.monitor_client.read().await.clone().ok_or_else(|| {
            ApiError::service_unavailable("Matrix delivery monitor has not completed setup")
        })
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
    monitor_initialized: bool,
    monitor_device_id: Option<String>,
    default_room_id: String,
}

#[derive(Deserialize)]
#[serde(crate = "rocket::serde")]
struct EnableEncryptionRequest {
    confirm: bool,
}

#[derive(Deserialize)]
#[serde(crate = "rocket::serde")]
struct RotateEncryptionSessionRequest {
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
    #[serde(skip_serializing_if = "Option::is_none")]
    monitor_verified: Option<bool>,
    idempotent_replay: bool,
    excluded_device_count: u32,
}

#[derive(Serialize)]
#[serde(crate = "rocket::serde")]
struct VerificationRequestResponse {
    requested_device_ids: Vec<String>,
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
        Self {
            status: Status::BadRequest,
            code: "INVALID_REQUEST",
            message: message.into(),
        }
    }

    fn unauthorized() -> Self {
        Self {
            status: Status::Unauthorized,
            code: "UNAUTHORIZED",
            message: "Missing or invalid API key".into(),
        }
    }

    fn conflict(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: Status::Conflict,
            code,
            message: message.into(),
        }
    }

    fn service_unavailable(message: impl Into<String>) -> Self {
        Self {
            status: Status::ServiceUnavailable,
            code: "SETUP_REQUIRED",
            message: message.into(),
        }
    }

    fn internal(error: anyhow::Error) -> Self {
        error!(error = ?error, "Matrix Service request failed");
        Self {
            status: Status::InternalServerError,
            code: "MATRIX_DELIVERY_FAILED",
            message: "Matrix delivery failed".into(),
        }
    }

}

impl<'r> Responder<'r, 'static> for ApiError {
    fn respond_to(self, _: &'r Request<'_>) -> response::Result<'static> {
        let body = serde_json::to_string(&ErrorBody {
            error: ErrorDetails {
                code: self.code,
                message: self.message,
            },
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
            return Outcome::Error((
                Status::InternalServerError,
                ApiError::service_unavailable("Service state is unavailable"),
            ));
        };

        let provided = request
            .headers()
            .get_one("Authorization")
            .and_then(|header| header.strip_prefix("Bearer "));

        match provided {
            Some(provided)
                if provided
                    .as_bytes()
                    .ct_eq(state.config.service_api_key.as_bytes())
                    .into() =>
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
    Json(state.setup_status(None, None).await)
}

#[post("/v1/setup/bootstrap")]
async fn bootstrap(_key: ApiKey, state: &State<AppState>) -> Result<Json<SetupStatus>, ApiError> {
    state
        .bootstrap()
        .await
        .map(Json)
        .map_err(ApiError::internal)
}

#[post("/v1/setup/monitor/request-verification")]
async fn request_monitor_verification(
    _key: ApiKey,
    state: &State<AppState>,
) -> Result<Json<VerificationRequestResponse>, ApiError> {
    let client = state.ready_monitor_client().await?;
    let monitor_user_id: OwnedUserId = state.config.monitor_user_id.parse().map_err(|_| {
        ApiError::bad_request("MATRIX_MONITOR_USER_ID is not a canonical Matrix user ID")
    })?;
    let own_device_id: Option<String> = query_scalar(
        "SELECT monitor_device_id FROM matrix_service.client_state WHERE singleton = TRUE",
    )
    .fetch_one(&state.pool)
    .await
    .map_err(|error| ApiError::internal(anyhow!(error)))?;
    let devices = client
        .encryption()
        .get_user_devices(&monitor_user_id)
        .await
        .map_err(|error| ApiError::internal(anyhow!(error)))?;
    let mut requested_device_ids = Vec::new();
    for device in devices.devices() {
        let device_id = device.device_id().to_string();
        if own_device_id.as_deref() == Some(device_id.as_str()) {
            continue;
        }
        device
            .request_verification()
            .await
            .map_err(|error| ApiError::internal(anyhow!(error)))?;
        requested_device_ids.push(device_id);
    }
    if requested_device_ids.is_empty() {
        return Err(ApiError::bad_request(
            "No other Matrix devices are available for verification",
        ));
    }
    info!(
        monitor_user_id = %state.config.monitor_user_id,
        device_count = requested_device_ids.len(),
        "Matrix device-verification requests sent"
    );
    Ok(Json(VerificationRequestResponse {
        requested_device_ids,
    }))
}

#[post(
    "/v1/setup/rooms/<room_id>/enable-encryption",
    format = "json",
    data = "<request>"
)]
async fn enable_encryption(
    _key: ApiKey,
    room_id: &str,
    request: Json<EnableEncryptionRequest>,
    state: &State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !request.confirm {
        return Err(ApiError::bad_request(
            "Set confirm to true to enable room encryption",
        ));
    }

    let room_id: OwnedRoomId = room_id
        .parse()
        .map_err(|_| ApiError::bad_request("room_id must be a canonical Matrix room ID"))?;
    let client = state.ready_client().await?;
    let room = joined_room(&client, &room_id)
        .ok_or_else(|| ApiError::bad_request("The Matrix account has not joined this room"))?;

    room.enable_encryption().await.map_err(|error| {
        error!(room_id = %room_id, error = ?error, "Unable to enable Matrix room encryption");
        ApiError {
            status: Status::InternalServerError,
            code: "MATRIX_ENCRYPTION_ENABLE_FAILED",
            message: "Matrix could not enable room encryption; check Matrix Service logs".into(),
        }
    })?;
    Ok(Json(
        serde_json::json!({ "room_id": room_id, "encrypted": true }),
    ))
}

#[post(
    "/v1/setup/rooms/<room_id>/rotate-encryption-session",
    format = "json",
    data = "<request>"
)]
async fn rotate_encryption_session(
    _key: ApiKey,
    room_id: &str,
    request: Json<RotateEncryptionSessionRequest>,
    state: &State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !request.confirm {
        return Err(ApiError::bad_request(
            "Set confirm to true to rotate the room encryption session",
        ));
    }

    // The maintained SDK discards the active session in place. The next send
    // creates and shares a replacement key without restarting the service.
    let _send_guard = state.send_lock.lock().await;
    let room_id: OwnedRoomId = room_id
        .parse()
        .map_err(|_| ApiError::bad_request("room_id must be a canonical Matrix room ID"))?;
    let client = state.ready_client().await?;
    let room = joined_room(&client, &room_id)
        .ok_or_else(|| ApiError::bad_request("The Matrix account has not joined this room"))?;
    if !room
        .latest_encryption_state()
        .await
        .map_err(|error| ApiError::internal(anyhow!(error)))?
        .is_encrypted()
    {
        return Err(ApiError {
            status: Status::Conflict,
            code: "ROOM_ENCRYPTION_REQUIRED",
            message: "The room is not configured for end-to-end encryption".into(),
        });
    }
    room
        .discard_room_key()
        .await
        .map_err(|error| ApiError::internal(anyhow!(error)))?;

    info!(room_id = %room_id, "Matrix outbound encryption session invalidated");
    Ok(Json(serde_json::json!({
        "room_id": room_id,
        "encrypted": true,
        "invalidated": true,
        "restart_required": false,
    })))
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
    let room_id = request
        .room_id
        .unwrap_or_else(|| state.config.default_room_id.clone());
    let room_id: OwnedRoomId = room_id
        .parse()
        .map_err(|_| ApiError::bad_request("room_id must be a canonical Matrix room ID"))?;
    let client = state.ready_client().await?;
    let room = joined_room(&client, &room_id)
        .ok_or_else(|| ApiError::bad_request("The Matrix account has not joined this room"))?;

    let room_is_encrypted = room
        .latest_encryption_state()
        .await
        .map_err(|error| ApiError::internal(anyhow!(error)))?
        .is_encrypted();
    if encrypted && !room_is_encrypted {
        return Err(ApiError {
            status: Status::Conflict,
            code: "ROOM_ENCRYPTION_REQUIRED",
            message: "The room is not configured for end-to-end encryption".into(),
        });
    }
    if !encrypted && room_is_encrypted {
        return Err(ApiError {
            status: Status::Conflict,
            code: "ROOM_ENCRYPTION_REQUIRED",
            message: "Plaintext delivery is not permitted in an encrypted room".into(),
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
        if let Some(existing) = load_idempotency_record(&state.pool, request_key)
            .await
            .map_err(ApiError::internal)?
        {
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
                    monitor_verified: existing.monitor_verified,
                    idempotent_replay: true,
                    excluded_device_count: 0,
                }));
            }
        }

        save_in_progress_record(
            &state.pool,
            request_key,
            &request_hash,
            room_id.as_str(),
            encrypted,
        )
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
        let image_transaction_id =
            stable_transaction_id(request_key.as_deref(), &request_hash, "image")
                .map_err(ApiError::internal)?;
        let response = if encrypted {
            send_encrypted(&room, image, &image_transaction_id).await
        } else {
            send_plaintext(&room, image, &image_transaction_id).await
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
        send_encrypted(&room, content.clone(), &transaction_id).await
    } else {
        send_plaintext(&room, content.clone(), &transaction_id).await
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

    let monitor_verified = if encrypted {
        match wait_for_monitor_receipt(&state.pool, &event_id).await {
            Ok(()) => {
                if let Err(error) =
                    update_monitor_verification(&state.pool, request_key.as_deref(), true).await
                {
                    warn!(event_id = %event_id, error = ?error, "Unable to record successful Matrix monitor verification");
                }
                Some(true)
            }
            Err(error) => {
                if let Err(record_error) =
                    record_monitor_delivery_failure(&state.pool, &event_id, room_id.as_str(), 1).await
                {
                    warn!(event_id = %event_id, error = ?record_error, "Unable to record Matrix monitor delivery failure");
                }
                if let Err(record_error) =
                    update_monitor_verification(&state.pool, request_key.as_deref(), false).await
                {
                    warn!(event_id = %event_id, error = ?record_error, "Unable to record failed Matrix monitor verification");
                }

                // The event has already been accepted by Matrix. Retrying its
                // content would create a second visible notification, so only
                // prepare a new outbound session for a future delivery.
                if let Err(rotate_error) = room.discard_room_key().await {
                    warn!(
                        event_id = %event_id,
                        error = ?rotate_error,
                        "Matrix monitor did not decrypt event and the outbound session could not be rotated"
                    );
                }
                warn!(
                    event_id = %event_id,
                    error = ?error,
                    "Matrix monitor did not decrypt event; preserving the accepted event without retrying it"
                );
                Some(false)
            }
        }
    } else {
        None
    };

    Ok(Json(SendMessageResponse {
        event_id,
        image_event_id,
        room_id: room_id.to_string(),
        encrypted,
        monitor_verified,
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
<button id="rotate-button" type="button" onclick="rotateEncryptionSession()">Confirm and rotate encryption session</button>
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
async function rotateEncryptionSession(){const room=document.getElementById('room-id').value.trim();if(!room){showResult('failure','Enter the Matrix room ID to rotate its encryption session.');return;}const button=document.getElementById('rotate-button');button.disabled=true;try{const payload=await callApi(apiRoot+'/setup/rooms/'+encodeURIComponent(room)+'/rotate-encryption-session','POST',{confirm:true});if(payload&&payload.restart_required)showResult('success',`The outbound encryption session for ${payload.room_id} was invalidated. Restart the Matrix service before sending another encrypted notification.`);}finally{button.disabled=false;}}
</script></body></html>"#,
    )
}

async fn send_encrypted(
    room: &Room,
    content: RoomMessageEventContent,
    transaction_id: &OwnedTransactionId,
) -> Result<matrix_sdk::ruma::api::client::message::send_message_event::v3::Response> {
    room.send(content)
        .with_transaction_id(transaction_id.to_owned())
        .await
        .map(|response| response.response)
        .map_err(|error| anyhow!("sending encrypted Matrix event: {error:?}"))
}

async fn wait_for_monitor_receipt(pool: &PgPool, event_id: &str) -> Result<()> {
    let wait_for_receipt = async {
        loop {
            let received: bool = query_scalar(
                "SELECT EXISTS(SELECT 1 FROM matrix_service.monitor_receipts WHERE event_id = $1)",
            )
            .bind(event_id)
            .fetch_one(pool)
            .await
            .context("checking Matrix delivery-monitor receipt")?;
            if received {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(45), wait_for_receipt)
        .await
        .map_err(|_| {
            anyhow!("no decryption receipt for encrypted Matrix event within 45 seconds")
        })?
}

async fn record_monitor_delivery_failure(
    pool: &PgPool,
    event_id: &str,
    room_id: &str,
    attempt: i16,
) -> Result<()> {
    query(
        "INSERT INTO matrix_service.monitor_delivery_failures \
         (event_id, room_id, attempt, failure_kind) \
         VALUES ($1, $2, $3, 'monitor_decrypt_timeout') \
         ON CONFLICT (event_id) DO NOTHING",
    )
    .bind(event_id)
    .bind(room_id)
    .bind(attempt)
    .execute(pool)
    .await
    .context("recording Matrix monitor delivery failure")?;
    Ok(())
}

async fn send_plaintext(
    room: &Room,
    content: RoomMessageEventContent,
    transaction_id: &OwnedTransactionId,
) -> Result<matrix_sdk::ruma::api::client::message::send_message_event::v3::Response> {
    room.send(content)
        .with_transaction_id(transaction_id.to_owned())
        .await
        .map(|response| response.response)
        .map_err(|error| anyhow!("sending plaintext Matrix event: {error:?}"))
}

fn format_content(message: &str, format: MessageFormat) -> RoomMessageEventContent {
    match format {
        MessageFormat::Text => RoomMessageEventContent::text_plain(message),
        MessageFormat::Markdown => {
            let mut html_body = String::new();
            html::push_html(&mut html_body, Parser::new_ext(message, Options::all()));
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
        return Err(anyhow!(
            "image URL must use HTTPS and an approved public CDN"
        ));
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
    let mxc_uri = matrix_client
        .media()
        .upload(&mime::IMAGE_JPEG, encoded, None)
        .await
        .context("uploading resized product image to Matrix")?
        .content_uri;
    let mut info = ImageInfo::new();
    info.width = Some(width);
    info.height = Some(height);
    info.mimetype = Some("image/jpeg".into());
    info.size = Some(size);
    let mut image = ImageMessageEventContent::plain(alt_text.to_owned(), mxc_uri);
    image.info = Some(Box::new(info));
    Ok(RoomMessageEventContent::new(MessageType::Image(image)))
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

struct IdempotencyRecord {
    request_hash: String,
    room_id: String,
    encrypted: bool,
    event_id: Option<String>,
    monitor_verified: Option<bool>,
}

async fn load_idempotency_record(
    pool: &PgPool,
    request_key: &str,
) -> Result<Option<IdempotencyRecord>> {
    let record: Option<(String, String, bool, Option<String>, Option<bool>)> = query_as(
        "SELECT request_hash, room_id, encrypted, event_id, monitor_verified \
         FROM matrix_service.idempotency_records WHERE request_key = $1",
    )
    .bind(request_key)
    .fetch_optional(pool)
    .await
    .context("loading idempotency record")?;
    Ok(record.map(|(request_hash, room_id, encrypted, event_id, monitor_verified)| IdempotencyRecord {
        request_hash,
        room_id,
        encrypted,
        event_id,
        monitor_verified,
    }))
}

async fn save_in_progress_record(
    pool: &PgPool,
    request_key: &str,
    request_hash: &str,
    room_id: &str,
    encrypted: bool,
) -> Result<()> {
    query(
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

async fn complete_idempotency_record(
    pool: &PgPool,
    request_key: &str,
    event_id: &str,
) -> Result<()> {
    query(
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

async fn update_monitor_verification(
    pool: &PgPool,
    request_key: Option<&str>,
    monitor_verified: bool,
) -> Result<()> {
    let Some(request_key) = request_key else {
        return Ok(());
    };
    query(
        "UPDATE matrix_service.idempotency_records \
         SET monitor_verified = $2 WHERE request_key = $1",
    )
    .bind(request_key)
    .bind(monitor_verified)
    .execute(pool)
    .await
    .context("recording Matrix monitor verification result")?;
    Ok(())
}

async fn delete_in_progress_record(pool: &PgPool, request_key: Option<&str>) {
    let Some(request_key) = request_key else {
        return;
    };
    let _ = query(
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

fn stable_transaction_id(
    request_key: Option<&str>,
    request_hash: &str,
    kind: &str,
) -> Result<OwnedTransactionId> {
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
        Info {
            name: "idempotency cleanup",
            kind: Kind::Ignite,
        }
    }

    async fn on_ignite(&self, rocket: rocket::Rocket<rocket::Build>) -> rocket::fairing::Result {
        let Some(state) = rocket.state::<AppState>() else {
            return Err(rocket);
        };
        let pool = state.pool.clone();
        let retention_days = state.config.idempotency_retention_days;
        tokio::spawn(async move {
            loop {
                if let Err(error) = query(
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
    run_migrations(&pool)
        .await
        .context("running Matrix Service migrations")?;

    let state = AppState {
        config,
        pool,
        client: RwLock::new(None),
        monitor_client: RwLock::new(None),
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
                request_monitor_verification,
                enable_encryption,
                rotate_encryption_session,
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
        let encrypted = request_hash(
            "hello",
            "!room:example.org",
            MessageFormat::Markdown,
            true,
            None,
            None,
        );
        let plaintext = request_hash(
            "hello",
            "!room:example.org",
            MessageFormat::Markdown,
            false,
            None,
            None,
        );
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
        assert!(!is_allowed_image_host(Some(
            "res.cloudinary.com.example.com"
        )));
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
    fn monitor_store_is_stable_and_identity_scoped() {
        let root = Path::new("/matrix-store");
        assert_eq!(
            monitor_store_directory(root, "@one:example.org"),
            monitor_store_directory(root, "@one:example.org")
        );
        assert_ne!(
            monitor_store_directory(root, "@one:example.org"),
            monitor_store_directory(root, "@two:example.org")
        );
    }

    #[test]
    fn response_includes_monitor_status_only_for_encrypted_delivery() {
        let encrypted = serde_json::to_value(SendMessageResponse {
            event_id: "$event:example.org".into(),
            image_event_id: None,
            room_id: "!room:example.org".into(),
            encrypted: true,
            monitor_verified: Some(false),
            idempotent_replay: false,
            excluded_device_count: 0,
        })
        .expect("response is serializable");
        assert_eq!(encrypted["monitor_verified"], false);

        let plaintext = serde_json::to_value(SendMessageResponse {
            event_id: "$event:example.org".into(),
            image_event_id: None,
            room_id: "!room:example.org".into(),
            encrypted: false,
            monitor_verified: None,
            idempotent_replay: false,
            excluded_device_count: 0,
        })
        .expect("response is serializable");
        assert!(plaintext.get("monitor_verified").is_none());
    }
}
