use actix_web::{App, HttpResponse, HttpServer, web};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::{env, fmt, time::Duration};

#[derive(Clone, Copy)]
pub enum DatabaseKind {
    App,
    Provider,
}

impl DatabaseKind {
    fn variable(self) -> &'static str {
        match self {
            Self::App => "APP_DATABASE_URL",
            Self::Provider => "PROVIDER_DATABASE_URL",
        }
    }
}

#[derive(Debug)]
pub struct RuntimeConfigError {
    variable: &'static str,
}

impl fmt::Display for RuntimeConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "missing required configuration: {} (copy .env.example to .env for local Compose use)",
            self.variable
        )
    }
}

impl std::error::Error for RuntimeConfigError {}

pub fn database_url(kind: DatabaseKind) -> Result<String, RuntimeConfigError> {
    let variable = kind.variable();
    env::var(variable).map_err(|_| RuntimeConfigError { variable })
}

pub fn required_secret(variable: &'static str) -> Result<String, RuntimeConfigError> {
    env::var(variable).map_err(|_| RuntimeConfigError { variable })
}

pub async fn database_pool(kind: DatabaseKind) -> Result<PgPool, Box<dyn std::error::Error>> {
    let url = database_url(kind)?;
    Ok(PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(Duration::from_secs(1))
        .connect(&url)
        .await?)
}

pub async fn live() -> HttpResponse {
    HttpResponse::Ok().json(serde_json::json!({ "status": "live" }))
}

pub async fn ready(pool: web::Data<PgPool>) -> HttpResponse {
    match sqlx::query("SELECT 1").execute(pool.get_ref()).await {
        Ok(_) => HttpResponse::Ok().json(serde_json::json!({ "status": "ready" })),
        Err(_) => {
            HttpResponse::ServiceUnavailable().json(serde_json::json!({ "status": "not_ready" }))
        }
    }
}

pub async fn serve_health(
    process_name: &'static str,
    bind_address: String,
    pool: PgPool,
) -> std::io::Result<()> {
    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(pool.clone()))
            .route("/health/live", web::get().to(live))
            .route("/health/ready", web::get().to(ready))
            .route(
                "/",
                web::get().to(move || async move {
                    HttpResponse::Ok().json(serde_json::json!({ "process": process_name }))
                }),
            )
    })
    .bind(bind_address)?
    .run()
    .await
}

pub fn listen_address(variable: &'static str, fallback: &'static str) -> String {
    env::var(variable).unwrap_or_else(|_| fallback.to_owned())
}
