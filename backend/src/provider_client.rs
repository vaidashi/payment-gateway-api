//! The application only reaches provider-owned state through this HTTP adapter.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone)]
pub struct ProviderClient {
    base_url: String,
    service_token: String,
    client: reqwest::Client,
}

#[derive(Debug, Deserialize)]
pub struct ProviderIntent {
    pub id: String,
    pub amount_cents: i64,
    pub currency: String,
    pub capture_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AttemptResponse {
    pub intent_id: String,
    pub attempt_id: String,
    pub outcome: String,
    pub capture_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct IntentRequest {
    intent_id: String,
    amount_cents: i64,
    currency: String,
    callback_url: String,
}
#[derive(Debug, Serialize)]
struct AttemptRequest {
    attempt_id: String,
    scenario: String,
}
#[derive(Debug, Serialize)]
struct RefundRequest {
    amount_cents: i64,
    currency: String,
}

impl ProviderClient {
    pub fn from_env() -> Result<Self, std::env::VarError> {
        Ok(Self {
            base_url: std::env::var("PROVIDER_BASE_URL")?,
            service_token: std::env::var("PROVIDER_SERVICE_TOKEN")?,
            client: reqwest::Client::new(),
        })
    }
    fn request(&self, method: reqwest::Method, path: String) -> reqwest::RequestBuilder {
        self.client
            .request(
                method,
                format!("{}{}", self.base_url.trim_end_matches('/'), path),
            )
            .header("X-Provider-Service-Token", &self.service_token)
    }
    pub async fn read_intent(&self, intent_id: &str) -> Result<ProviderIntent, reqwest::Error> {
        self.request(
            reqwest::Method::GET,
            format!("/v1/payment_intents/{intent_id}"),
        )
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
    }
    pub async fn ensure_intent(
        &self,
        key: Uuid,
        intent_id: String,
        amount_cents: i64,
        callback_url: String,
    ) -> Result<(), reqwest::Error> {
        self.request(reqwest::Method::POST, "/v1/payment_intents".into())
            .header("Idempotency-Key", key.to_string())
            .json(&IntentRequest {
                intent_id,
                amount_cents,
                currency: "usd".into(),
                callback_url,
            })
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }
    pub async fn capture(
        &self,
        key: Uuid,
        intent_id: &str,
        attempt_id: String,
        scenario: String,
    ) -> Result<AttemptResponse, reqwest::Error> {
        self.request(
            reqwest::Method::POST,
            format!("/v1/payment_intents/{intent_id}/attempts"),
        )
        .header("Idempotency-Key", key.to_string())
        .json(&AttemptRequest {
            attempt_id,
            scenario,
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
    }
    pub async fn refund(
        &self,
        key: Uuid,
        intent_id: &str,
        amount_cents: i64,
    ) -> Result<serde_json::Value, reqwest::Error> {
        self.request(
            reqwest::Method::POST,
            format!("/v1/payment_intents/{intent_id}/refunds"),
        )
        .header("Idempotency-Key", key.to_string())
        .json(&RefundRequest {
            amount_cents,
            currency: "usd".into(),
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
    }
}
