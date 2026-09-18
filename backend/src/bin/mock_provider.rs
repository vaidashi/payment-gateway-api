use actix_web::{App, HttpServer, web};
use food_ordering_runtime::{
    mock_provider::{self, ProviderConfig, ProviderState},
    runtime::{DatabaseKind, database_pool, listen_address, required_secret},
};
use std::time::Duration;

#[actix_web::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config = ProviderConfig {
        service_token: required_secret("PROVIDER_SERVICE_TOKEN")?,
        callback_url: required_secret("PROVIDER_CALLBACK_URL")?,
        callback_signing_secret: required_secret("PROVIDER_CALLBACK_SIGNING_SECRET")?,
    };
    let pool = database_pool(DatabaseKind::Provider).await?;
    let callback_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()?;
    let state = ProviderState {
        pool,
        config,
        callback_client,
    };
    let retry_state = state.clone();
    tokio::spawn(async move {
        loop {
            if let Err(error) = mock_provider::settle_due_attempts(&retry_state.pool).await {
                eprintln!("mock provider attempt settlement failed: {error}");
            }
            if let Err(error) = mock_provider::deliver_due_callbacks(&retry_state).await {
                eprintln!("mock provider callback delivery failed: {error}");
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(state.clone()))
            .route(
                "/health/live",
                web::get().to(food_ordering_runtime::runtime::live),
            )
            .route(
                "/health/ready",
                web::get().to(food_ordering_runtime::runtime::ready),
            )
            .configure(mock_provider::configure)
    })
    .bind(listen_address("PROVIDER_LISTEN_ADDR", "127.0.0.1:8083"))?
    .run()
    .await?;
    Ok(())
}
