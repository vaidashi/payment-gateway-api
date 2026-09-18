//! A deliberately small, Stripe-shaped provider boundary for local integration tests.
//! The provider owns this state; application code communicates only over its HTTP API.

use actix_web::{HttpRequest, HttpResponse, web};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use std::{
    fmt,
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
pub struct ProviderConfig {
    pub service_token: String,
    pub callback_url: String,
    pub callback_signing_secret: String,
}

#[derive(Clone)]
pub struct ProviderState {
    pub pool: PgPool,
    pub config: ProviderConfig,
    pub callback_client: reqwest::Client,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct IntentRequest {
    pub intent_id: String,
    pub amount_cents: i64,
    pub currency: String,
    pub callback_url: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AttemptRequest {
    pub attempt_id: String,
    pub scenario: Scenario,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RefundRequest {
    pub amount_cents: i64,
    pub currency: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Scenario {
    Success,
    Decline,
    DelayedSuccess,
    DuplicateCallback,
}
impl Scenario {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Decline => "decline",
            Self::DelayedSuccess => "delayed_success",
            Self::DuplicateCallback => "duplicate_callback",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttemptResponse {
    pub intent_id: String,
    pub attempt_id: String,
    pub outcome: String,
    pub capture_id: Option<String>,
}

#[derive(Debug)]
pub enum ProviderError {
    Invalid(&'static str),
    Conflict(&'static str),
    NotFound,
    Database(sqlx::Error),
}
impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ProviderError {}
impl From<sqlx::Error> for ProviderError {
    fn from(value: sqlx::Error) -> Self {
        Self::Database(value)
    }
}

fn digest<T: Serialize>(value: &T) -> Result<String, ProviderError> {
    let bytes = serde_json::to_vec(value).map_err(|_| ProviderError::Invalid("invalid_payload"))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}
fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}
fn currency(value: &str) -> Result<(), ProviderError> {
    if value == "usd" {
        Ok(())
    } else {
        Err(ProviderError::Invalid("invalid_currency"))
    }
}
fn idempotency(req: &HttpRequest) -> Result<Uuid, ProviderError> {
    req.headers()
        .get("Idempotency-Key")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| Uuid::parse_str(v).ok())
        .ok_or(ProviderError::Invalid("invalid_idempotency_key"))
}
fn authorized(req: &HttpRequest, config: &ProviderConfig) -> bool {
    req.headers()
        .get("X-Provider-Service-Token")
        .and_then(|v| v.to_str().ok())
        == Some(config.service_token.as_str())
}
fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
fn callback_signature(secret: &str, timestamp: i64, body: &[u8]) -> String {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts arbitrary key length");
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/v1/payment_intents", web::post().to(create_intent_http))
        .route(
            "/v1/payment_intents/{intent_id}",
            web::get().to(read_intent_http),
        )
        .route(
            "/v1/payment_intents/{intent_id}/attempts",
            web::post().to(confirm_http),
        )
        .route(
            "/v1/payment_intents/{intent_id}/refunds",
            web::post().to(refund_http),
        );
}

fn error_response(error: ProviderError) -> HttpResponse {
    match error {
        ProviderError::Invalid(code) => {
            HttpResponse::UnprocessableEntity().json(serde_json::json!({"code":code}))
        }
        ProviderError::Conflict(code) => {
            HttpResponse::Conflict().json(serde_json::json!({"code":code}))
        }
        ProviderError::NotFound => HttpResponse::NotFound().finish(),
        ProviderError::Database(_) => HttpResponse::InternalServerError().finish(),
    }
}
fn guard(req: &HttpRequest, state: &ProviderState) -> Result<Uuid, HttpResponse> {
    if !authorized(req, &state.config) {
        return Err(HttpResponse::Unauthorized().finish());
    }
    idempotency(req).map_err(error_response)
}
async fn create_intent_http(
    state: web::Data<ProviderState>,
    req: HttpRequest,
    body: web::Json<IntentRequest>,
) -> HttpResponse {
    let key = match guard(&req, &state) {
        Ok(key) => key,
        Err(response) => return response,
    };
    match create_intent(
        &state.pool,
        key,
        body.into_inner(),
        &state.config.callback_url,
    )
    .await
    {
        Ok(body) => HttpResponse::Created().json(body),
        Err(error) => error_response(error),
    }
}
async fn read_intent_http(
    state: web::Data<ProviderState>,
    req: HttpRequest,
    path: web::Path<String>,
) -> HttpResponse {
    if !authorized(&req, &state.config) {
        return HttpResponse::Unauthorized().finish();
    }
    match read_intent(&state.pool, &path).await {
        Ok(body) => HttpResponse::Ok().json(body),
        Err(error) => error_response(error),
    }
}
async fn confirm_http(
    state: web::Data<ProviderState>,
    req: HttpRequest,
    path: web::Path<String>,
    body: web::Json<AttemptRequest>,
) -> HttpResponse {
    let key = match guard(&req, &state) {
        Ok(key) => key,
        Err(response) => return response,
    };
    match confirm_attempt(&state.pool, key, &path, body.into_inner()).await {
        Ok(body) => HttpResponse::Ok().json(body),
        Err(error) => error_response(error),
    }
}
async fn refund_http(
    state: web::Data<ProviderState>,
    req: HttpRequest,
    path: web::Path<String>,
    body: web::Json<RefundRequest>,
) -> HttpResponse {
    let key = match guard(&req, &state) {
        Ok(key) => key,
        Err(response) => return response,
    };
    match refund(&state.pool, key, &path, body.into_inner()).await {
        Ok(body) => HttpResponse::Ok().json(body),
        Err(error) => error_response(error),
    }
}

pub async fn create_intent(
    pool: &PgPool,
    key: Uuid,
    request: IntentRequest,
    allowed_callback_url: &str,
) -> Result<IntentRequest, ProviderError> {
    if !valid_id(&request.intent_id) || request.amount_cents < 0 {
        return Err(ProviderError::Invalid("invalid_intent"));
    }
    currency(&request.currency)?;
    if request.callback_url != allowed_callback_url {
        return Err(ProviderError::Invalid("invalid_callback_url"));
    }
    let input_digest = digest(&request)?;
    let mut tx = pool.begin().await?;
    if let Some(row) = sqlx::query("SELECT input_digest, response FROM provider_idempotency_keys WHERE operation='intent' AND idempotency_key=$1 FOR UPDATE").bind(key).fetch_optional(&mut *tx).await? {
        if row.get::<String,_>("input_digest") != input_digest { return Err(ProviderError::Conflict("idempotency_input_mismatch")); }
        return serde_json::from_value(row.get("response")).map_err(|_| ProviderError::Database(sqlx::Error::Protocol("invalid stored response".into())));
    }
    if let Some(row) = sqlx::query(
        "SELECT amount_cents,currency,callback_url FROM provider_intents WHERE id=$1 FOR UPDATE",
    )
    .bind(&request.intent_id)
    .fetch_optional(&mut *tx)
    .await?
    {
        if row.get::<i64, _>("amount_cents") != request.amount_cents
            || row.get::<String, _>("currency") != request.currency
            || row.get::<String, _>("callback_url") != request.callback_url
        {
            return Err(ProviderError::Conflict("intent_identity_mismatch"));
        }
    } else {
        sqlx::query("INSERT INTO provider_intents(id,amount_cents,currency,callback_url) VALUES($1,$2,$3,$4)").bind(&request.intent_id).bind(request.amount_cents).bind(&request.currency).bind(&request.callback_url).execute(&mut *tx).await?;
    }
    sqlx::query("INSERT INTO provider_idempotency_keys(operation,idempotency_key,input_digest,response) VALUES('intent',$1,$2,$3)").bind(key).bind(input_digest).bind(serde_json::to_value(&request).unwrap()).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(request)
}

pub async fn confirm_attempt(
    pool: &PgPool,
    key: Uuid,
    intent_id: &str,
    request: AttemptRequest,
) -> Result<AttemptResponse, ProviderError> {
    if !valid_id(intent_id) || !valid_id(&request.attempt_id) {
        return Err(ProviderError::Invalid("invalid_attempt"));
    }
    let input_digest = digest(&(intent_id, &request))?;
    let mut tx = pool.begin().await?;
    if let Some(row) = sqlx::query("SELECT input_digest,response FROM provider_idempotency_keys WHERE operation='attempt' AND idempotency_key=$1 FOR UPDATE").bind(key).fetch_optional(&mut *tx).await? {
        if row.get::<String,_>("input_digest") != input_digest { return Err(ProviderError::Conflict("idempotency_input_mismatch")); }
        return serde_json::from_value(row.get("response")).map_err(|_| ProviderError::Database(sqlx::Error::Protocol("invalid stored response".into())));
    }
    let intent = sqlx::query("SELECT callback_url FROM provider_intents WHERE id=$1 FOR UPDATE")
        .bind(intent_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ProviderError::NotFound)?;
    if let Some(row) = sqlx::query("SELECT outcome FROM provider_attempts WHERE id=$1 FOR UPDATE")
        .bind(&request.attempt_id)
        .fetch_optional(&mut *tx)
        .await?
    {
        let capture_id = sqlx::query_scalar("SELECT id FROM provider_captures WHERE attempt_id=$1")
            .bind(&request.attempt_id)
            .fetch_optional(&mut *tx)
            .await?;
        let response = AttemptResponse {
            intent_id: intent_id.into(),
            attempt_id: request.attempt_id.clone(),
            outcome: row.get("outcome"),
            capture_id,
        };
        sqlx::query("INSERT INTO provider_idempotency_keys(operation,idempotency_key,input_digest,response) VALUES('attempt',$1,$2,$3)").bind(key).bind(input_digest).bind(serde_json::to_value(&response).unwrap()).execute(&mut *tx).await?;
        tx.commit().await?;
        return Ok(response);
    }
    if sqlx::query_scalar::<_, i64>("SELECT count(*) FROM provider_captures WHERE intent_id=$1")
        .bind(intent_id)
        .fetch_one(&mut *tx)
        .await?
        > 0
    {
        return Err(ProviderError::Conflict("intent_already_captured"));
    }
    if sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM provider_attempts WHERE intent_id=$1 AND outcome='pending'",
    )
    .bind(intent_id)
    .fetch_one(&mut *tx)
    .await?
        > 0
    {
        return Err(ProviderError::Conflict("attempt_pending"));
    }
    let outcome = match request.scenario {
        Scenario::Decline => "declined",
        Scenario::DelayedSuccess => "pending",
        _ => "succeeded",
    };
    let available_at = matches!(request.scenario, Scenario::DelayedSuccess).then_some(5_i64);
    sqlx::query("INSERT INTO provider_attempts(id,intent_id,scenario,outcome,available_at) VALUES($1,$2,$3,$4, CASE WHEN $5::bigint IS NULL THEN NULL ELSE now() + ($5::text || ' seconds')::interval END)").bind(&request.attempt_id).bind(intent_id).bind(request.scenario.as_str()).bind(outcome).bind(available_at).execute(&mut *tx).await?;
    let capture_id = if outcome == "succeeded" {
        Some(
            capture_and_enqueue(
                &mut tx,
                intent_id,
                &request.attempt_id,
                intent.get("callback_url"),
                matches!(request.scenario, Scenario::DuplicateCallback),
            )
            .await?,
        )
    } else {
        None
    };
    let response = AttemptResponse {
        intent_id: intent_id.into(),
        attempt_id: request.attempt_id,
        outcome: outcome.into(),
        capture_id,
    };
    sqlx::query("INSERT INTO provider_idempotency_keys(operation,idempotency_key,input_digest,response) VALUES('attempt',$1,$2,$3)").bind(key).bind(input_digest).bind(serde_json::to_value(&response).unwrap()).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(response)
}

async fn capture_and_enqueue(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    intent_id: &str,
    attempt_id: &str,
    callback_url: String,
    duplicate: bool,
) -> Result<String, ProviderError> {
    let row = sqlx::query("SELECT amount_cents,currency FROM provider_intents WHERE id=$1")
        .bind(intent_id)
        .fetch_one(&mut **tx)
        .await?;
    let capture_id = format!("ch_{attempt_id}");
    sqlx::query("INSERT INTO provider_captures(id,intent_id,attempt_id,amount_cents,currency) VALUES($1,$2,$3,$4,$5) ON CONFLICT(intent_id) DO NOTHING").bind(&capture_id).bind(intent_id).bind(attempt_id).bind(row.get::<i64,_>("amount_cents")).bind(row.get::<String,_>("currency")).execute(&mut **tx).await?;
    let event_id = format!("evt_{attempt_id}");
    let payload = serde_json::json!({"id":event_id,"type":"payment_intent.succeeded","data":{"intent_id":intent_id,"attempt_id":attempt_id,"capture_id":capture_id,"amount_cents":row.get::<i64,_>("amount_cents"),"currency":row.get::<String,_>("currency")}});
    sqlx::query("INSERT INTO provider_callback_events(id,intent_id,attempt_id,payload) VALUES($1,$2,$3,$4) ON CONFLICT(id) DO NOTHING").bind(&event_id).bind(intent_id).bind(attempt_id).bind(payload).execute(&mut **tx).await?;
    for _ in 0..if duplicate { 3 } else { 1 } {
        sqlx::query(
            "INSERT INTO provider_callback_deliveries(id,event_id,callback_url) VALUES($1,$2,$3)",
        )
        .bind(Uuid::new_v4())
        .bind(&event_id)
        .bind(&callback_url)
        .execute(&mut **tx)
        .await?;
    }
    Ok(capture_id)
}

pub async fn settle_due_attempts(pool: &PgPool) -> Result<usize, ProviderError> {
    let mut tx = pool.begin().await?;
    let rows = sqlx::query("SELECT a.id,a.intent_id,i.callback_url FROM provider_attempts a JOIN provider_intents i ON i.id=a.intent_id WHERE a.outcome='pending' AND a.available_at <= now() ORDER BY a.available_at LIMIT 20 FOR UPDATE SKIP LOCKED").fetch_all(&mut *tx).await?;
    for row in &rows {
        sqlx::query(
            "UPDATE provider_attempts SET outcome='succeeded',updated_at=now() WHERE id=$1",
        )
        .bind(row.get::<String, _>("id"))
        .execute(&mut *tx)
        .await?;
        capture_and_enqueue(
            &mut tx,
            &row.get::<String, _>("intent_id"),
            &row.get::<String, _>("id"),
            row.get("callback_url"),
            false,
        )
        .await?;
    }
    tx.commit().await?;
    Ok(rows.len())
}

pub async fn refund(
    pool: &PgPool,
    key: Uuid,
    intent_id: &str,
    request: RefundRequest,
) -> Result<serde_json::Value, ProviderError> {
    currency(&request.currency)?;
    if request.amount_cents < 0 {
        return Err(ProviderError::Invalid("invalid_refund"));
    }
    let input_digest = digest(&(intent_id, &request))?;
    let mut tx = pool.begin().await?;
    if let Some(row) = sqlx::query("SELECT input_digest,response FROM provider_idempotency_keys WHERE operation='refund' AND idempotency_key=$1 FOR UPDATE").bind(key).fetch_optional(&mut *tx).await? { if row.get::<String,_>("input_digest") != input_digest { return Err(ProviderError::Conflict("idempotency_input_mismatch")); } return Ok(row.get("response")); }
    let capture = sqlx::query(
        "SELECT id,amount_cents,currency FROM provider_captures WHERE intent_id=$1 FOR UPDATE",
    )
    .bind(intent_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(ProviderError::Conflict("intent_not_captured"))?;
    if capture.get::<i64, _>("amount_cents") != request.amount_cents
        || capture.get::<String, _>("currency") != request.currency
    {
        return Err(ProviderError::Conflict("refund_identity_mismatch"));
    }
    let refund_id = format!("re_{}", capture.get::<String, _>("id"));
    sqlx::query("INSERT INTO provider_refunds(id,capture_id,amount_cents,currency) VALUES($1,$2,$3,$4) ON CONFLICT(capture_id) DO NOTHING").bind(&refund_id).bind(capture.get::<String,_>("id")).bind(request.amount_cents).bind(&request.currency).execute(&mut *tx).await?;
    let body = serde_json::json!({"id":refund_id,"capture_id":capture.get::<String,_>("id"),"amount_cents":request.amount_cents,"currency":request.currency,"outcome":"succeeded"});
    sqlx::query("INSERT INTO provider_idempotency_keys(operation,idempotency_key,input_digest,response) VALUES('refund',$1,$2,$3)").bind(key).bind(input_digest).bind(&body).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(body)
}

pub async fn read_intent(
    pool: &PgPool,
    intent_id: &str,
) -> Result<serde_json::Value, ProviderError> {
    let row = sqlx::query("SELECT i.id,i.amount_cents,i.currency,(SELECT id FROM provider_captures c WHERE c.intent_id=i.id) AS capture_id FROM provider_intents i WHERE i.id=$1").bind(intent_id).fetch_optional(pool).await?.ok_or(ProviderError::NotFound)?;
    Ok(
        serde_json::json!({"id":row.get::<String,_>("id"),"amount_cents":row.get::<i64,_>("amount_cents"),"currency":row.get::<String,_>("currency"),"capture_id":row.get::<Option<String>,_>("capture_id")}),
    )
}

pub async fn deliver_due_callbacks(state: &ProviderState) -> Result<usize, ProviderError> {
    // Lease state is committed before HTTP. The network call therefore never holds a database
    // transaction, while another provider process cannot double-send an in-flight delivery.
    let lease_token = Uuid::new_v4();
    let mut tx = state.pool.begin().await?;
    let rows = sqlx::query("SELECT d.id,d.event_id,d.callback_url,e.payload FROM provider_callback_deliveries d JOIN provider_callback_events e ON e.id=d.event_id WHERE d.delivered_at IS NULL AND d.due_at <= now() AND (d.lease_expires_at IS NULL OR d.lease_expires_at < now()) ORDER BY d.due_at LIMIT 20 FOR UPDATE SKIP LOCKED").fetch_all(&mut *tx).await?;
    for row in &rows {
        sqlx::query("UPDATE provider_callback_deliveries SET lease_token=$1,lease_expires_at=now() + interval '60 seconds' WHERE id=$2")
            .bind(lease_token).bind(row.get::<Uuid, _>("id")).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    let mut delivered = 0;
    for row in rows {
        let body = serde_json::to_vec(&row.get::<serde_json::Value, _>("payload"))
            .map_err(|_| ProviderError::Invalid("callback_payload"))?;
        let timestamp = now_seconds();
        let signature = callback_signature(&state.config.callback_signing_secret, timestamp, &body);
        let result = state
            .callback_client
            .post(row.get::<String, _>("callback_url"))
            .header("Content-Type", "application/json")
            .header("X-Provider-Event-Id", row.get::<String, _>("event_id"))
            .header("X-Provider-Timestamp", timestamp)
            .header("X-Provider-Signature", format!("v1={signature}"))
            .body(body)
            .send()
            .await;
        match result {
            Ok(response) if response.status().is_success() => {
                sqlx::query("UPDATE provider_callback_deliveries SET delivered_at=now(),attempt_count=attempt_count+1,last_error=NULL,lease_token=NULL,lease_expires_at=NULL WHERE id=$1 AND lease_token=$2").bind(row.get::<Uuid,_>("id")).bind(lease_token).execute(&state.pool).await?;
                delivered += 1;
            }
            _ => {
                sqlx::query("UPDATE provider_callback_deliveries SET attempt_count=attempt_count+1,due_at=now() + interval '1 second',last_error='delivery_failed',lease_token=NULL,lease_expires_at=NULL WHERE id=$1 AND lease_token=$2").bind(row.get::<Uuid,_>("id")).bind(lease_token).execute(&state.pool).await?;
            }
        }
    }
    Ok(delivered)
}
