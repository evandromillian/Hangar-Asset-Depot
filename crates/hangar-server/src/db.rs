//! Embedded SQL migrations, applied at boot inside a transaction each.

use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};

const MIGRATIONS: &[(&str, &str)] = &[
    ("0001_init", include_str!("../migrations/0001_init.sql")),
    ("0002_skeletons", include_str!("../migrations/0002_skeletons.sql")),
    ("0003_api_tokens", include_str!("../migrations/0003_api_tokens.sql")),
    ("0004_blob_uploads", include_str!("../migrations/0004_blob_uploads.sql")),
];

pub async fn migrate(db: &DatabaseConnection) -> anyhow::Result<()> {
    db.execute_unprepared(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            name TEXT PRIMARY KEY,
            applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
        )",
    )
    .await?;

    for (name, sql) in MIGRATIONS {
        let txn = db.begin().await?;
        // Serialise concurrent boots of several instances.
        txn.execute_unprepared("LOCK TABLE schema_migrations IN EXCLUSIVE MODE").await?;
        let applied = txn
            .query_one_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT 1 AS one FROM schema_migrations WHERE name = $1",
                [(*name).into()],
            ))
            .await?
            .is_some();
        if applied {
            txn.rollback().await?;
            continue;
        }
        tracing::info!("applying migration {name}");
        txn.execute_unprepared(sql).await?;
        txn.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO schema_migrations(name) VALUES ($1)",
            [(*name).into()],
        ))
        .await?;
        txn.commit().await?;
    }
    Ok(())
}
