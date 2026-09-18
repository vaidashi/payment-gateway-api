use food_ordering_runtime::{
    jobs, payments,
    provider_client::ProviderClient,
    runtime::{DatabaseKind, database_pool, listen_address, serve_health},
};
use sqlx::Row;

#[actix_web::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let pool = database_pool(DatabaseKind::App).await?;
    let provider = ProviderClient::from_env()?;
    let callback_url = std::env::var("PROVIDER_CALLBACK_URL")?;
    let worker_pool = pool.clone();
    tokio::spawn(async move {
        loop {
            match jobs::claim(&worker_pool, 20).await {
                Ok(claimed) => {
                    for job in claimed {
                        let result: Result<(), String> = match job.kind.as_str() {
                            "process_inbox" => match job.payload.get("event_id").and_then(|value| value.as_str()) {
                                Some(id) => payments::process_inbox(&worker_pool, &provider, id).await.map_err(|error| error.to_string()),
                                None => Err("missing_event_id".into()),
                            },
                            "refund" => match job.payload.get("capture_id").and_then(|value| value.as_str()) {
                                Some(capture_id) => match sqlx::query("SELECT provider_intent_id,amount_cents FROM refund_obligations WHERE provider_capture_id=$1").bind(capture_id).fetch_optional(&worker_pool).await {
                                    Ok(Some(row)) => match provider.refund(job.id, &row.get::<String,_>("provider_intent_id"), row.get("amount_cents")).await {
                                        Ok(body) => payments::complete_refund(&worker_pool, capture_id, body.get("id").and_then(|value| value.as_str()).unwrap_or("")).await.map_err(|error| error.to_string()),
                                        Err(error) => Err(error.to_string()),
                                    },
                                    Ok(None) => Ok(()),
                                    Err(error) => Err(error.to_string()),
                                },
                                None => Err("missing_capture_id".into()),
                            },
                            "capture" => match job.payload.get("provider_attempt_id").and_then(|value| value.as_str()) {
                                Some(provider_attempt_id) => match sqlx::query("SELECT provider_intent_id,amount_cents FROM payment_attempts WHERE provider_attempt_id=$1").bind(provider_attempt_id).fetch_optional(&worker_pool).await {
                                    Ok(Some(row)) => {
                                        let intent_id: String = row.get("provider_intent_id");
                                        let amount: i64 = row.get("amount_cents");
                                        match provider.ensure_intent(job.id, intent_id.clone(), amount, callback_url.clone()).await {
                                            Ok(()) => match provider.capture(job.id, &intent_id, provider_attempt_id.to_owned(), job.payload.get("scenario").and_then(|value| value.as_str()).unwrap_or("success").to_owned()).await {
                                                Ok(response) => payments::record_attempt_outcome(&worker_pool, provider_attempt_id, &response.outcome).await.map_err(|error| error.to_string()),
                                                Err(error) => Err(error.to_string()),
                                            },
                                            Err(error) => Err(error.to_string()),
                                        }
                                    },
                                    Ok(None) => Ok(()),
                                    Err(error) => Err(error.to_string()),
                                },
                                None => Err("missing_provider_attempt_id".into()),
                            },
                            _ => Ok(()),
                        };
                        if result.is_ok() {
                            let _ = jobs::acknowledge(&worker_pool, &job).await;
                        } else {
                            let _ = jobs::retry(&worker_pool, &job, "retryable_job_error").await;
                        }
                    }
                }
                Err(error) => eprintln!("job claim failed: {error}"),
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    });
    serve_health(
        "worker",
        listen_address("WORKER_LISTEN_ADDR", "127.0.0.1:8082"),
        pool,
    )
    .await?;
    Ok(())
}
