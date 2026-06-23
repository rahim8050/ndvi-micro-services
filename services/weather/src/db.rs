use sqlx::{mysql::MySqlPoolOptions, MySqlPool};
use tokio::time::{sleep, Duration};

const DB_CONNECT_MAX_RETRIES: u32 = 10;
const DB_CONNECT_RETRY_DELAY_MS: u64 = 2_000;

pub async fn create_pool(database_url: &str) -> Result<MySqlPool, sqlx::Error> {
    let mut last_err = None;
    for attempt in 1..=DB_CONNECT_MAX_RETRIES {
        match MySqlPoolOptions::new()
            .max_connections(10)
            .connect(database_url)
            .await
        {
            Ok(pool) => return Ok(pool),
            Err(e) => {
                tracing::warn!(
                    "db.connect.retry attempt={}/{} error={}",
                    attempt,
                    DB_CONNECT_MAX_RETRIES,
                    e,
                );
                last_err = Some(e);
                if attempt < DB_CONNECT_MAX_RETRIES {
                    sleep(Duration::from_millis(DB_CONNECT_RETRY_DELAY_MS)).await;
                }
            }
        }
    }
    Err(last_err.unwrap())
}
