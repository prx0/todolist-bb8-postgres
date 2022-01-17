use bb8_postgres::bb8::{Pool, PooledConnection, RunError};
use bb8_postgres::PostgresConnectionManager;
use chrono::{DateTime, Utc};
use once_cell::sync::OnceCell;
use snafu::{ResultExt, Snafu};
use tokio_postgres::types::{FromSql, ToSql};
use tokio_postgres::{NoTls, Row, ToStatement};
use uuid::Uuid;

static DB_MANAGER_INSTANCE: OnceCell<DBManager> = OnceCell::new();

pub type DBConnection<'a> = PooledConnection<'a, PostgresConnectionManager<NoTls>>;
pub type DBPool = Pool<PostgresConnectionManager<NoTls>>;
pub type PostgresConnectionError = RunError<tokio_postgres::error::Error>;

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("ConnectionError: {}", source))]
    ConnectionError { source: PostgresConnectionError },

    #[snafu(display("PostgresError: {}", source))]
    PostgresError { source: tokio_postgres::Error },
}

pub struct DBOptions {
    // see https://docs.rs/tokio-postgres/latest/tokio_postgres/config/struct.Config.html"
    pub pg_params: String,
    pub pool_max_size: u32,
}

// We call the DBManager when required
// like a kind of singleton
pub struct DBManager {
    pool: DBPool,
}

impl DBManager {
    // Get an instance of DBManager
    pub async fn get() -> &'static DBManager {
        DB_MANAGER_INSTANCE.get().unwrap()
    }

    // Create the DBManager instance using DBOptions
    async fn new(config: DBOptions) -> Result<Self, Error> {
        let DBOptions {
            pg_params,
            pool_max_size,
        } = config;

        let manager = PostgresConnectionManager::new_from_stringlike(pg_params, NoTls)
            .expect("unable build PostgresConnectionManager");

        let pool = Pool::builder()
            .max_size(pool_max_size)
            .build(manager)
            .await
            .context(PostgresError)?;

        Ok(Self { pool })
    }

    // Helper to get a connection from the bb8 pool
    pub async fn connection(&self) -> Result<DBConnection<'_>, Error> {
        let conn = self.pool.get().await.context(ConnectionError)?;
        Ok(conn)
    }

    // Perform a query from a fetched bb8 connection
    pub async fn query<T>(
        &self,
        statement: &T,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Vec<Row>, Error>
    where
        T: ?Sized + ToStatement,
    {
        let conn = self.connection().await?;
        let rows = conn.query(statement, params).await.context(PostgresError)?;
        Ok(rows)
    }

    // Perform a query_one from a fetched bb8 connection
    pub async fn query_one<T>(
        &self,
        statement: &T,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Row, Error>
    where
        T: ?Sized + ToStatement,
    {
        let conn = self.connection().await?;
        let row = conn
            .query_one(statement, params)
            .await
            .context(PostgresError)?;
        Ok(row)
    }
}

#[derive(Debug, ToSql, FromSql)]
#[postgres(name = "priority_level")]
pub enum PriorityLevel {
    Low,
    Medium,
    High,
}

#[derive(Debug)]
pub struct Todo {
    id: uuid::Uuid,
    task: String,
    priority: PriorityLevel,
    created_at: DateTime<Utc>,
    expired_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
}

impl Todo {
    pub fn new(task: String, priority: PriorityLevel, expired_at: Option<DateTime<Utc>>) -> Self {
        Self {
            id: Uuid::new_v4(),
            task,
            priority,
            created_at: chrono::offset::Utc::now(),
            expired_at: None,
            completed_at: None,
        }
    }

    pub async fn get_all() -> Result<Vec<Self>, Error> {
        let select_one_todo = "
        select
            id as todo_id,
            task as todo_task,
            priority as todo_priority,
            created_at as todo_created_at,
            expired_at as todo_expired_at,
            completed_at as todo_completed_at
            from todo;";

        let rows = DBManager::get().await.query(select_one_todo, &[]).await?;

        let todo_list: Vec<Self> = rows
            .iter()
            .map(|row| Self::try_from(row).unwrap())
            .collect();

        Ok(todo_list)
    }

    pub async fn get_by_id(id: &Uuid) -> Result<Self, Error> {
        let select_one_todo = "
        select
            id as todo_id,
            task as todo_task,
            priority as todo_priority,
            created_at as todo_created_at,
            expired_at as todo_expired_at,
            completed_at as todo_completed_at
            from todo where id = $1;";

        let row = DBManager::get()
            .await
            .query_one(select_one_todo, &[id])
            .await?;

        Ok(Self::try_from(&row)?)
    }

    pub fn toogle_complete(&mut self) {
        self.completed_at = match self.completed_at {
            Some(_) => None,
            None => Some(chrono::offset::Utc::now()),
        }
    }

    pub async fn save(&self) -> Result<&Self, Error> {
        let insert_new_todo = "
            insert into todo (id, task, priority, created_at, expired_at, completed_at)
            values ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (id)
            DO UPDATE SET
                task = EXCLUDED.task,
                priority = EXCLUDED.priority,
                created_at = EXCLUDED.created_at,
                expired_at = EXCLUDED.expired_at,
                completed_at = EXCLUDED.completed_at;";

        let _ = DBManager::get()
            .await
            .query(
                insert_new_todo,
                &[
                    &self.id,
                    &self.task,
                    &self.priority,
                    &self.created_at,
                    &self.expired_at,
                    &self.completed_at,
                ],
            )
            .await?;
        Ok(self)
    }
}

impl<'a> TryFrom<&'a Row> for Todo {
    type Error = Error;

    fn try_from(row: &'a Row) -> Result<Self, Self::Error> {
        let id = row.try_get("todo_id").context(PostgresError)?;
        let task = row.try_get("todo_task").context(PostgresError)?;
        let created_at = row.try_get("todo_created_at").context(PostgresError)?;
        let expired_at = row.try_get("todo_expired_at").context(PostgresError)?;
        let completed_at = row.try_get("todo_completed_at").context(PostgresError)?;
        let priority = row.try_get("todo_priority").context(PostgresError)?;

        Ok(Self {
            id,
            task,
            created_at,
            expired_at,
            completed_at,
            priority,
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let options = DBOptions {
        pg_params: String::from(
            "postgres://postgres:test@localhost:5432/postgres?connect_timeout=10",
        ),
        pool_max_size: 8u32,
    };

    let _ = DB_MANAGER_INSTANCE.set(DBManager::new(options).await?);

    let mut todo_finish_this_draft = Todo::new(String::from("Publish this draft"), PriorityLevel::High, None);

    todo_finish_this_draft.save().await?;

    println!("{:?}", todo_finish_this_draft);

    todo_finish_this_draft.toogle_complete();
    todo_finish_this_draft.save().await?;

    println!("{:?}", todo_finish_this_draft);

    let todo_list = Todo::get_all().await?;
    println!("{:?}", todo_list);

    Ok(())
}
