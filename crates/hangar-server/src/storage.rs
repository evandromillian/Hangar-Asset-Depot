//! Content-addressed blob store on top of OpenDAL.
//!
//! Layout: `blobs/ab/cd/abcdef…` (BLAKE3 hex). The `blobs` table is the source
//! of truth: a row is only inserted after the object was fully written, so a
//! crashed upload never becomes visible.

use crate::entities::blobs;
use crate::error::{Ctx, Result, bad_request, not_found};
use bytes::Bytes;
use futures::{Stream, StreamExt};
use hangar_core::{BlobUploaded, is_valid_hash};
use opendal::Operator;
use sea_orm::{ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, QueryFilter, Statement};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const CHUNK: usize = 8 * 1024 * 1024;

#[derive(Clone)]
pub struct BlobStore {
    op: Operator,
    db: DatabaseConnection,
    tmp: PathBuf,
}

pub fn key(hash: &str) -> String {
    format!("blobs/{}/{}/{}", &hash[0..2], &hash[2..4], hash)
}

impl BlobStore {
    pub fn new(op: Operator, db: DatabaseConnection, tmp: PathBuf) -> Self {
        Self { op, db, tmp }
    }

    pub fn temp_dir(&self) -> &Path {
        &self.tmp
    }

    pub async fn get(&self, hash: &str) -> Result<Option<blobs::Model>> {
        blobs::Entity::find_by_id(hash.to_string()).one(&self.db).await.ctx("load blob")
    }

    /// Which of `hashes` the server does not have yet.
    pub async fn missing(&self, hashes: &[String]) -> Result<Vec<String>> {
        for h in hashes {
            if !is_valid_hash(h) {
                return Err(bad_request(format!("hash inválido: {h}")));
            }
        }
        let mut uniq: Vec<String> = hashes.to_vec();
        uniq.sort();
        uniq.dedup();
        let mut present = std::collections::HashSet::new();
        for chunk in uniq.chunks(1000) {
            let rows = blobs::Entity::find()
                .filter(blobs::Column::Hash.is_in(chunk.iter().cloned()))
                .all(&self.db)
                .await
                .ctx("query blobs")?;
            present.extend(rows.into_iter().map(|r| r.hash));
        }
        Ok(uniq.into_iter().filter(|h| !present.contains(h)).collect())
    }

