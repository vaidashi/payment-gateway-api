//! App-side monetary transitions. Provider HTTP is deliberately outside these transactions.

use crate::{orders::OrderStatus, provider_client::ProviderClient};
use serde::Deserialize;
use serde_json::json;
use sqlx::{PgPool, Row};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureEffect {
    MarkPaid,
    RefundOnly,
    Noop,
}
#[derive(Deserialize)]
struct Callback {
    data: CallbackData,
}
#[derive(Deserialize)]
struct CallbackData {
    intent_id: String,
    attempt_id: String,
    capture_id: String,
    amount_cents: i64,
    currency: String,
}

pub async fn process_inbox(
    pool: &PgPool,
    provider: &ProviderClient,
    event_id: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let body: Option<Vec<u8>> = sqlx::query_scalar("SELECT raw_body FROM payment_inbox WHERE event_id=$1 AND processed_at IS NULL AND quarantined_at IS NULL").bind(event_id).fetch_optional(pool).await?;
    let Some(body) = body else {
        return Ok(());
    };
    let callback: Callback = serde_json::from_slice(&body)?;
    let actual = provider.read_intent(&callback.data.intent_id).await?;
    if actual.id != callback.data.intent_id
        || actual.amount_cents != callback.data.amount_cents
        || actual.currency != callback.data.currency
        || actual.capture_id.as_deref() != Some(callback.data.capture_id.as_str())
    {
        sqlx::query("UPDATE payment_inbox SET quarantined_at=now(),error_code='provider_identity_mismatch' WHERE event_id=$1").bind(event_id).execute(pool).await?;
        return Ok(());
    }
    let local: Option<(String, i64, String)> = sqlx::query_as("SELECT provider_intent_id,amount_cents,currency FROM payment_attempts WHERE provider_attempt_id=$1")
        .bind(&callback.data.attempt_id).fetch_optional(pool).await?;
    if local.as_ref().is_none_or(|(intent, amount, currency)| {
        intent != &callback.data.intent_id
            || amount != &callback.data.amount_cents
            || currency != &callback.data.currency
    }) {
        sqlx::query("UPDATE payment_inbox SET quarantined_at=now(),error_code='local_identity_mismatch' WHERE event_id=$1").bind(event_id).execute(pool).await?;
        return Ok(());
    }
    apply_capture(
        pool,
        &callback.data.attempt_id,
        &callback.data.capture_id,
        callback.data.amount_cents,
        &callback.data.currency,
    )
    .await?;
    sqlx::query(
        "UPDATE payment_inbox SET processed_at=now() WHERE event_id=$1 AND quarantined_at IS NULL",
    )
    .bind(event_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn complete_refund(
    pool: &PgPool,
    capture_id: &str,
    refund_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE refund_obligations SET status='succeeded',provider_refund_id=$2,updated_at=now() WHERE provider_capture_id=$1 AND status IN ('pending','unknown')").bind(capture_id).bind(refund_id).execute(pool).await?;
    Ok(())
}

pub async fn record_attempt_outcome(
    pool: &PgPool,
    provider_attempt_id: &str,
    outcome: &str,
) -> Result<(), sqlx::Error> {
    if outcome == "declined" {
        sqlx::query("UPDATE payment_attempts SET status='declined',updated_at=now() WHERE provider_attempt_id=$1 AND status IN ('pending','unknown')")
            .bind(provider_attempt_id).execute(pool).await?;
    }
    Ok(())
}

pub fn capture_effect(status: OrderStatus) -> CaptureEffect {
    match status {
        OrderStatus::Placed => CaptureEffect::MarkPaid,
        OrderStatus::Cancelled => CaptureEffect::RefundOnly,
        _ => CaptureEffect::Noop,
    }
}

/// Applies a verified authoritative capture. It never reopens a terminal order.
pub async fn apply_capture(
    pool: &PgPool,
    provider_attempt_id: &str,
    provider_capture_id: &str,
    amount_cents: i64,
    currency: &str,
) -> Result<CaptureEffect, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query("SELECT p.order_id,o.status,o.total_cents,p.provider_intent_id FROM payment_attempts p JOIN orders o ON o.id=p.order_id WHERE p.provider_attempt_id=$1 FOR UPDATE")
        .bind(provider_attempt_id).fetch_optional(&mut *tx).await?;
    let Some(row) = row else {
        tx.rollback().await?;
        return Ok(CaptureEffect::Noop);
    };
    let order_id: Uuid = row.get("order_id");
    let total: i64 = row.get("total_cents");
    let intent: String = row.get("provider_intent_id");
    if amount_cents != total || currency != "usd" {
        tx.rollback().await?;
        return Ok(CaptureEffect::Noop);
    }
    let status =
        OrderStatus::parse(&row.get::<String, _>("status")).unwrap_or(OrderStatus::Completed);
    let effect = capture_effect(status);
    sqlx::query("UPDATE payment_attempts SET status='succeeded',provider_capture_id=$2,updated_at=now() WHERE provider_attempt_id=$1")
        .bind(provider_attempt_id).bind(provider_capture_id).execute(&mut *tx).await?;
    match effect {
        CaptureEffect::MarkPaid => {
            sqlx::query("UPDATE orders SET status='PAID',version=version+1,paid_at=now() WHERE id=$1 AND status='PLACED'").bind(order_id).execute(&mut *tx).await?;
        }
        CaptureEffect::RefundOnly => {
            sqlx::query("INSERT INTO refund_obligations(id,order_id,provider_intent_id,provider_capture_id,amount_cents,currency,status) VALUES($1,$2,$3,$4,$5,'usd','pending') ON CONFLICT(provider_capture_id) DO NOTHING")
                .bind(Uuid::new_v4()).bind(order_id).bind(&intent).bind(provider_capture_id).bind(total).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO durable_jobs(id,kind,dedupe_key,payload) VALUES($1,'refund',$2,$3) ON CONFLICT(dedupe_key) DO NOTHING")
                .bind(Uuid::new_v4()).bind(format!("refund:{provider_capture_id}")).bind(json!({"capture_id":provider_capture_id})).execute(&mut *tx).await?;
        }
        CaptureEffect::Noop => {}
    }
    tx.commit().await?;
    Ok(effect)
}

pub async fn queue_capture(
    pool: &PgPool,
    order_id: Uuid,
    attempt_id: Uuid,
    provider_attempt_id: String,
    scenario: String,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query("SELECT status,total_cents FROM orders WHERE id=$1 FOR UPDATE")
        .bind(order_id)
        .fetch_optional(&mut *tx)
        .await?;
    let Some(row) = row else {
        tx.rollback().await?;
        return Ok(false);
    };
    if row.get::<String, _>("status") != "PLACED" {
        tx.rollback().await?;
        return Ok(false);
    }
    // The browser reuses its command UUID after a lost response. Its derived provider-attempt
    // identity therefore replays the original pending command rather than creating work again.
    if sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM payment_attempts WHERE provider_attempt_id=$1",
    )
    .bind(&provider_attempt_id)
    .fetch_one(&mut *tx)
    .await?
        > 0
    {
        tx.commit().await?;
        return Ok(true);
    }
    if sqlx::query_scalar::<_, i64>("SELECT count(*) FROM payment_attempts WHERE order_id=$1 AND status IN ('pending','unknown','succeeded')")
        .bind(order_id).fetch_one(&mut *tx).await? > 0 { tx.rollback().await?; return Ok(false); }
    let intent = format!("pi_{}", order_id.simple());
    sqlx::query("INSERT INTO payment_attempts(id,order_id,provider_intent_id,provider_attempt_id,amount_cents,currency,status) VALUES($1,$2,$3,$4,$5,'usd','pending')")
        .bind(attempt_id).bind(order_id).bind(&intent).bind(&provider_attempt_id).bind(row.get::<i64,_>("total_cents")).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO durable_jobs(id,kind,dedupe_key,payload) VALUES($1,'capture',$2,$3)")
        .bind(Uuid::new_v4()).bind(format!("capture:{provider_attempt_id}")).bind(json!({"order_id":order_id,"provider_attempt_id":provider_attempt_id,"scenario":scenario})).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(true)
}
