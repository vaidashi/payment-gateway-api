//! PostgreSQL race coverage is enabled with TEST_DATABASE_URL pointing at a disposable database.
//! It intentionally does not substitute SQLite: row locks and unique constraints are the proof.

use food_ordering_runtime::{
    catalog::{QuoteLine, QuoteRequest},
    orders::{self, CreateOrderRequest},
};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

async fn pool() -> Option<PgPool> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    assert!(url.starts_with("postgres"));
    Some(
        PgPoolOptions::new()
            .max_connections(8)
            .connect(&url)
            .await
            .unwrap(),
    )
}

async fn fixture(pool: &PgPool) -> (Uuid, CreateOrderRequest) {
    sqlx::query(include_str!("../migrations/0000_runtime.sql"))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(include_str!("../migrations/0001_catalog_sessions.sql"))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(include_str!("../migrations/0002_orders_commands.sql"))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("TRUNCATE command_identities,order_line_options,order_lines,orders,menu_options,option_groups,menu_items,staff_restaurants,sessions,restaurants,users CASCADE").execute(pool).await.unwrap();
    let customer = Uuid::new_v4();
    let restaurant = Uuid::new_v4();
    let item = Uuid::new_v4();
    sqlx::query("INSERT INTO users(id,display_name,role) VALUES($1,'Customer','customer')")
        .bind(customer)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO restaurants(id,name,tax_basis_points,service_fee_basis_points) VALUES($1,'R',800,0)").bind(restaurant).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO menu_items(id,restaurant_id,name,price_cents) VALUES($1,$2,'Immutable item',1000)").bind(item).bind(restaurant).execute(pool).await.unwrap();
    let quote = QuoteRequest {
        restaurant_id: restaurant.to_string(),
        lines: vec![QuoteLine {
            item_id: item.to_string(),
            quantity: 1,
            selections: vec![],
            extras: vec![],
        }],
    };
    let priced = food_ordering_runtime::catalog::quote(
        &quote,
        &[food_ordering_runtime::catalog::Item {
            id: item.to_string(),
            restaurant_id: restaurant.to_string(),
            price_cents: 1000,
            available: true,
            groups: vec![],
        }],
        800,
        Some(0),
        1,
    )
    .unwrap();
    (
        customer,
        CreateOrderRequest {
            quote,
            quote_digest: priced.quote_digest,
            expected_total_cents: priced.totals.total_cents,
        },
    )
}

#[tokio::test]
async fn concurrent_same_key_creates_one_order_and_replays_original_response() {
    let Some(pool) = pool().await else {
        return;
    };
    let (customer, request) = fixture(&pool).await;
    let key = Uuid::new_v4();
    let (left, right) = tokio::join!(
        orders::create(&pool, customer, key, request.clone()),
        orders::create(&pool, customer, key, request)
    );
    let first = left.unwrap();
    let second = right.unwrap();
    assert_eq!(first.1.id, second.1.id);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM orders")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}
