use sqlx::migrate::Migrator;

#[tokio::main]
async fn main() {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let db = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root (crates/buzz-db parent x2)");
    let m = Migrator::new(root.join("migrations").as_path())
        .await
        .expect("load migrations");
    m.run(&db).await.expect("run migrations");
    let v: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(version),0) FROM _sqlx_migrations")
        .fetch_one(&db)
        .await
        .expect("version");
    println!("MIGRATED_TO={v}");
}
