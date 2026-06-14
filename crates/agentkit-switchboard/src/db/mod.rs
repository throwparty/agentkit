use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;

pub async fn create_pool(path: &str) -> Result<SqlitePool, sqlx::Error> {
    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .connect(path)
        .await?;
    sqlx::migrate!("src/db/migrations").run(&pool).await?;
    Ok(pool)
}
