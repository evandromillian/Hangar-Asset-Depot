//! Project-level authorisation.

use crate::auth::CurrentUser;
use crate::entities::{asset_versions, assets, project_members, projects};
use crate::error::{Ctx, Result, forbidden, not_found};
use hangar_core::Role;
use sea_orm::{DatabaseConnection, EntityTrait};
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
