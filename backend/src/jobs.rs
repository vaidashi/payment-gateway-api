//! PostgreSQL-backed work leasing.  The lease token fences stale workers.

use serde_json::Value;
use sqlx::{PgPool, Row};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Job {
    pub id: Uuid,
    pub kind: String,
    pub payload: Value,
    pub lease_token: Uuid,
}

pub fn can_acknowledge(owner: Uuid, claim: Uuid) -> bool {
    owner == claim
}

pub async fn enqueue(
    pool: &PgPool,
    kind: &str,
    dedupe_key: String,
    payload: Value,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO durable_jobs(id,kind,dedupe_key,payload) VALUES($1,$2,$3,$4) ON CONFLICT(dedupe_key) DO NOTHING")
        .bind(Uuid::new_v4()).bind(kind).bind(dedupe_key).bind(payload).execute(pool).await?;
    Ok(())
}

pub async fn claim(pool: &PgPool, limit: i64) -> Result<Vec<Job>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let rows = sqlx::query("SELECT id,kind,payload FROM durable_jobs WHERE completed_at IS NULL AND quarantined_at IS NULL AND due_at <= now() AND (lease_expires_at IS NULL OR lease_expires_at < now()) ORDER BY due_at,id LIMIT $1 FOR UPDATE SKIP LOCKED")
        .bind(limit).fetch_all(&mut *tx).await?;
    let mut jobs = Vec::with_capacity(rows.len());
    for row in rows {
        let token = Uuid::new_v4();
        let id = row.get::<Uuid, _>("id");
        sqlx::query("UPDATE durable_jobs SET lease_token=$1,lease_expires_at=now()+interval '30 seconds',attempts=attempts+1 WHERE id=$2")
            .bind(token).bind(id).execute(&mut *tx).await?;
        jobs.push(Job {
            id,
            kind: row.get("kind"),
            payload: row.get("payload"),
            lease_token: token,
        });
    }
    tx.commit().await?;
    Ok(jobs)
}

pub async fn acknowledge(pool: &PgPool, job: &Job) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query("UPDATE durable_jobs SET completed_at=now(),lease_token=NULL,lease_expires_at=NULL WHERE id=$1 AND lease_token=$2 AND completed_at IS NULL")
        .bind(job.id).bind(job.lease_token).execute(pool).await?.rows_affected() == 1)
}

pub async fn retry(pool: &PgPool, job: &Job, error: &str) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query("UPDATE durable_jobs SET due_at=now() + make_interval(secs => LEAST(60, 1 << LEAST(attempts, 6))),last_error=$3,lease_token=NULL,lease_expires_at=NULL WHERE id=$1 AND lease_token=$2 AND completed_at IS NULL")
        .bind(job.id).bind(job.lease_token).bind(error).execute(pool).await?.rows_affected() == 1)
}
