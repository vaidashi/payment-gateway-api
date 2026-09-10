use food_ordering_runtime::runtime::{
    DatabaseKind, database_pool, listen_address, required_secret, serve_health,
};

#[actix_web::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let _service_token = required_secret("PROVIDER_SERVICE_TOKEN")?;
    let pool = database_pool(DatabaseKind::Provider).await?;
    serve_health(
        "mock_provider",
        listen_address("PROVIDER_LISTEN_ADDR", "127.0.0.1:8083"),
        pool,
    )
    .await?;
    Ok(())
}
