use crate::{
    auth,
    catalog::{self, Group, Item, OptionDef, QuoteRequest},
    seed,
};
use actix_web::{HttpRequest, HttpResponse, web};
use sqlx::PgPool;
use uuid::Uuid;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.route("/health/live", web::get().to(crate::runtime::live))
        .route("/health/ready", web::get().to(crate::runtime::ready))
        .route("/api/session", web::get().to(session))
        .route("/api/demo/accounts", web::get().to(accounts))
        .route("/api/demo/session", web::post().to(switch_session))
        .route("/api/restaurants", web::get().to(restaurants))
        .route("/api/restaurants/{id}/menu", web::get().to(menu))
        .route(
            "/api/restaurants/{id}/menu/{item_id}/availability",
            web::patch().to(availability),
        )
        .route("/api/orders/quote", web::post().to(quote))
        .configure(super::orders::configure);
}
async fn session(pool: web::Data<PgPool>, req: HttpRequest) -> HttpResponse {
    match auth::identity(&pool, &req).await {
        Ok(Some(i)) => HttpResponse::Ok()
            .json(serde_json::json!({"user_id":i.user_id,"role":i.role,"csrf_token":i.csrf})),
        Ok(None) => match auth::bootstrap(&pool).await {
            Ok((token, i)) => HttpResponse::Ok()
                .cookie(auth::session_cookie(token))
                .json(serde_json::json!({"csrf_token":i.csrf})),
            Err(_) => HttpResponse::ServiceUnavailable().finish(),
        },
        Err(_) => HttpResponse::InternalServerError().finish(),
    }
}
async fn accounts(pool: web::Data<PgPool>) -> HttpResponse {
    match sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT id,display_name,role FROM users ORDER BY display_name",
    )
    .fetch_all(pool.get_ref())
    .await
    {
        Ok(rows) => HttpResponse::Ok().json(
            rows.into_iter()
                .map(
                    |(id, name, role)| serde_json::json!({"id":id,"display_name":name,"role":role}),
                )
                .collect::<Vec<_>>(),
        ),
        Err(_) => HttpResponse::InternalServerError().finish(),
    }
}
#[derive(serde::Deserialize)]
struct Switch {
    user_id: Uuid,
}
async fn switch_session(
    pool: web::Data<PgPool>,
    req: HttpRequest,
    body: web::Json<Switch>,
) -> HttpResponse {
    let Ok(Some(i)) = auth::identity(&pool, &req).await else {
        return HttpResponse::Unauthorized().finish();
    };
    if let Err(response) = auth::require_mutation(&req, &i) {
        return response;
    };
    let exists = sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM users WHERE id=$1)")
        .bind(body.user_id)
        .fetch_one(pool.get_ref())
        .await
        .unwrap_or(false);
    if !exists {
        return HttpResponse::NotFound().finish();
    }
    let _ = sqlx::query("DELETE FROM sessions WHERE csrf_token=$1")
        .bind(&i.csrf)
        .execute(pool.get_ref())
        .await;
    match auth::bootstrap(&pool).await {
        Ok((token, new)) => {
            let _ = sqlx::query("UPDATE sessions SET user_id=$1 WHERE csrf_token=$2")
                .bind(body.user_id)
                .bind(&new.csrf)
                .execute(pool.get_ref())
                .await;
            HttpResponse::Ok()
                .cookie(auth::session_cookie(token))
                .json(serde_json::json!({"csrf_token":new.csrf,"user_id":body.user_id}))
        }
        Err(_) => HttpResponse::InternalServerError().finish(),
    }
}
async fn restaurants(pool: web::Data<PgPool>) -> HttpResponse {
    let rows = sqlx::query_as::<_, (Uuid, String, i64)>(
        "SELECT id,name,configuration_version FROM restaurants ORDER BY name",
    )
    .fetch_all(pool.get_ref())
    .await
    .unwrap_or_default();
    HttpResponse::Ok().json(rows.into_iter().map(|(id,name,version)|serde_json::json!({"id":id,"name":name,"configuration_version":version})).collect::<Vec<_>>())
}
async fn menu(pool: web::Data<PgPool>, path: web::Path<Uuid>) -> HttpResponse {
    let rows=sqlx::query_as::<_,(Uuid,String,i64,bool)>("SELECT id,name,price_cents,available FROM menu_items WHERE restaurant_id=$1 ORDER BY position").bind(*path).fetch_all(pool.get_ref()).await.unwrap_or_default();
    HttpResponse::Ok().json(rows.into_iter().map(|(id,name,price,available)|serde_json::json!({"id":id,"name":name,"price_cents":price,"available":available})).collect::<Vec<_>>())
}
#[derive(serde::Deserialize)]
struct Availability {
    available: bool,
}
async fn availability(
    pool: web::Data<PgPool>,
    req: HttpRequest,
    path: web::Path<(Uuid, Uuid)>,
    body: web::Json<Availability>,
) -> HttpResponse {
    let Ok(Some(i)) = auth::identity(&pool, &req).await else {
        return HttpResponse::Unauthorized().finish();
    };
    if let Err(response) = auth::require_mutation(&req, &i) {
        return response;
    };
    if i.role.as_deref() != Some("staff") {
        return HttpResponse::Forbidden().finish();
    }
    let (restaurant, item) = path.into_inner();
    let Ok(mut tx) = pool.begin().await else {
        return HttpResponse::InternalServerError().finish();
    };
    let allowed = sqlx::query_scalar::<_, Uuid>("SELECT r.id FROM restaurants r JOIN staff_restaurants sr ON sr.restaurant_id=r.id WHERE r.id=$1 AND sr.user_id=$2 FOR UPDATE OF r")
        .bind(restaurant).bind(i.user_id).fetch_optional(&mut *tx).await.unwrap_or(None).is_some();
    if !allowed {
        return HttpResponse::NotFound().finish();
    }
    let changed =
        sqlx::query("UPDATE menu_items SET available=$1 WHERE id=$2 AND restaurant_id=$3")
            .bind(body.available)
            .bind(item)
            .bind(restaurant)
            .execute(&mut *tx)
            .await
            .map(|x| x.rows_affected())
            .unwrap_or(0);
    if changed == 0 {
        return HttpResponse::NotFound().finish();
    }
    let _ = sqlx::query(
        "UPDATE restaurants SET configuration_version=configuration_version+1 WHERE id=$1",
    )
    .bind(restaurant)
    .execute(&mut *tx)
    .await;
    if tx.commit().await.is_err() {
        return HttpResponse::InternalServerError().finish();
    }
    HttpResponse::Ok().finish()
}
async fn quote(
    pool: web::Data<PgPool>,
    req: HttpRequest,
    body: web::Json<QuoteRequest>,
) -> HttpResponse {
    let Ok(Some(i)) = auth::identity(&pool, &req).await else {
        return HttpResponse::Unauthorized().finish();
    };
    if i.role.as_deref() != Some("customer") {
        return HttpResponse::Forbidden().finish();
    }
    if let Err(response) = auth::require_mutation(&req, &i) {
        return response;
    }
    let Ok(rid) = Uuid::parse_str(&body.restaurant_id) else {
        return HttpResponse::UnprocessableEntity().finish();
    };
    let Ok((tax,fee,version))=sqlx::query_as::<_,(i32,Option<i32>,i64)>("SELECT tax_basis_points,service_fee_basis_points,configuration_version FROM restaurants WHERE id=$1").bind(rid).fetch_one(pool.get_ref()).await else{return HttpResponse::NotFound().finish()};
    let mut items = Vec::new();
    for line in &body.lines {
        let Ok(id) = Uuid::parse_str(&line.item_id) else {
            return HttpResponse::UnprocessableEntity().finish();
        };
        let Ok((restaurant_id, price, available)) = sqlx::query_as::<_, (Uuid, i64, bool)>(
            "SELECT restaurant_id,price_cents,available FROM menu_items WHERE id=$1",
        )
        .bind(id)
        .fetch_one(pool.get_ref())
        .await
        else {
            return HttpResponse::UnprocessableEntity().finish();
        };
        let group_rows = sqlx::query_as::<_, (Uuid, bool)>(
            "SELECT id,required FROM option_groups WHERE menu_item_id=$1 ORDER BY position",
        )
        .bind(id)
        .fetch_all(pool.get_ref())
        .await
        .unwrap_or_default();
        let mut groups = Vec::new();
        for (group_id, required) in group_rows {
            let options = sqlx::query_as::<_, (Uuid, i64)>(
                "SELECT id,price_adjustment_cents FROM menu_options WHERE option_group_id=$1 ORDER BY position",
            )
            .bind(group_id)
            .fetch_all(pool.get_ref())
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|(id, adjustment_cents)| OptionDef { id: id.to_string(), adjustment_cents })
            .collect();
            groups.push(Group { required, options });
        }
        items.push(Item {
            id: line.item_id.clone(),
            restaurant_id: restaurant_id.to_string(),
            price_cents: price,
            available,
            groups,
        });
    }
    match catalog::quote(&body, &items, tax, fee, version) {
        Ok(q) => HttpResponse::Ok().json(q),
        Err(e) => {
            HttpResponse::UnprocessableEntity().json(serde_json::json!({"code":format!("{e:?}")}))
        }
    }
}
pub async fn seed_database(pool: &PgPool) -> Result<(), sqlx::Error> {
    seed::seed(pool).await
}
