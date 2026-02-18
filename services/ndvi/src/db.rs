use sqlx::{postgres::PgPoolOptions, PgPool};

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
}

pub async fn create_pool(database_url: &str) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(10)
        .connect(database_url)
        .await
}