    /// Streams an upload to a temp file while hashing it, verifies the hash
    /// against `expected`, then moves it into the object store.
    pub async fn put_stream<S, E>(&self, expected: &str, mime: Option<String>, mut body: S) -> Result<BlobUploaded>
    where
        S: Stream<Item = std::result::Result<Bytes, E>> + Unpin,
        E: std::fmt::Display,
    {
        if !is_valid_hash(expected) {
            return Err(bad_request("hash inválido"));
        }
        if let Some(b) = self.get(expected).await? {
            // Drain nothing: the client should have asked /missing first.
            return Ok(BlobUploaded { hash: b.hash, size: b.size, created: false });
        }

        tokio::fs::create_dir_all(&self.tmp).await.ctx("create temp dir")?;
        let tmp = tempfile::NamedTempFile::new_in(&self.tmp).ctx("create temp file")?;
        let (std_file, tmp_path) = tmp.into_parts();
        let mut file = tokio::fs::File::from_std(std_file);
        let mut hasher = blake3::Hasher::new();
        let mut size: u64 = 0;
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|e| bad_request(format!("upload interrompido: {e}")))?;
            hasher.update(&chunk);
            size += chunk.len() as u64;
            file.write_all(&chunk).await.ctx("write temp file")?;
        }
        file.flush().await.ctx("flush temp file")?;
        drop(file);

        let actual = hasher.finalize().to_hex().to_string();
        if actual != expected {
            return Err(bad_request(format!("hash não confere: esperado {expected}, recebido {actual}")));
        }
        self.store_file(&actual, &tmp_path, size, mime).await?;
        Ok(BlobUploaded { hash: actual, size: size as i64, created: true })
    }

    /// Stores an in-memory buffer (pipeline artifacts). Returns its hash.
    pub async fn put_bytes(&self, bytes: Vec<u8>, mime: &str) -> Result<String> {
        let hash = blake3::hash(&bytes).to_hex().to_string();
        if self.get(&hash).await?.is_some() {
            return Ok(hash);
        }
        let size = bytes.len() as i64;
        self.op.write(&key(&hash), bytes).await.ctx("write blob")?;
        self.insert_row(&hash, size, Some(mime.to_string())).await?;
        Ok(hash)
    }

    async fn store_file(&self, hash: &str, path: &Path, size: u64, mime: Option<String>) -> Result<()> {
        let mut writer = self.op.writer(&key(hash)).await.ctx("open blob writer")?;
        let mut file = tokio::fs::File::open(path).await.ctx("reopen temp file")?;
        let mut buf = vec![0u8; CHUNK];
        loop {
            let n = file.read(&mut buf).await.ctx("read temp file")?;
            if n == 0 {
                break;
            }
            writer.write(Bytes::copy_from_slice(&buf[..n])).await.ctx("write blob")?;
        }
        writer.close().await.ctx("close blob writer")?;
        self.insert_row(hash, size as i64, mime).await
    }

    async fn insert_row(&self, hash: &str, size: i64, mime: Option<String>) -> Result<()> {
        self.db
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO blobs(hash, size, mime) VALUES ($1, $2, $3) ON CONFLICT (hash) DO NOTHING",
                [hash.into(), size.into(), mime.into()],
            ))
            .await
            .ctx("insert blob")?;
        Ok(())
    }

    /// Byte stream of a blob.
    pub async fn stream(
        &self,
        hash: &str,
    ) -> Result<impl Stream<Item = std::result::Result<Bytes, std::io::Error>> + Send + 'static> {
        if !is_valid_hash(hash) {
            return Err(not_found("blob"));
        }
        let reader = self.op.reader(&key(hash)).await.ctx("open blob")?;
        let stream = reader.into_bytes_stream(..).await.ctx("stream blob")?;
        Ok(stream)
    }

    /// Deletes blobs no version or rendition references any more. Blobs younger
    /// than `grace_hours` are kept so in-flight uploads (blobs sent, commit not
    /// yet made) survive. Returns (count, bytes) freed.
    pub async fn gc(&self, grace_hours: i64) -> Result<(u64, i64)> {
        #[derive(sea_orm::FromQueryResult)]
        struct Gone {
            hash: String,
            size: i64,
        }
        use sea_orm::FromQueryResult;
        let gone = Gone::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM blobs b
             WHERE b.created_at < now() - make_interval(hours => $1::int)
               AND NOT EXISTS (SELECT 1 FROM version_files f WHERE f.blob_hash = b.hash)
               AND NOT EXISTS (SELECT 1 FROM renditions r WHERE r.blob_hash = b.hash)
             RETURNING b.hash, b.size",
            [(grace_hours as i32).into()],
        ))
        .all(&self.db)
        .await
        .ctx("gc blobs")?;
        let mut freed = 0i64;
        for g in &gone {
            if let Err(e) = self.op.delete(&key(&g.hash)).await {
                tracing::warn!(hash = %g.hash, "gc: delete object failed: {e}");
            }
            freed += g.size;
        }
        Ok((gone.len() as u64, freed))
    }

    /// Copies a blob to a local file (creating parent directories).
    pub async fn download_to(&self, hash: &str, dest: &Path) -> Result<()> {
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await.ctx("create dirs")?;
        }
        let mut stream = Box::pin(self.stream(hash).await?);
        let mut file = tokio::fs::File::create(dest).await.ctx("create file")?;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.ctx("read blob chunk")?;
            file.write_all(&chunk).await.ctx("write file")?;
        }
        file.flush().await.ctx("flush file")?;
        Ok(())
    }
}
