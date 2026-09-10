use actix_web::{App, HttpServer, web};
use food_ordering_runtime::{
    http::catalog,
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
    catalog::seed_database(&pool).await?;
    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(pool.clone()))
            .configure(catalog::configure)
    })
    .bind(listen_address("API_LISTEN_ADDR", "127.0.0.1:8080"))?
    .run()
    .await?;
    Ok(())
}
