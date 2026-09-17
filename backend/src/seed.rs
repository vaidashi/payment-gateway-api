use sqlx::PgPool;
use uuid::Uuid;

pub async fn seed(pool: &PgPool) -> Result<(), sqlx::Error> {
    if sqlx::query_scalar::<_, i64>("SELECT count(*) FROM restaurants")
        .fetch_one(pool)
        .await?
        > 0
    {
        return Ok(());
    }
    for n in 1..=4 {
        let id = Uuid::new_v4();
        let fee = [None, Some(0), Some(150), Some(300)][n - 1];
        sqlx::query("INSERT INTO restaurants(id,name,tax_basis_points,service_fee_basis_points) VALUES($1,$2,800,$3)").bind(id).bind(format!("Demo Restaurant {n}")).bind(fee).execute(pool).await?;
        for item in 1..=12 {
            let item_id = Uuid::new_v4();
            sqlx::query("INSERT INTO menu_items(id,restaurant_id,name,price_cents,available,position) VALUES($1,$2,$3,$4,$5,$6)").bind(item_id).bind(id).bind(format!("Menu item {item}")).bind(500_i64+item as i64*25).bind(item != 12).bind(item).execute(pool).await?;
            let choice = Uuid::new_v4();
            let extra = Uuid::new_v4();
            sqlx::query("INSERT INTO option_groups(id,menu_item_id,name,required,position) VALUES($1,$2,'Required choice',true,0),($3,$2,'Extras',false,1)").bind(choice).bind(item_id).bind(extra).execute(pool).await?;
            for (group, name, price) in [
                (choice, "Standard", 0_i64),
                (choice, "Large", 150),
                (extra, "Extra sauce", 50),
                (extra, "Add cheese", 100),
            ] {
                sqlx::query("INSERT INTO menu_options(id,option_group_id,name,price_adjustment_cents) VALUES($1,$2,$3,$4)").bind(Uuid::new_v4()).bind(group).bind(name).bind(price).execute(pool).await?;
            }
        }
    }
    for n in 1..=4 {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO users(id,display_name,role) VALUES($1,$2,'customer')")
            .bind(id)
            .bind(format!("Customer {n}"))
            .execute(pool)
            .await?;
    }
    let restaurants: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM restaurants ORDER BY name")
        .fetch_all(pool)
        .await?;
    for (i, restaurant) in restaurants.iter().enumerate() {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO users(id,display_name,role) VALUES($1,$2,'staff')")
            .bind(id)
            .bind(format!("Staff {}", i + 1))
            .execute(pool)
            .await?;
        sqlx::query("INSERT INTO staff_restaurants(user_id,restaurant_id) VALUES($1,$2)")
            .bind(id)
            .bind(restaurant)
            .execute(pool)
            .await?;
    }
    sqlx::query("INSERT INTO users(id,display_name,role) VALUES($1,'Demo Admin','admin')")
        .bind(Uuid::new_v4())
        .execute(pool)
        .await?;
    Ok(())
}
