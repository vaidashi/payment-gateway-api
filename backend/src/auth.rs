use actix_web::{
    HttpRequest, HttpResponse,
    cookie::{Cookie, SameSite},
};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

pub const SESSION_COOKIE: &str = "demo_session";
#[derive(Clone, Debug)]
pub struct Identity {
    pub user_id: Option<Uuid>,
    pub role: Option<String>,
    pub csrf: String,
}
pub fn token_hash(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}
fn random() -> String {
    Uuid::new_v4().simple().to_string() + &Uuid::new_v4().simple().to_string()
}
pub async fn bootstrap(pool: &PgPool) -> Result<(String, Identity), sqlx::Error> {
    let token = random();
    let csrf = random();
    sqlx::query("INSERT INTO sessions(token_hash, csrf_token, expires_at) VALUES ($1,$2,now()+interval '8 hours')").bind(token_hash(&token)).bind(&csrf).execute(pool).await?;
    Ok((
        token,
        Identity {
            user_id: None,
            role: None,
            csrf,
        },
    ))
}
pub async fn identity(
    pool: &PgPool,
    request: &HttpRequest,
) -> Result<Option<Identity>, sqlx::Error> {
    let Some(cookie) = request.cookie(SESSION_COOKIE) else {
        return Ok(None);
    };
    let row=sqlx::query_as::<_,(Option<Uuid>,Option<String>,String)>("SELECT s.user_id,u.role,s.csrf_token FROM sessions s LEFT JOIN users u ON u.id=s.user_id WHERE s.token_hash=$1 AND s.expires_at > now()").bind(token_hash(cookie.value())).fetch_optional(pool).await?;
    Ok(row.map(|(user_id, role, csrf)| Identity {
        user_id,
        role,
        csrf,
    }))
}
pub fn require_mutation(request: &HttpRequest, identity: &Identity) -> Result<(), HttpResponse> {
    let origin = request
        .headers()
        .get("Origin")
        .and_then(|v| v.to_str().ok());
    let expected =
        request.connection_info().scheme().to_owned() + "://" + request.connection_info().host();
    if origin != Some(expected.as_str()) {
        return Err(HttpResponse::Forbidden().json(serde_json::json!({"code":"invalid_origin"})));
    }
    let csrf = request
        .headers()
        .get("X-CSRF-Token")
        .and_then(|v| v.to_str().ok());
    if csrf != Some(identity.csrf.as_str()) {
        return Err(HttpResponse::Forbidden().json(serde_json::json!({"code":"invalid_csrf"})));
    }
    Ok(())
}
pub fn session_cookie(token: String) -> Cookie<'static> {
    Cookie::build(SESSION_COOKIE, token)
        .http_only(true)
        .same_site(SameSite::Lax)
        .path("/")
        .finish()
}
