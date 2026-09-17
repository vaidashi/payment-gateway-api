use crate::{catalog::QuoteRequest, money::Totals};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OrderStatus {
    Placed,
    Paid,
    InProgress,
    Ready,
    Completed,
    Cancelled,
}

impl OrderStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Placed => "PLACED",
            Self::Paid => "PAID",
            Self::InProgress => "IN_PROGRESS",
            Self::Ready => "READY",
            Self::Completed => "COMPLETED",
            Self::Cancelled => "CANCELLED",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "PLACED" => Some(Self::Placed),
            "PAID" => Some(Self::Paid),
            "IN_PROGRESS" => Some(Self::InProgress),
            "READY" => Some(Self::Ready),
            "COMPLETED" => Some(Self::Completed),
            "CANCELLED" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionError {
    InvalidTransition,
    PaymentIntegrationOnly,
}

pub fn next_status(
    current: OrderStatus,
    requested: OrderStatus,
) -> Result<OrderStatus, TransitionError> {
    match (current, requested) {
        (OrderStatus::Placed, OrderStatus::Paid) => Err(TransitionError::PaymentIntegrationOnly),
        (OrderStatus::Placed | OrderStatus::Paid, OrderStatus::Cancelled)
        | (OrderStatus::Paid, OrderStatus::InProgress)
        | (OrderStatus::InProgress, OrderStatus::Ready)
        | (OrderStatus::Ready, OrderStatus::Completed) => Ok(requested),
        _ => Err(TransitionError::InvalidTransition),
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CreateOrderRequest {
    #[serde(flatten)]
    pub quote: QuoteRequest,
    pub quote_digest: String,
    pub expected_total_cents: i64,
}
#[derive(Debug, Deserialize)]
pub struct VersionedCommand {
    pub expected_version: i64,
}
#[derive(Debug, Deserialize)]
pub struct StatusCommand {
    pub expected_version: i64,
    pub status: OrderStatus,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OrderResponse {
    pub id: Uuid,
    pub restaurant_id: Uuid,
    pub status: OrderStatus,
    pub version: i64,
    pub totals: Totals,
    pub configuration_version: i64,
}
#[derive(Debug, Serialize)]
pub struct OrderLineResponse {
    pub item_name: String,
    pub unit_price_cents: i64,
    pub quantity: i32,
    pub line_total_cents: i64,
    pub options: Vec<OrderOptionResponse>,
}
#[derive(Debug, Serialize)]
pub struct OrderOptionResponse {
    pub option_name: String,
    pub price_adjustment_cents: i64,
    pub kind: String,
}
#[derive(Debug, Serialize)]
pub struct OrderDetailResponse {
    #[serde(flatten)]
    pub order: OrderResponse,
    pub lines: Vec<OrderLineResponse>,
}

#[derive(Debug)]
pub enum OrderError {
    NotFound,
    Forbidden,
    Conflict(&'static str),
    Invalid(&'static str),
    Database(sqlx::Error),
}
impl From<sqlx::Error> for OrderError {
    fn from(value: sqlx::Error) -> Self {
        Self::Database(value)
    }
}

pub fn canonical_digest<T: Serialize>(payload: &T) -> String {
    let bytes = serde_json::to_vec(payload).expect("command payload must serialize");
    format!("{:x}", Sha256::digest(bytes))
}

pub async fn create(
    pool: &PgPool,
    actor_id: Uuid,
    key: Uuid,
    request: CreateOrderRequest,
) -> Result<(u16, OrderResponse), OrderError> {
    let digest = canonical_digest(&request);
    if let Some(replay) = command(pool, actor_id, "order.create", key, &digest).await? {
        return Ok(replay);
    }
    let mut tx = pool.begin().await?;
    let restaurant_id = Uuid::parse_str(&request.quote.restaurant_id)
        .map_err(|_| OrderError::Invalid("restaurant_id"))?;
    let config = sqlx::query_as::<_, (i32, Option<i32>, i64)>("SELECT tax_basis_points,service_fee_basis_points,configuration_version FROM restaurants WHERE id=$1 FOR SHARE")
        .bind(restaurant_id).fetch_optional(&mut *tx).await?.ok_or(OrderError::NotFound)?;
    let snapshot = snapshot(&mut tx, &request.quote, restaurant_id).await?;
    let totals = crate::money::totals(snapshot.subtotal, config.0, config.1)
        .map_err(|_| OrderError::Invalid("amount"))?;
    let quote = crate::catalog::quote(
        &request.quote,
        &snapshot.items,
        config.0,
        config.1,
        config.2,
    )
    .map_err(|_| OrderError::Invalid("quote"))?;
    if quote.quote_digest != request.quote_digest
        || totals.total_cents != request.expected_total_cents
    {
        return Err(OrderError::Conflict("quote_changed"));
    }
    let id = Uuid::new_v4();
    let fee = config.1.unwrap_or(0);
    sqlx::query("INSERT INTO orders(id,customer_id,restaurant_id,status,configuration_version,tax_basis_points,service_fee_basis_points,subtotal_cents,tax_cents,service_fee_cents,total_cents) VALUES($1,$2,$3,'PLACED',$4,$5,$6,$7,$8,$9,$10)")
        .bind(id).bind(actor_id).bind(restaurant_id).bind(config.2).bind(config.0).bind(fee).bind(totals.subtotal_cents).bind(totals.tax_cents).bind(totals.service_fee_cents).bind(totals.total_cents).execute(&mut *tx).await?;
    for line in snapshot.lines {
        persist_line(&mut tx, id, line).await?;
    }
    let response = OrderResponse {
        id,
        restaurant_id,
        status: OrderStatus::Placed,
        version: 1,
        totals,
        configuration_version: config.2,
    };
    let body = serde_json::to_value(&response).expect("order response serializes");
    let inserted = sqlx::query("INSERT INTO command_identities(actor_id,operation,idempotency_key,input_digest,resource_id,response_status,response_body) VALUES($1,'order.create',$2,$3,$4,201,$5) ON CONFLICT DO NOTHING")
        .bind(actor_id).bind(key).bind(&digest).bind(id).bind(&body).execute(&mut *tx).await?;
    if inserted.rows_affected() == 0 {
        tx.rollback().await?;
        return command(pool, actor_id, "order.create", key, &digest)
            .await?
            .ok_or(OrderError::Conflict("command_in_progress"));
    }
    tx.commit().await?;
    Ok((201, response))
}

pub async fn transition(
    pool: &PgPool,
    actor_id: Uuid,
    role: &str,
    order_id: Uuid,
    key: Uuid,
    requested: OrderStatus,
    expected_version: i64,
) -> Result<(u16, OrderResponse), OrderError> {
    let operation = if requested == OrderStatus::Cancelled {
        "order.cancel"
    } else {
        "order.status"
    };
    let digest = canonical_digest(&(order_id, requested, expected_version));
    authorize_transition(pool, actor_id, role, order_id, requested).await?;
    if let Some(replay) = command(pool, actor_id, operation, key, &digest).await? {
        return Ok(replay);
    }
    let mut tx = pool.begin().await?;
    let row = sqlx::query_as::<_, (Uuid, Uuid, String, i64, i64, i64, i64, i64, i64)>("SELECT customer_id,restaurant_id,status,version,configuration_version,subtotal_cents,tax_cents,service_fee_cents,total_cents FROM orders WHERE id=$1 FOR UPDATE")
        .bind(order_id).fetch_optional(&mut *tx).await?.ok_or(OrderError::NotFound)?;
    let allowed = match role { "customer" => requested == OrderStatus::Cancelled && row.0 == actor_id, "staff" => requested != OrderStatus::Cancelled && sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM staff_restaurants WHERE user_id=$1 AND restaurant_id=$2)").bind(actor_id).bind(row.1).fetch_one(&mut *tx).await?, _ => false };
    if !allowed {
        return Err(OrderError::Forbidden);
    }
    if row.3 != expected_version {
        return Err(OrderError::Conflict("stale_version"));
    }
    let current = OrderStatus::parse(&row.2).ok_or(OrderError::Database(sqlx::Error::Protocol(
        "invalid status".into(),
    )))?;
    next_status(current, requested).map_err(|e| match e {
        TransitionError::PaymentIntegrationOnly => OrderError::Forbidden,
        TransitionError::InvalidTransition => OrderError::Conflict("invalid_transition"),
    })?;
    let next_version = row.3 + 1;
    let column = match requested {
        OrderStatus::Cancelled => "cancelled_at",
        OrderStatus::InProgress => "in_progress_at",
        OrderStatus::Ready => "ready_at",
        OrderStatus::Completed => "completed_at",
        _ => unreachable!(),
    };
    match column {
        "cancelled_at" => {
            sqlx::query("UPDATE orders SET status=$1,version=$2,cancelled_at=now() WHERE id=$3")
                .bind(requested.as_str())
                .bind(next_version)
                .bind(order_id)
                .execute(&mut *tx)
                .await?
        }
        "in_progress_at" => {
            sqlx::query("UPDATE orders SET status=$1,version=$2,in_progress_at=now() WHERE id=$3")
                .bind(requested.as_str())
                .bind(next_version)
                .bind(order_id)
                .execute(&mut *tx)
                .await?
        }
        "ready_at" => {
            sqlx::query("UPDATE orders SET status=$1,version=$2,ready_at=now() WHERE id=$3")
                .bind(requested.as_str())
                .bind(next_version)
                .bind(order_id)
                .execute(&mut *tx)
                .await?
        }
        "completed_at" => {
            sqlx::query("UPDATE orders SET status=$1,version=$2,completed_at=now() WHERE id=$3")
                .bind(requested.as_str())
                .bind(next_version)
                .bind(order_id)
                .execute(&mut *tx)
                .await?
        }
        _ => unreachable!(),
    };
    let response = OrderResponse {
        id: order_id,
        restaurant_id: row.1,
        status: requested,
        version: next_version,
        totals: Totals {
            subtotal_cents: row.5,
            tax_cents: row.6,
            service_fee_cents: row.7,
            total_cents: row.8,
        },
        configuration_version: row.4,
    };
    let body = serde_json::to_value(&response).expect("order response serializes");
    let inserted = sqlx::query("INSERT INTO command_identities(actor_id,operation,idempotency_key,input_digest,resource_id,response_status,response_body) VALUES($1,$2,$3,$4,$5,200,$6) ON CONFLICT DO NOTHING")
        .bind(actor_id).bind(operation).bind(key).bind(&digest).bind(order_id).bind(&body).execute(&mut *tx).await?;
    if inserted.rows_affected() == 0 {
        tx.rollback().await?;
        return command(pool, actor_id, operation, key, &digest)
            .await?
            .ok_or(OrderError::Conflict("command_in_progress"));
    }
    tx.commit().await?;
    Ok((200, response))
}

/// Internal-only U5 seam: verified provider processing, never an HTTP actor command.
pub async fn confirm_paid(pool: &PgPool, order_id: Uuid) -> Result<OrderResponse, OrderError> {
    let mut tx = pool.begin().await?;
    let response = locked_order_response(&mut tx, order_id).await?;
    if response.status != OrderStatus::Placed {
        return Err(OrderError::Conflict("invalid_payment_transition"));
    }
    sqlx::query("UPDATE orders SET status='PAID',version=version+1,paid_at=now() WHERE id=$1")
        .bind(order_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(OrderResponse {
        status: OrderStatus::Paid,
        version: response.version + 1,
        ..response
    })
}

pub async fn command(
    pool: &PgPool,
    actor_id: Uuid,
    operation: &str,
    key: Uuid,
    digest: &str,
) -> Result<Option<(u16, OrderResponse)>, OrderError> {
    let row = sqlx::query_as::<_, (String, i32, serde_json::Value)>("SELECT input_digest,response_status,response_body FROM command_identities WHERE actor_id=$1 AND operation=$2 AND idempotency_key=$3")
        .bind(actor_id).bind(operation).bind(key).fetch_optional(pool).await?;
    match row {
        None => Ok(None),
        Some((stored, _, _)) if stored != digest => {
            Err(OrderError::Conflict("idempotency_key_reused"))
        }
        Some((_, status, body)) => Ok(Some((
            status as u16,
            serde_json::from_value(body).map_err(|_| OrderError::Invalid("stored_response"))?,
        ))),
    }
}

pub async fn read(
    pool: &PgPool,
    actor_id: Uuid,
    role: &str,
    order_id: Uuid,
) -> Result<OrderDetailResponse, OrderError> {
    let row = sqlx::query_as::<_, (Uuid, Uuid, String, i64, i64, i64, i64, i64, i64)>("SELECT customer_id,restaurant_id,status,version,configuration_version,subtotal_cents,tax_cents,service_fee_cents,total_cents FROM orders WHERE id=$1")
        .bind(order_id).fetch_optional(pool).await?.ok_or(OrderError::NotFound)?;
    let allowed = role == "admin" || (role == "customer" && row.0 == actor_id) || (role == "staff" && sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM staff_restaurants WHERE user_id=$1 AND restaurant_id=$2)").bind(actor_id).bind(row.1).fetch_one(pool).await?);
    if !allowed {
        return Err(OrderError::NotFound);
    }
    let order = OrderResponse {
        id: order_id,
        restaurant_id: row.1,
        status: OrderStatus::parse(&row.2).ok_or(OrderError::Invalid("status"))?,
        version: row.3,
        configuration_version: row.4,
        totals: Totals {
            subtotal_cents: row.5,
            tax_cents: row.6,
            service_fee_cents: row.7,
            total_cents: row.8,
        },
    };
    let line_rows = sqlx::query_as::<_, (Uuid, String, i64, i32, i64)>("SELECT id,item_name,unit_price_cents,quantity,line_total_cents FROM order_lines WHERE order_id=$1 ORDER BY id").bind(order_id).fetch_all(pool).await?;
    let mut lines = Vec::new();
    for (line_id, item_name, unit_price_cents, quantity, line_total_cents) in line_rows {
        let options = sqlx::query_as::<_, (String, i64, String)>("SELECT option_name,price_adjustment_cents,kind FROM order_line_options WHERE order_line_id=$1 ORDER BY id").bind(line_id).fetch_all(pool).await?.into_iter().map(|(option_name, price_adjustment_cents, kind)| OrderOptionResponse { option_name, price_adjustment_cents, kind }).collect();
        lines.push(OrderLineResponse {
            item_name,
            unit_price_cents,
            quantity,
            line_total_cents,
            options,
        });
    }
    Ok(OrderDetailResponse { order, lines })
}

async fn authorize_transition(
    pool: &PgPool,
    actor_id: Uuid,
    role: &str,
    order_id: Uuid,
    requested: OrderStatus,
) -> Result<(), OrderError> {
    let row = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT customer_id,restaurant_id FROM orders WHERE id=$1",
    )
    .bind(order_id)
    .fetch_optional(pool)
    .await?
    .ok_or(OrderError::NotFound)?;
    let allowed = match role { "customer" => requested == OrderStatus::Cancelled && row.0 == actor_id, "staff" => requested != OrderStatus::Cancelled && sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM staff_restaurants WHERE user_id=$1 AND restaurant_id=$2)").bind(actor_id).bind(row.1).fetch_one(pool).await?, _ => false };
    if allowed {
        Ok(())
    } else {
        Err(OrderError::Forbidden)
    }
}
async fn locked_order_response(
    tx: &mut Transaction<'_, Postgres>,
    order_id: Uuid,
) -> Result<OrderResponse, OrderError> {
    let row = sqlx::query_as::<_, (Uuid, String, i64, i64, i64, i64, i64, i64, i64)>("SELECT restaurant_id,status,version,configuration_version,subtotal_cents,tax_cents,service_fee_cents,total_cents,customer_id FROM orders WHERE id=$1 FOR UPDATE").bind(order_id).fetch_optional(&mut **tx).await?.ok_or(OrderError::NotFound)?;
    Ok(OrderResponse {
        id: order_id,
        restaurant_id: row.0,
        status: OrderStatus::parse(&row.1).ok_or(OrderError::Invalid("status"))?,
        version: row.2,
        configuration_version: row.3,
        totals: Totals {
            subtotal_cents: row.4,
            tax_cents: row.5,
            service_fee_cents: row.6,
            total_cents: row.7,
        },
    })
}

pub async fn list(
    pool: &PgPool,
    actor_id: Uuid,
    role: &str,
) -> Result<Vec<OrderResponse>, OrderError> {
    let rows = match role {
        "customer" => sqlx::query_as::<_, (Uuid, Uuid, String, i64, i64, i64, i64, i64, i64)>("SELECT id,restaurant_id,status,version,configuration_version,subtotal_cents,tax_cents,service_fee_cents,total_cents FROM orders WHERE customer_id=$1 ORDER BY placed_at DESC,id DESC LIMIT 50").bind(actor_id).fetch_all(pool).await?,
        "staff" => sqlx::query_as::<_, (Uuid, Uuid, String, i64, i64, i64, i64, i64, i64)>("SELECT o.id,o.restaurant_id,o.status,o.version,o.configuration_version,o.subtotal_cents,o.tax_cents,o.service_fee_cents,o.total_cents FROM orders o JOIN staff_restaurants sr ON sr.restaurant_id=o.restaurant_id WHERE sr.user_id=$1 AND o.status IN ('PAID','IN_PROGRESS','READY') ORDER BY o.placed_at,o.id LIMIT 50").bind(actor_id).fetch_all(pool).await?,
        "admin" => sqlx::query_as::<_, (Uuid, Uuid, String, i64, i64, i64, i64, i64, i64)>("SELECT id,restaurant_id,status,version,configuration_version,subtotal_cents,tax_cents,service_fee_cents,total_cents FROM orders ORDER BY placed_at DESC,id DESC LIMIT 50").fetch_all(pool).await?,
        _ => return Err(OrderError::Forbidden),
    };
    rows.into_iter()
        .map(|row| {
            Ok(OrderResponse {
                id: row.0,
                restaurant_id: row.1,
                status: OrderStatus::parse(&row.2).ok_or(OrderError::Invalid("status"))?,
                version: row.3,
                configuration_version: row.4,
                totals: Totals {
                    subtotal_cents: row.5,
                    tax_cents: row.6,
                    service_fee_cents: row.7,
                    total_cents: row.8,
                },
            })
        })
        .collect()
}

struct Snapshot {
    items: Vec<crate::catalog::Item>,
    lines: Vec<SnapshotLine>,
    subtotal: i64,
}
struct SnapshotLine {
    item_id: Uuid,
    name: String,
    unit: i64,
    quantity: i32,
    options: Vec<(Uuid, String, i64, &'static str)>,
}
async fn snapshot(
    tx: &mut Transaction<'_, Postgres>,
    request: &QuoteRequest,
    restaurant_id: Uuid,
) -> Result<Snapshot, OrderError> {
    let mut ids: Vec<Uuid> = request
        .lines
        .iter()
        .map(|line| Uuid::parse_str(&line.item_id).map_err(|_| OrderError::Invalid("item_id")))
        .collect::<Result<_, _>>()?;
    ids.sort();
    ids.dedup();
    let mut database_items = std::collections::HashMap::new();
    for id in ids {
        let row = sqlx::query_as::<_, (Uuid, String, i64, bool)>(
            "SELECT restaurant_id,name,price_cents,available FROM menu_items WHERE id=$1 FOR SHARE",
        )
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(OrderError::Invalid("item"))?;
        database_items.insert(id, row);
    }
    let mut items = Vec::new();
    let mut lines = Vec::new();
    let mut subtotal = 0_i64;
    for input in &request.lines {
        let id = Uuid::parse_str(&input.item_id).map_err(|_| OrderError::Invalid("item_id"))?;
        let (owner, name, price, available) = database_items
            .get(&id)
            .cloned()
            .ok_or(OrderError::Invalid("item"))?;
        if owner != restaurant_id {
            return Err(OrderError::Invalid("foreign_item"));
        }
        let groups = load_groups(tx, id).await?;
        let catalog_groups = groups
            .iter()
            .map(|group| crate::catalog::Group {
                required: group.0,
                options: group
                    .1
                    .iter()
                    .map(|(id, _, adjustment)| crate::catalog::OptionDef {
                        id: id.to_string(),
                        adjustment_cents: *adjustment,
                    })
                    .collect(),
            })
            .collect();
        items.push(crate::catalog::Item {
            id: input.item_id.clone(),
            restaurant_id: owner.to_string(),
            price_cents: price,
            available,
            groups: catalog_groups,
        });
        let selected: std::collections::HashSet<_> = input
            .selections
            .iter()
            .filter_map(|value| Uuid::parse_str(value).ok())
            .collect();
        let extras: std::collections::HashSet<_> = input
            .extras
            .iter()
            .filter_map(|value| Uuid::parse_str(value).ok())
            .collect();
        let mut unit = price;
        let mut options = Vec::new();
        for (required, choices) in &groups {
            for (option_id, option_name, adjustment) in choices {
                if selected.contains(option_id) || extras.contains(option_id) {
                    unit = unit
                        .checked_add(*adjustment)
                        .ok_or(OrderError::Invalid("amount"))?;
                    options.push((
                        *option_id,
                        option_name.clone(),
                        *adjustment,
                        if *required { "selection" } else { "extra" },
                    ));
                }
            }
        }
        let line_total = unit
            .checked_mul(input.quantity as i64)
            .ok_or(OrderError::Invalid("amount"))?;
        subtotal = subtotal
            .checked_add(line_total)
            .ok_or(OrderError::Invalid("amount"))?;
        lines.push(SnapshotLine {
            item_id: id,
            name,
            unit,
            quantity: input.quantity,
            options,
        });
    }
    Ok(Snapshot {
        items,
        lines,
        subtotal,
    })
}
async fn load_groups(
    tx: &mut Transaction<'_, Postgres>,
    item_id: Uuid,
) -> Result<Vec<(bool, Vec<(Uuid, String, i64)>)>, OrderError> {
    let groups = sqlx::query_as::<_, (Uuid, bool)>(
        "SELECT id,required FROM option_groups WHERE menu_item_id=$1 ORDER BY position FOR SHARE",
    )
    .bind(item_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut result = Vec::new();
    for (id, required) in groups {
        result.push((required, sqlx::query_as::<_, (Uuid, String, i64)>("SELECT id,name,price_adjustment_cents FROM menu_options WHERE option_group_id=$1 ORDER BY position FOR SHARE").bind(id).fetch_all(&mut **tx).await?));
    }
    Ok(result)
}
async fn persist_line(
    tx: &mut Transaction<'_, Postgres>,
    order_id: Uuid,
    line: SnapshotLine,
) -> Result<(), OrderError> {
    let id = Uuid::new_v4();
    let total = line
        .unit
        .checked_mul(line.quantity as i64)
        .ok_or(OrderError::Invalid("amount"))?;
    sqlx::query("INSERT INTO order_lines(id,order_id,menu_item_id,item_name,unit_price_cents,quantity,line_total_cents) VALUES($1,$2,$3,$4,$5,$6,$7)").bind(id).bind(order_id).bind(line.item_id).bind(line.name).bind(line.unit).bind(line.quantity).bind(total).execute(&mut **tx).await?;
    for (option_id, name, adjustment, kind) in line.options {
        sqlx::query("INSERT INTO order_line_options(id,order_line_id,menu_option_id,option_name,price_adjustment_cents,kind) VALUES($1,$2,$3,$4,$5,$6)").bind(Uuid::new_v4()).bind(id).bind(option_id).bind(name).bind(adjustment).bind(kind).execute(&mut **tx).await?;
    }
    Ok(())
}
