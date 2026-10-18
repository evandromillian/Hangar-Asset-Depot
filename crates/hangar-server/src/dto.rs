//! Entity → DTO conversions that need extra lookups.

use crate::entities::{asset_versions, assets, users};
use crate::error::{Ctx, Result};
use hangar_core::{AssetDetail, AssetSummary, Role, VersionBrief, VersionDto};
use sea_orm::{ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter};
use std::collections::HashMap;
use uuid::Uuid;

pub async fn usernames<C: ConnectionTrait>(db: &C, ids: impl IntoIterator<Item = Uuid>) -> Result<HashMap<Uuid, String>> {
    let mut ids: Vec<Uuid> = ids.into_iter().collect();
    ids.sort();
    ids.dedup();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = users::Entity::find()
        .filter(users::Column::Id.is_in(ids))
        .all(db)
        .await
        .ctx("load usernames")?;
    Ok(rows.into_iter().map(|u| (u.id, u.username)).collect())
}

pub fn version_dto(v: asset_versions::Model, head: Option<Uuid>, names: &HashMap<Uuid, String>) -> VersionDto {
    let preview = v.preview_manifest();
    VersionDto {
        id: v.id,
        asset_id: v.asset_id,
        number: v.number,
        parent_id: v.parent_id,
        message: v.message.clone(),
        author: v.author_id.and_then(|id| names.get(&id).cloned()),
        status: v.status_enum(),
        error: v.error.clone(),
        file_count: v.file_count,
        total_size: v.total_size,
        is_head: head == Some(v.id),
        created_at: v.created_at,
        processed_at: v.processed_at,
        preview,
    }
}

pub async fn version_dto_one(db: &DatabaseConnection, v: asset_versions::Model, head: Option<Uuid>) -> Result<VersionDto> {
    let names = usernames(db, v.author_id).await?;
    Ok(version_dto(v, head, &names))
}

pub async fn asset_detail(db: &DatabaseConnection, a: assets::Model, role: Option<Role>) -> Result<AssetDetail> {
    let head = match a.head_version_id {
        Some(id) => asset_versions::Entity::find_by_id(id).one(db).await.ctx("load head")?,
        None => None,
    };
    let version_count = asset_versions::Entity::find()
        .filter(asset_versions::Column::AssetId.eq(a.id))
        .count(db)
        .await
        .ctx("count versions")? as i64;
    let names = usernames(db, a.created_by).await?;
    let thumbnail_url = head
        .as_ref()
        .and_then(|h| h.preview_manifest())
        .and_then(|p| p.thumbnail_hash)
        .map(|h| hangar_core::blob_url(&h));
    Ok(AssetDetail {
        summary: AssetSummary {
            id: a.id,
            project_id: a.project_id,
            slug: a.slug.clone(),
            name: a.name.clone(),
            kind: a.kind_enum(),
            tags: a.tags_vec(),
            head: head.as_ref().map(|h| VersionBrief { id: h.id, number: h.number, status: h.status_enum() }),
            version_count,
            thumbnail_url,
            updated_at: a.updated_at,
        },
        description: a.description.clone(),
        source: a.source_struct(),
        created_by: a.created_by.and_then(|id| names.get(&id).cloned()),
        created_at: a.created_at,
        my_role: role,
    })
}
