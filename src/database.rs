use chrono::{DateTime, NaiveDate, Utc};
use futures::stream::TryStreamExt;
use sqlx::sqlite::SqlitePool;
pub use sqlx::Error;
use sqlx::{query, query_as, query_scalar, FromRow, QueryBuilder};
use teloxide::types::ChatId;
use thiserror::Error;

use crate::scraper::Session;

#[derive(Clone)]
pub struct Database {
    pool: SqlitePool,
}

#[derive(Error, Debug)]
#[error("timestamp out of range")]
pub struct TimestampOutOfRangeError;

impl Database {
    pub async fn new(database_url: &str) -> Result<Self, Error> {
        let pool = SqlitePool::connect(database_url).await?;
        Ok(Self { pool })
    }

    // Add a new subscription
    pub async fn add_subscription(
        &self,
        chat_id: ChatId,
        court: &str,
        name: &str,
        reference_filter: &str,
    ) -> Result<Option<i64>, Error> {
        let mut transaction = self.pool.begin().await?;

        let exists: i64 = query_scalar!(
            "SELECT COUNT(*) FROM subscriptions WHERE chat_id = ? AND name = ?",
            chat_id.0,
            name
        )
        .fetch_one(&mut *transaction)
        .await?;

        if exists > 0 {
            transaction.rollback().await?;
            return Ok(None);
        }

        let id = query!(
            "INSERT INTO subscriptions (chat_id, court, name, reference_filter)
            VALUES (?, ?, ?, ?)",
            chat_id.0,
            court,
            name,
            reference_filter
        )
        .execute(&mut *transaction)
        .await?
        .last_insert_rowid();

        transaction.commit().await?;

        Ok(Some(id))
    }

    pub async fn get_subscription_by_id(
        &self,
        subscription_id: i64,
    ) -> Result<Option<Subscription>, Error> {
        query_as!(
            Subscription,
            "SELECT * FROM subscriptions WHERE subscription_id= ?",
            subscription_id
        )
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn set_subscription_confirmation_sent(
        &self,
        subscription_id: i64,
    ) -> Result<bool, Error> {
        query!(
            "UPDATE subscriptions SET confirmation_sent = 1 WHERE subscription_id = ?",
            subscription_id
        )
        .execute(&self.pool)
        .await
        .map(|r| r.rows_affected() > 0)
    }

    pub async fn remove_subscription(&self, chat_id: ChatId, name: &str) -> Result<bool, Error> {
        query!(
            "DELETE FROM subscriptions WHERE chat_id = ? AND name = ?",
            chat_id.0,
            name
        )
        .execute(&self.pool)
        .await
        .map(|r| r.rows_affected() > 0)
    }

    pub async fn get_subscriptions_by_chat(
        &self,
        chat_id: ChatId,
    ) -> Result<Vec<Subscription>, Error> {
        sqlx::query_as(
            "SELECT
                COALESCE(c.full_name, s.court) court,
                s.subscription_id,
                s.chat_id,
                s.confirmation_sent,
                s.name,
                s.reference_filter
            FROM subscriptions s LEFT JOIN courts c ON s.court = c.name
            WHERE s.chat_id = ?"
        ).bind(
            chat_id.0
        )
        .fetch_all(&self.pool)
        .await
    }

    pub async fn get_confirmed_subscriptions_by_court(
        &self,
        court: &str,
    ) -> Result<Vec<Subscription>, Error> {
        query_as!(
            Subscription,
            "SELECT * FROM subscriptions WHERE court = ? AND confirmation_sent != 0",
            court
        )
        .fetch_all(&self.pool)
        .await
    }

    pub async fn update_court_info(
        &self,
        court: &str,
        last_update: &DateTime<Utc>,
        full_name: Option<&str>,
        schedule: Option<&[Session]>,
    ) -> Result<(), Error> {
        let mut transaction = self.pool.begin().await?;
        let timestamp = last_update.timestamp();

        query!(
            "INSERT INTO courts (name, full_name, last_update)
                VALUES($1, $2, $3) 
                ON CONFLICT(name) 
                DO UPDATE SET full_name = $2, last_update = $3",
            court,
            full_name,
            timestamp
        )
        .execute(&mut *transaction)
        .await?;

        if let Some(schedule) = schedule {
            // Delete old sessions for the court
            query!("DELETE FROM sessions WHERE court = ?", court)
                .execute(&mut *transaction)
                .await?;

            // Insert new sessions
            for session in schedule {
                query!(
                    "INSERT INTO sessions (court, date, time, type, lawsuit, hall, reference, note)
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                    court,
                    session.date,
                    session.time,
                    session.r#type,
                    session.lawsuit,
                    session.hall,
                    session.reference,
                    session.note
                )
                .execute(&mut *transaction)
                .await?;
            }
        }

        transaction.commit().await?;

        Ok(())
    }

    pub async fn get_court_meta(&self, court_name: &str) -> Result<Option<CourtMeta>, Error> {
        let row = query!(
            "SELECT full_name, last_update FROM courts WHERE name=?",
            court_name
        )
        .fetch_optional(&self.pool)
        .await?;

        row.map(|row| {
            let last_update = DateTime::from_timestamp(row.last_update, 0).ok_or_else(|| {
                Error::ColumnDecode {
                    index: "last_update".to_string(),
                    source: Box::new(TimestampOutOfRangeError),
                }
            })?;
            Ok(CourtMeta {
                full_name: row.full_name,
                last_update,
            })
        })
        .transpose()
    }

    pub async fn get_sessions(
        &self,
        court_name: &str,
        reference_filter: Option<&str>,
        date_filter: Option<NaiveDate>,
    ) -> Result<Vec<Session>, Error> {
        let mut query = QueryBuilder::new(
            "SELECT date,time,type,lawsuit,hall,reference,note FROM sessions WHERE court = ",
        );
        query.push_bind(court_name);

        if let Some(reference) = reference_filter {
            query.push(" AND reference LIKE  ").push_bind(reference);
        }

        if let Some(date) = date_filter {
            query.push(" AND date = ").push_bind(date.to_string());
        }

        query
            .build()
            .fetch(&self.pool)
            .and_then(|x| async move { Session::from_row(&x) })
            .try_collect()
            .await
    }

    pub async fn get_subscribed_courts(&self) -> Result<Vec<String>, Error> {
        query_scalar!("SELECT DISTINCT court FROM subscriptions")
            .fetch_all(&self.pool)
            .await
    }
}

#[derive(Debug, sqlx::FromRow)]
#[allow(unused)]
pub struct Subscription {
    pub subscription_id: i64,
    pub chat_id: i64,
    pub court: String,
    pub confirmation_sent: i64,
    pub name: String,
    pub reference_filter: String,
}

#[derive(Debug, Clone)]
pub struct CourtMeta {
    pub full_name: Option<String>,
    pub last_update: DateTime<Utc>,
}
