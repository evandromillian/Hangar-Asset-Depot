//! Durable job queue on Postgres and the processing workers.
//!
//! Jobs are claimed with `FOR UPDATE SKIP LOCKED`, so any number of workers
//! (threads or server instances) can share the queue. Failed jobs are retried
//! with quadratic back-off; jobs stuck in `running` (crashed worker) are
//! re-queued after `STALE_AFTER`.

use crate::config::HangarConfig;
use crate::entities::{asset_versions, version_files};
use crate::storage::BlobStore;
use anyhow::Context;
use hangar_core::{FileEntry, VersionStatus};
use hangar_pipeline::{ArtifactRole, PipelineConfig, ProcessInput};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, FromQueryResult, QueryFilter, Statement,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use uuid::Uuid;

pub const KIND_PROCESS_VERSION: &str = "process_version";
const POLL_EVERY: Duration = Duration::from_secs(5);
const STALE_AFTER: &str = "30 minutes";

#[derive(Debug, Serialize, Deserialize)]
pub struct ProcessVersionPayload {
    pub version_id: Uuid,
}

#[derive(Debug, FromQueryResult)]
pub struct JobRow {
    pub id: i64,
    pub kind: String,
    pub payload: serde_json::Value,
    pub attempts: i32,
    pub max_attempts: i32,
}

#[derive(Clone)]
pub struct JobQueue {
    db: DatabaseConnection,
    notify: Arc<Notify>,
}

impl JobQueue {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db, notify: Arc::new(Notify::new()) }
    }

    /// Enqueue inside the caller's transaction (or connection).
    pub async fn enqueue<C: ConnectionTrait>(&self, conn: &C, kind: &str, payload: impl Serialize) -> anyhow::Result<()> {
        conn.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO jobs(kind, payload) VALUES ($1, $2)",
            [kind.into(), serde_json::to_value(payload)?.into()],
        ))
        .await?;
        Ok(())
    }

    /// Wake a sleeping worker (call after the enqueueing transaction committed).
    pub fn wake(&self) {
        self.notify.notify_one();
    }

    async fn claim(&self) -> anyhow::Result<Option<JobRow>> {
        let row = JobRow::find_by_statement(Statement::from_string(
            DbBackend::Postgres,
            "UPDATE jobs SET status = 'running', locked_at = now(), attempts = attempts + 1
             WHERE id = (
                 SELECT id FROM jobs
                 WHERE status = 'queued' AND run_after <= now()
                 ORDER BY run_after, id
                 FOR UPDATE SKIP LOCKED
                 LIMIT 1)
             RETURNING id, kind, payload, attempts, max_attempts",
        ))
        .one(&self.db)
        .await?;
        Ok(row)
    }

    async fn finish(&self, id: i64) -> anyhow::Result<()> {
        self.db
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE jobs SET status = 'done', finished_at = now(), last_error = NULL WHERE id = $1",
                [id.into()],
            ))
            .await?;
        Ok(())
    }

    /// Returns true when the job will be retried.
    async fn fail(&self, job: &JobRow, err: &str) -> anyhow::Result<bool> {
        let retry = job.attempts < job.max_attempts;
        let backoff_secs = 30 * (job.attempts as i64).pow(2);
        self.db
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE jobs SET status = CASE WHEN $2 THEN 'queued' ELSE 'failed' END,
                        run_after = now() + make_interval(secs => $3),
                        finished_at = CASE WHEN $2 THEN NULL ELSE now() END,
                        locked_at = NULL, last_error = $4
                 WHERE id = $1",
                [job.id.into(), retry.into(), (backoff_secs as f64).into(), err.into()],
            ))
            .await?;
        Ok(retry)
    }

    async fn requeue_stale(&self) -> anyhow::Result<u64> {
        let res = self
            .db
            .execute_raw(Statement::from_string(
                DbBackend::Postgres,
                format!(
                    "UPDATE jobs SET status = 'queued', locked_at = NULL
                     WHERE status = 'running' AND locked_at < now() - interval '{STALE_AFTER}'"
                ),
            ))
            .await?;
        Ok(res.rows_affected())
    }
}

/// Everything a worker needs.
#[derive(Clone)]
pub struct WorkerCtx {
    pub db: DatabaseConnection,
    pub store: BlobStore,
    pub queue: JobQueue,
    pub cfg: HangarConfig,
    pub pipeline: PipelineConfig,
}

pub async fn run_workers(ctx: WorkerCtx) -> summer::error::Result<String> {
    match ctx.queue.requeue_stale().await {
        Ok(n) if n > 0 => tracing::warn!("re-queued {n} stale job(s)"),
        Ok(_) => {}
        Err(e) => tracing::error!("requeue stale jobs: {e:#}"),
    }
    let n = ctx.cfg.workers.max(1);
    tracing::info!(
        workers = n,
        blender = ?ctx.pipeline.blender_path,
        "processing workers started"
    );
    let mut handles = Vec::new();
    for i in 0..n {
        let ctx = ctx.clone();
        handles.push(tokio::spawn(async move { worker_loop(i, ctx).await }));
    }
    summer::signal::shutdown_signal("job workers").await;
    for h in handles {
        h.abort();
    }
    Ok("job workers stopped".into())
}

