use actix_web::{App, HttpServer, web};
use food_ordering_runtime::{
    http::{catalog, webhooks},
    runtime::{DatabaseKind, database_pool, listen_address},
};

#[actix_web::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let pool = database_pool(DatabaseKind::App).await?;
    let webhook_secret =
        food_ordering_runtime::runtime::required_secret("PROVIDER_CALLBACK_SIGNING_SECRET")?;
    catalog::seed_database(&pool).await?;
    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(pool.clone()))
            .app_data(web::Data::new(webhooks::WebhookConfig {
                signing_secret: webhook_secret.clone(),
            }))
            .configure(catalog::configure)
            .configure(webhooks::configure)
    })
    .bind(listen_address("API_LISTEN_ADDR", "127.0.0.1:8080"))?
    .run()
    .await?;
    Ok(())
}
