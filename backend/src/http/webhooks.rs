use actix_web::{HttpRequest, HttpResponse, web};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use sqlx::PgPool;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
pub struct WebhookConfig {
    pub signing_secret: String,
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/webhooks/mock-payments", web::post().to(receive));
}

fn verified(req: &HttpRequest, body: &[u8], secret: &str) -> Option<String> {
    let event_id = req
        .headers()
        .get("X-Provider-Event-Id")?
        .to_str()
        .ok()?
        .to_owned();
    let timestamp: i64 = req
        .headers()
        .get("X-Provider-Timestamp")?
        .to_str()
        .ok()?
        .parse()
        .ok()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    if (now - timestamp).abs() > 300 {
        return None;
    }
    let supplied = req
        .headers()
        .get("X-Provider-Signature")?
        .to_str()
        .ok()?
        .strip_prefix("v1=")?;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    let expected = hex::decode(supplied).ok()?;
    mac.verify_slice(&expected).ok()?;
    Some(event_id)
}

async fn receive(
    pool: web::Data<PgPool>,
    config: web::Data<WebhookConfig>,
    req: HttpRequest,
    body: web::Bytes,
) -> HttpResponse {
    if body.len() > 256 * 1024 {
        return HttpResponse::PayloadTooLarge().finish();
    }
    let Some(event_id) = verified(&req, &body, &config.signing_secret) else {
        return HttpResponse::Unauthorized().finish();
    };
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(_) => return HttpResponse::InternalServerError().finish(),
    };
    let inserted = match sqlx::query("INSERT INTO payment_inbox(event_id,raw_body) VALUES($1,$2) ON CONFLICT(event_id) DO NOTHING")
        .bind(&event_id).bind(body.as_ref()).execute(&mut *tx).await { Ok(x) => x, Err(_) => return HttpResponse::InternalServerError().finish() };
    if inserted.rows_affected() == 1 {
        if sqlx::query("INSERT INTO durable_jobs(id,kind,dedupe_key,payload) VALUES($1,'process_inbox',$2,$3) ON CONFLICT(dedupe_key) DO NOTHING")
            .bind(Uuid::new_v4()).bind(format!("inbox:{event_id}")).bind(serde_json::json!({"event_id":event_id})).execute(&mut *tx).await.is_err() { return HttpResponse::InternalServerError().finish(); }
    }
    match tx.commit().await {
        Ok(_) => HttpResponse::Accepted().finish(),
        Err(_) => HttpResponse::InternalServerError().finish(),
    }
}
