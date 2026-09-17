use crate::{
    auth,
    orders::{self, CreateOrderRequest, OrderError, OrderStatus, StatusCommand, VersionedCommand},
};
use actix_web::{HttpRequest, HttpResponse, web};
use sqlx::PgPool;
use uuid::Uuid;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/api/orders", web::post().to(create))
        .route("/api/orders", web::get().to(list))
        .route("/api/orders/{id}", web::get().to(detail))
        .route("/api/orders/{id}/cancel", web::post().to(cancel))
        .route("/api/orders/{id}/status", web::patch().to(status));
}
fn key(req: &HttpRequest) -> Result<Uuid, HttpResponse> {
    req.headers()
        .get("Idempotency-Key")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| Uuid::parse_str(v).ok())
        .ok_or_else(|| {
            HttpResponse::BadRequest().json(serde_json::json!({"code":"invalid_idempotency_key"}))
        })
}
async fn actor(pool: &PgPool, req: &HttpRequest) -> Result<auth::Identity, HttpResponse> {
    auth::identity(pool, req)
        .await
        .map_err(|_| HttpResponse::InternalServerError().finish())?
        .ok_or_else(|| HttpResponse::Unauthorized().finish())
}
fn order_error(error: OrderError) -> HttpResponse {
    match error {
        OrderError::NotFound => HttpResponse::NotFound().finish(),
        OrderError::Forbidden => HttpResponse::Forbidden().finish(),
        OrderError::Conflict(code) => {
            HttpResponse::Conflict().json(serde_json::json!({"code":code}))
        }
        OrderError::Invalid(code) => {
            HttpResponse::UnprocessableEntity().json(serde_json::json!({"code":code}))
        }
        OrderError::Database(_) => HttpResponse::InternalServerError().finish(),
    }
}
async fn create(
    pool: web::Data<PgPool>,
    req: HttpRequest,
    body: web::Json<CreateOrderRequest>,
) -> HttpResponse {
    let identity = match actor(&pool, &req).await {
        Ok(identity) => identity,
        Err(response) => return response,
    };
    if identity.role.as_deref() != Some("customer") {
        return HttpResponse::Forbidden().finish();
    }
    if let Err(response) = auth::require_mutation(&req, &identity) {
        return response;
    }
    let key = match key(&req) {
        Ok(key) => key,
        Err(response) => return response,
    };
    match orders::create(&pool, identity.user_id.unwrap(), key, body.into_inner()).await {
        Ok((status, body)) => {
            HttpResponse::build(actix_web::http::StatusCode::from_u16(status).unwrap()).json(body)
        }
        Err(error) => order_error(error),
    }
}
async fn list(pool: web::Data<PgPool>, req: HttpRequest) -> HttpResponse {
    let identity = match actor(&pool, &req).await {
        Ok(identity) => identity,
        Err(response) => return response,
    };
    match orders::list(
        &pool,
        identity.user_id.unwrap(),
        identity.role.as_deref().unwrap_or(""),
    )
    .await
    {
        Ok(body) => HttpResponse::Ok().json(body),
        Err(error) => order_error(error),
    }
}
async fn detail(pool: web::Data<PgPool>, req: HttpRequest, path: web::Path<Uuid>) -> HttpResponse {
    let identity = match actor(&pool, &req).await {
        Ok(identity) => identity,
        Err(response) => return response,
    };
    match orders::read(
        &pool,
        identity.user_id.unwrap(),
        identity.role.as_deref().unwrap_or(""),
        *path,
    )
    .await
    {
        Ok(body) => HttpResponse::Ok().json(body),
        Err(error) => order_error(error),
    }
}
async fn cancel(
    pool: web::Data<PgPool>,
    req: HttpRequest,
    path: web::Path<Uuid>,
    body: web::Json<VersionedCommand>,
) -> HttpResponse {
    command(
        &pool,
        &req,
        *path,
        OrderStatus::Cancelled,
        body.expected_version,
    )
    .await
}
async fn status(
    pool: web::Data<PgPool>,
    req: HttpRequest,
    path: web::Path<Uuid>,
    body: web::Json<StatusCommand>,
) -> HttpResponse {
    command(&pool, &req, *path, body.status, body.expected_version).await
}
async fn command(
    pool: &PgPool,
    req: &HttpRequest,
    order_id: Uuid,
    requested: OrderStatus,
    expected_version: i64,
) -> HttpResponse {
    let identity = match actor(pool, req).await {
        Ok(identity) => identity,
        Err(response) => return response,
    };
    if let Err(response) = auth::require_mutation(req, &identity) {
        return response;
    }
    let key = match key(req) {
        Ok(key) => key,
        Err(response) => return response,
    };
    match orders::transition(
        pool,
        identity.user_id.unwrap(),
        identity.role.as_deref().unwrap_or(""),
        order_id,
        key,
        requested,
        expected_version,
    )
    .await
    {
        Ok((status, body)) => {
            HttpResponse::build(actix_web::http::StatusCode::from_u16(status).unwrap()).json(body)
        }
        Err(error) => order_error(error),
    }
}
