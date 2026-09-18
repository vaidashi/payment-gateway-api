//! Provider contract coverage uses PostgreSQL when TEST_PROVIDER_DATABASE_URL is supplied.
//! It intentionally does not substitute SQLite because provider uniqueness is the contract.

use food_ordering_runtime::mock_provider::{
    self, AttemptRequest, IntentRequest, ProviderConfig, ProviderState, RefundRequest, Scenario,
};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

async fn pool() -> Option<PgPool> {
    let url = std::env::var("TEST_PROVIDER_DATABASE_URL").ok()?;
    assert!(url.starts_with("postgres"));
    Some(
        PgPoolOptions::new()
            .max_connections(8)
            .connect(&url)
            .await
            .unwrap(),
    )
}

async fn reset(pool: &PgPool) {
    sqlx::query(include_str!("../provider_migrations/0000_runtime.sql"))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(include_str!("../provider_migrations/0001_provider.sql"))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("TRUNCATE provider_callback_deliveries,provider_callback_events,provider_refunds,provider_captures,provider_attempts,provider_idempotency_keys,provider_intents CASCADE").execute(pool).await.unwrap();
}

fn intent(id: &str) -> IntentRequest {
    IntentRequest {
        intent_id: id.into(),
        amount_cents: 2220,
        currency: "usd".into(),
        callback_url: "http://api:8080/webhooks/mock-payments".into(),
    }
}

#[test]
fn provider_contract_has_an_independent_entry_point() {
    let request = IntentRequest {
        intent_id: "pi_contract".into(),
        amount_cents: 2220,
        currency: "usd".into(),
        callback_url: "http://api:8080/webhooks/mock-payments".into(),
    };
    assert_eq!(request.amount_cents, 2220);
    assert_eq!(Scenario::Success.as_str(), "success");
}

#[actix_web::test]
async fn provider_http_boundary_rejects_missing_service_token() {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://unused:unused@localhost/unused")
        .unwrap();
    let app = actix_web::test::init_service(
        actix_web::App::new()
            .app_data(actix_web::web::Data::new(ProviderState {
                pool,
                config: ProviderConfig {
                    service_token: "test-token".into(),
                    callback_url: "http://api:8080/webhooks/mock-payments".into(),
                    callback_signing_secret: "test-signing-secret".into(),
                },
                callback_client: reqwest::Client::new(),
            }))
            .configure(mock_provider::configure),
    )
    .await;
    let response = actix_web::test::call_service(
        &app,
        actix_web::test::TestRequest::post()
            .uri("/v1/payment_intents")
            .set_json(intent("pi_auth"))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), actix_web::http::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn concurrent_confirmation_and_duplicate_callback_create_one_capture() {
    let Some(pool) = pool().await else {
        return;
    };
    reset(&pool).await;
    mock_provider::create_intent(
        &pool,
        Uuid::new_v4(),
        intent("pi_concurrent"),
        "http://api:8080/webhooks/mock-payments",
    )
    .await
    .unwrap();
    let (left, right) = tokio::join!(
        mock_provider::confirm_attempt(
            &pool,
            Uuid::new_v4(),
            "pi_concurrent",
            AttemptRequest {
                attempt_id: "att_one".into(),
                scenario: Scenario::DuplicateCallback
            }
        ),
        mock_provider::confirm_attempt(
            &pool,
            Uuid::new_v4(),
            "pi_concurrent",
            AttemptRequest {
                attempt_id: "att_two".into(),
                scenario: Scenario::Success
            }
        )
    );
    assert!(left.is_ok() || right.is_ok());
    let captures: i64 = sqlx::query_scalar("SELECT count(*) FROM provider_captures")
        .fetch_one(&pool)
        .await
        .unwrap();
    let deliveries: i64 = sqlx::query_scalar("SELECT count(*) FROM provider_callback_deliveries")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(captures, 1);
    assert!(deliveries == 1 || deliveries == 3);
}

#[tokio::test]
async fn decline_is_terminal_but_a_distinct_attempt_can_succeed() {
    let Some(pool) = pool().await else {
        return;
    };
    reset(&pool).await;
    mock_provider::create_intent(
        &pool,
        Uuid::new_v4(),
        intent("pi_retry"),
        "http://api:8080/webhooks/mock-payments",
    )
    .await
    .unwrap();
    let declined = mock_provider::confirm_attempt(
        &pool,
        Uuid::new_v4(),
        "pi_retry",
        AttemptRequest {
            attempt_id: "att_declined".into(),
            scenario: Scenario::Decline,
        },
    )
    .await
    .unwrap();
    assert_eq!(declined.outcome, "declined");
    let succeeded = mock_provider::confirm_attempt(
        &pool,
        Uuid::new_v4(),
        "pi_retry",
        AttemptRequest {
            attempt_id: "att_succeeds".into(),
            scenario: Scenario::Success,
        },
    )
    .await
    .unwrap();
    assert_eq!(succeeded.outcome, "succeeded");
    assert!(succeeded.capture_id.is_some());
}

#[tokio::test]
async fn committed_capture_replays_after_response_loss_and_refunds_once() {
    let Some(pool) = pool().await else {
        return;
    };
    reset(&pool).await;
    mock_provider::create_intent(
        &pool,
        Uuid::new_v4(),
        intent("pi_replay"),
        "http://api:8080/webhooks/mock-payments",
    )
    .await
    .unwrap();
    let key = Uuid::new_v4();
    let first = mock_provider::confirm_attempt(
        &pool,
        key,
        "pi_replay",
        AttemptRequest {
            attempt_id: "att_replay".into(),
            scenario: Scenario::Success,
        },
    )
    .await
    .unwrap();
    let replay = mock_provider::confirm_attempt(
        &pool,
        key,
        "pi_replay",
        AttemptRequest {
            attempt_id: "att_replay".into(),
            scenario: Scenario::Success,
        },
    )
    .await
    .unwrap();
    assert_eq!(first, replay);
    let refund_key = Uuid::new_v4();
    let request = RefundRequest {
        amount_cents: 2220,
        currency: "usd".into(),
    };
    let one = mock_provider::refund(&pool, refund_key, "pi_replay", request.clone())
        .await
        .unwrap();
    let two = mock_provider::refund(&pool, refund_key, "pi_replay", request)
        .await
        .unwrap();
    assert_eq!(one, two);
    let refunds: i64 = sqlx::query_scalar("SELECT count(*) FROM provider_refunds")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(refunds, 1);
}

#[tokio::test]
async fn rejects_inconsistent_money_and_idempotency_inputs() {
    let Some(pool) = pool().await else {
        return;
    };
    reset(&pool).await;
    let key = Uuid::new_v4();
    mock_provider::create_intent(
        &pool,
        key,
        intent("pi_validation"),
        "http://api:8080/webhooks/mock-payments",
    )
    .await
    .unwrap();
    let error = mock_provider::create_intent(
        &pool,
        key,
        IntentRequest {
            amount_cents: 1,
            ..intent("pi_validation")
        },
        "http://api:8080/webhooks/mock-payments",
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        mock_provider::ProviderError::Conflict("idempotency_input_mismatch")
    ));
    let error = mock_provider::create_intent(
        &pool,
        Uuid::new_v4(),
        IntentRequest {
            currency: "eur".into(),
            ..intent("pi_other")
        },
        "http://api:8080/webhooks/mock-payments",
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        mock_provider::ProviderError::Invalid("invalid_currency")
    ));
}
