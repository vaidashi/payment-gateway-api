use food_ordering_runtime::runtime::{DatabaseKind, database_pool, listen_address, serve_health};

#[actix_web::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let pool = database_pool(DatabaseKind::App).await?;
    serve_health(
        "api",
        listen_address("API_LISTEN_ADDR", "127.0.0.1:8080"),
        pool,
    )
    .await?;
    Ok(())
}
