//! Project-level authorisation.

use crate::auth::CurrentUser;
use crate::entities::{asset_versions, assets, project_members, projects};
use crate::error::{Ctx, Result, bad_request, forbidden, not_found};
use hangar_core::{Role, is_valid_hash};
use sea_orm::{DatabaseConnection, DbBackend, EntityTrait, FromQueryResult, Statement};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    Read,
    Write,
    Admin,
}

/// Role of `user` in `project_id`. Global admins act as owners everywhere.
pub async fn project_role(db: &DatabaseConnection, user: &CurrentUser, project_id: Uuid) -> Result<Option<Role>> {
    let member = project_members::Entity::find_by_id((project_id, user.0.id))
        .one(db)
        .await
        .ctx("load membership")?;
    let role = member.and_then(|m| m.role.parse().ok());
    Ok(if user.0.is_admin { Some(Role::Owner) } else { role })
}

fn check(role: Option<Role>, need: Need) -> Result<Role> {
    // Non-members get 404 so project existence isn't leaked.
    let role = role.ok_or_else(|| not_found("projeto não encontrado"))?;
    let ok = match need {
        Need::Read => true,
        Need::Write => role.can_write(),
        Need::Admin => role.can_admin(),
    };
    if ok { Ok(role) } else { Err(forbidden("permissão insuficiente neste projeto")) }
}

pub async fn project(db: &DatabaseConnection, user: &CurrentUser, id: Uuid, need: Need) -> Result<(projects::Model, Role)> {
    let p = projects::Entity::find_by_id(id)
        .one(db)
        .await
        .ctx("load project")?
        .ok_or_else(|| not_found("projeto não encontrado"))?;
    let role = check(project_role(db, user, id).await?, need)?;
    Ok((p, role))
}

pub async fn asset(db: &DatabaseConnection, user: &CurrentUser, id: Uuid, need: Need) -> Result<(assets::Model, Role)> {
    let a = assets::Entity::find_by_id(id)
        .one(db)
        .await
        .ctx("load asset")?
        .ok_or_else(|| not_found("asset não encontrado"))?;
    let role = check(project_role(db, user, a.project_id).await?, need)
        .map_err(|e| if need == Need::Read { not_found("asset não encontrado") } else { e })?;
    Ok((a, role))
}

pub async fn version(
    db: &DatabaseConnection,
    user: &CurrentUser,
    id: Uuid,
    need: Need,
) -> Result<(asset_versions::Model, assets::Model, Role)> {
    let v = asset_versions::Entity::find_by_id(id)
        .one(db)
        .await
        .ctx("load version")?
        .ok_or_else(|| not_found("versão não encontrada"))?;
    let (a, role) = asset(db, user, v.asset_id, need).await?;
    Ok((v, a, role))
}

/// SQL condition: blob `h` is readable by user `$2`.
const BLOB_READABLE: &str = "
    EXISTS (SELECT 1 FROM blob_uploads u WHERE u.blob_hash = h AND u.user_id = $2)
    OR EXISTS (SELECT 1 FROM version_files f
               JOIN asset_versions v ON v.id = f.version_id
               JOIN assets a ON a.id = v.asset_id
               JOIN project_members m ON m.project_id = a.project_id AND m.user_id = $2
               WHERE f.blob_hash = h)
    OR EXISTS (SELECT 1 FROM renditions r
               JOIN asset_versions v ON v.id = r.version_id
               JOIN assets a ON a.id = v.asset_id
               JOIN project_members m ON m.project_id = a.project_id AND m.user_id = $2
               WHERE r.blob_hash = h)
    -- Reference skeletons are a system setting: every user sees their thumbnail.
    OR EXISTS (SELECT 1 FROM skeletons s
               JOIN assets a ON a.id = s.asset_id
               JOIN renditions r ON r.version_id = a.head_version_id AND r.kind = 'thumbnail'
               WHERE r.blob_hash = h)";

/// Which of `hashes` `user` can neither read nor reference: blobs are readable
/// through a version (file or rendition) of a project the user is a member of,
/// or when the user uploaded their bytes. Unknown hashes are included; admins
/// can use every stored blob.
pub async fn unusable_blobs(db: &DatabaseConnection, user: &CurrentUser, hashes: &[String]) -> Result<Vec<String>> {
    if let Some(h) = hashes.iter().find(|h| !is_valid_hash(h)) {
        return Err(bad_request(format!("hash inválido: {h}")));
    }
    #[derive(FromQueryResult)]
    struct Row {
        hash: String,
    }
    let mut values = vec![serde_json::json!(hashes).into()];
    let cond = if user.0.is_admin {
        "EXISTS (SELECT 1 FROM blobs b WHERE b.hash = h)"
    } else {
        values.push(user.0.id.into());
        BLOB_READABLE
    };
    let rows = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("SELECT DISTINCT h AS hash FROM jsonb_array_elements_text($1) AS t(h) WHERE NOT ({cond}) ORDER BY h"),
        values,
    ))
    .all(db)
    .await
    .ctx("check blob access")?;
    Ok(rows.into_iter().map(|r| r.hash).collect())
}
