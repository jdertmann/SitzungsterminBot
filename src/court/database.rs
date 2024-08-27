use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use redis::{AsyncCommands, RedisFuture, RedisResult};
use teloxide::types::ChatId;

use super::{CourtState, Subscription};

pub type RedisConnection = redis::aio::MultiplexedConnection;

#[derive(Debug, Clone)]
pub struct Database {
    redis: redis::Client,
    name: String,
    redis_conn: Option<RedisConnection>,
}

impl Database {
    pub fn new(client: redis::Client, court_name: &str) -> Self {
        Self {
            redis: client,
            name: court_name.to_string(),
            redis_conn: None,
        }
    }

    fn court_info_key(&self) -> String {
        format!("court:{}:info", self.name)
    }

    fn sub_key(&self) -> String {
        format!("court:{}:subs", self.name)
    }

    pub async fn get_connection(&mut self) -> RedisResult<RedisConnection> {
        if let Some(conn) = &self.redis_conn {
            Ok(conn.clone())
        } else {
            let conn = self.redis.get_multiplexed_async_connection().await?;
            self.redis_conn = Some(conn.clone());
            Ok(conn)
        }
    }

    pub async fn execute_with_retry<F, T>(&mut self, mut operation: F) -> RedisResult<T>
    where
        F: FnMut(&mut RedisConnection) -> RedisFuture<'_, T>,
    {
        const MAX_RETRIES: usize = 5;

        let mut attempt = 0;

        loop {
            let result = match self.get_connection().await {
                Ok(mut conn) => operation(&mut conn).await,
                Err(e) => Err(e),
            };

            match result {
                Ok(result) => return Ok(result),
                Err(err) => {
                    match get_retry_strategy(&err) {
                        s @ (RetryStrategy::Reconnect | RetryStrategy::Retry)
                            if attempt < MAX_RETRIES =>
                        {
                            attempt += 1;
                            log::warn!(
                                "Redis error, trying to recover ({}/{MAX_RETRIES}, {:?}): {err}",
                                attempt,
                                s
                            );
                            if matches!(s, RetryStrategy::Reconnect) {
                                self.redis_conn = None; // Drop the current connection
                            }
                            let backoff = Duration::from_millis(50 * (1 << attempt));
                            tokio::time::sleep(backoff).await;
                        }
                        _ => {
                            log::warn!("Redis error, not recovering: {err}");
                            return Err(err);
                        }
                    }
                }
            }
        }
    }

    pub async fn load_court_state(&mut self) -> RedisResult<Option<CourtState>> {
        let key = self.court_info_key();
        let info_str: Option<String> = self
            .execute_with_retry(|conn| conn.get(key.clone()))
            .await?;

        let info = match info_str {
            Some(info_str) => Some(serde_json::from_str(&info_str)?),
            None => None,
        };

        Ok(info)
    }

    pub async fn save_court_state(&mut self, info: &CourtState) -> RedisResult<()> {
        let key = self.court_info_key();
        let info_str = serde_json::to_string(info).expect("Couldn't serialize");
        self.execute_with_retry(|conn| conn.set(key.clone(), info_str.clone()))
            .await
    }

    pub async fn load_subscriptions(&mut self) -> RedisResult<HashSet<Subscription>> {
        let sub_key = self.sub_key();
        let map: BTreeMap<String, String> = self
            .execute_with_retry(|conn| conn.hgetall(sub_key.clone()))
            .await?;

        let mut subscriptions = HashSet::new();

        for (k, reference) in map {
            let Some((chat_id, name)) = k.split_once(':') else {
                log::info!("Invalid subscription key in database, skipping");
                continue;
            };

            let Ok(chat_id) = chat_id.parse() else {
                log::info!("Invalid subscription key in database, skipping");
                continue;
            };

            subscriptions.insert(Subscription {
                chat_id: ChatId(chat_id),
                name: name.to_string(),
                reference: reference,
            });
        }

        Ok(subscriptions)
    }

    pub async fn save_subscription(&mut self, sub: &Subscription) -> RedisResult<()> {
        let sub_key = self.sub_key();

        let sub = sub.clone();
        self.execute_with_retry(|conn| {
            conn.hset(
                sub_key.clone(),
                format!("{}:{}", sub.chat_id.0, sub.name),
                sub.reference.clone(),
            )
        })
        .await
    }

    pub async fn remove_subscription(&mut self, name: &str, chat_id: ChatId) -> RedisResult<usize> {
        let sub_key = self.sub_key();
        self.execute_with_retry(|conn| {
            conn.hdel(sub_key.clone(), format!("{}:{}", chat_id.0, &name))
        })
        .await
    }
}

#[derive(Debug)]
enum RetryStrategy {
    Reconnect,
    Retry,
    NoRetry,
}

fn get_retry_strategy(err: &redis::RedisError) -> RetryStrategy {
    use redis::ErrorKind::*;

    match err.kind() {
        TryAgain
        | MasterDown
        | ClusterDown
        | BusyLoadingError
        | MasterNameNotFoundBySentinel
        | NoValidReplicasFoundBySentinel => RetryStrategy::Retry,
        ParseError | AuthenticationFailed | ClusterConnectionNotFound | IoError => {
            RetryStrategy::Reconnect
        }
        _ => RetryStrategy::NoRetry,
    }
}
