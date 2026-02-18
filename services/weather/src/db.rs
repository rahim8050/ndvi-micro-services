use sqlx::{mysql::MySqlPoolOptions, MySqlPool};

#[derive(Clone)]
pub struct AppState {
    pub pool: MySqlPool,
}

pub async fn create_pool(database_url: &str) -> Result<MySqlPool, sqlx::Error> {
    MySqlPoolOptions::new()
        .max_connections(10)
        .connect(database_url)
        .await
}