async fn worker_loop(index: usize, ctx: WorkerCtx) {
    loop {
        match ctx.queue.claim().await {
            Ok(Some(job)) => {
                let id = job.id;
                tracing::info!(worker = index, job = id, kind = %job.kind, attempt = job.attempts, "job started");
                let started = std::time::Instant::now();
                match handle(&ctx, &job).await {
                    Ok(()) => {
                        tracing::info!(job = id, elapsed_ms = started.elapsed().as_millis() as u64, "job done");
                        if let Err(e) = ctx.queue.finish(id).await {
                            tracing::error!(job = id, "mark job done: {e:#}");
                        }
                    }
                    Err(err) => {
                        let msg = format!("{err:#}");
                        tracing::error!(job = id, "job failed: {msg}");
                        let retry = ctx.queue.fail(&job, &msg).await.unwrap_or(false);
                        on_failure(&ctx, &job, &msg, retry).await;
                    }
                }
            }
            Ok(None) => {
                tokio::select! {
                    _ = ctx.queue.notify.notified() => {}
                    _ = tokio::time::sleep(POLL_EVERY) => {}
                }
            }
            Err(e) => {
                tracing::error!("claim job: {e:#}");
                tokio::time::sleep(POLL_EVERY).await;
            }
        }
    }
}

async fn handle(ctx: &WorkerCtx, job: &JobRow) -> anyhow::Result<()> {
    match job.kind.as_str() {
        KIND_PROCESS_VERSION => {
            let p: ProcessVersionPayload = serde_json::from_value(job.payload.clone())?;
            process_version(ctx, p.version_id).await
        }
        other => anyhow::bail!("unknown job kind {other}"),
    }
}

async fn on_failure(ctx: &WorkerCtx, job: &JobRow, msg: &str, retry: bool) {
    if job.kind != KIND_PROCESS_VERSION {
        return;
    }
    let Ok(p) = serde_json::from_value::<ProcessVersionPayload>(job.payload.clone()) else { return };
    let status = if retry { VersionStatus::Pending } else { VersionStatus::Failed };
    let _ = set_status(&ctx.db, p.version_id, status, Some(msg)).await;
}

async fn set_status(db: &DatabaseConnection, id: Uuid, status: VersionStatus, error: Option<&str>) -> anyhow::Result<()> {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE asset_versions SET status = $2, error = $3 WHERE id = $1",
        [id.into(), status.as_str().into(), error.map(str::to_string).into()],
    ))
    .await?;
    Ok(())
}

/// Download the relevant files, run the pipeline, store the artifacts and
/// the manifest.
pub async fn process_version(ctx: &WorkerCtx, version_id: Uuid) -> anyhow::Result<()> {
    let version = asset_versions::Entity::find_by_id(version_id)
        .one(&ctx.db)
        .await?
        .context("version vanished")?;
    set_status(&ctx.db, version.id, VersionStatus::Processing, None).await?;

    let files: Vec<FileEntry> = version_files::Entity::find()
        .filter(version_files::Column::VersionId.eq(version.id))
        .all(&ctx.db)
        .await?
        .iter()
        .map(|f| f.to_entry())
        .collect();

    let tmp_root = ctx.cfg.temp_root().join("hangar-work");
    tokio::fs::create_dir_all(&tmp_root).await?;
    let work = tempfile::Builder::new().prefix("v-").tempdir_in(&tmp_root)?;

    let relevant: Vec<FileEntry> = files.iter().filter(|f| hangar_pipeline::is_relevant(&f.path)).cloned().collect();
    for f in &relevant {
        // Paths were normalised on commit, so joining cannot escape `work`.
        ctx.store
            .download_to(&f.hash, &work.path().join(&f.path))
            .await
            .map_err(|e| anyhow::anyhow!("download {}: {e}", f.path))?;
    }

    let root = work.path().to_path_buf();
    let cfg = ctx.pipeline.clone();
    let output = tokio::task::spawn_blocking(move || {
        let input = ProcessInput { root: &root, files: &relevant };
        hangar_pipeline::process(&input, &cfg)
    })
    .await??;

    // Store artifacts (deduplicated by hash) and record renditions.
    ctx.db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM renditions WHERE version_id = $1",
            [version.id.into()],
        ))
        .await?;
    for artifact in output.artifacts {
        let (kind, key) = match &artifact.role {
            ArtifactRole::LodGlb(level) => ("lod_glb", level.to_string()),
            ArtifactRole::Thumbnail => ("thumbnail", String::new()),
            ArtifactRole::TexturePreview(path) => ("texture_preview", path.clone()),
        };
        let expected = artifact.hash.clone();
        let hash = ctx
            .store
            .put_bytes(artifact.bytes, artifact.mime)
            .await
            .map_err(|e| anyhow::anyhow!("store artifact: {e}"))?;
        debug_assert_eq!(hash, expected);
        ctx.db
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO renditions(version_id, kind, key, blob_hash) VALUES ($1, $2, $3, $4)
                 ON CONFLICT (version_id, kind, key) DO UPDATE SET blob_hash = EXCLUDED.blob_hash",
                [version.id.into(), kind.into(), key.into(), hash.into()],
            ))
            .await?;
    }

    let manifest = serde_json::to_value(&output.manifest)?;
    ctx.db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE asset_versions SET status = 'ready', error = NULL, preview = $2, processed_at = now()
             WHERE id = $1",
            [version.id.into(), manifest.into()],
        ))
        .await?;
    tracing::info!(
        version = %version.id,
        lods = output.manifest.lods.len(),
        variants = output.manifest.variants.len(),
        warnings = output.manifest.warnings.len(),
        "version processed"
    );
    Ok(())
}
