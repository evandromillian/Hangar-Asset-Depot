use crate::access::{self, Need};
use crate::auth::CurrentUser;
use crate::entities::{project_members, projects, users};
use crate::error::{Ctx, Result, bad_request, not_found, slugify};
use hangar_core::{AddMemberRequest, CreateProjectRequest, MemberDto, ProjectDto, Role};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, FromQueryResult, QueryFilter, Set,
    Statement, TransactionTrait,
};
use summer_web::axum::http::StatusCode;
use summer_web::extractor::{Component, Json, Path};
use summer_web::{delete, get, post};
use uuid::Uuid;

#[derive(FromQueryResult)]
struct ProjectRow {
    id: Uuid,
    slug: String,
    name: String,
    description: String,
    created_at: chrono::DateTime<chrono::Utc>,
    my_role: Option<String>,
    asset_count: i64,
}

impl ProjectRow {
    fn into_dto(self, is_admin: bool) -> ProjectDto {
        let role = self.my_role.and_then(|r| r.parse().ok()).or(is_admin.then_some(Role::Owner));
        ProjectDto {
            id: self.id,
            slug: self.slug,
            name: self.name,
            description: self.description,
            my_role: role,
            asset_count: self.asset_count,
            created_at: self.created_at,
        }
    }
}

const PROJECT_SELECT: &str = "SELECT p.id, p.slug, p.name, p.description, p.created_at, m.role AS my_role,
        (SELECT count(*) FROM assets a WHERE a.project_id = p.id) AS asset_count
     FROM projects p
     LEFT JOIN project_members m ON m.project_id = p.id AND m.user_id = $1";

async fn load_project_dto(db: &DatabaseConnection, user: &CurrentUser, id: Uuid) -> Result<ProjectDto> {
    let row = ProjectRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("{PROJECT_SELECT} WHERE p.id = $2"),
        [user.0.id.into(), id.into()],
    ))
    .one(db)
    .await
    .ctx("load project")?
    .ok_or_else(|| not_found("projeto não encontrado"))?;
    Ok(row.into_dto(user.0.is_admin))
}

#[get("/api/projects")]
async fn list_projects(user: CurrentUser, Component(db): Component<DatabaseConnection>) -> Result<Json<Vec<ProjectDto>>> {
    let rows = ProjectRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("{PROJECT_SELECT} WHERE $2 OR m.user_id IS NOT NULL ORDER BY lower(p.name)"),
        [user.0.id.into(), user.0.is_admin.into()],
    ))
    .all(&db)
    .await
    .ctx("list projects")?;
    Ok(Json(rows.into_iter().map(|r| r.into_dto(user.0.is_admin)).collect()))
}

#[post("/api/projects")]
async fn create_project(
    user: CurrentUser,
    Component(db): Component<DatabaseConnection>,
    Json(req): Json<CreateProjectRequest>,
) -> Result<Json<ProjectDto>> {
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(bad_request("nome obrigatório"));
    }
    let base = slugify(req.slug.as_deref().unwrap_or(&name));
    let mut slug = base.clone();
    let mut n = 2;
    while projects::Entity::find()
        .filter(projects::Column::Slug.eq(&slug))
        .one(&db)
        .await
        .ctx("check slug")?
        .is_some()
    {
        slug = format!("{base}-{n}");
        n += 1;
    }

    let id = Uuid::now_v7();
    let now = chrono::Utc::now();
    let txn = db.begin().await.ctx("begin")?;
    projects::ActiveModel {
        id: Set(id),
        slug: Set(slug),
        name: Set(name),
        description: Set(req.description.trim().to_string()),
        created_by: Set(Some(user.0.id)),
        created_at: Set(now),
    }
    .insert(&txn)
    .await
    .ctx("insert project")?;
    project_members::ActiveModel {
        project_id: Set(id),
        user_id: Set(user.0.id),
        role: Set(Role::Owner.as_str().into()),
        added_at: Set(now),
    }
    .insert(&txn)
    .await
    .ctx("insert owner")?;
    txn.commit().await.ctx("commit")?;

    Ok(Json(load_project_dto(&db, &user, id).await?))
}

#[get("/api/projects/{id}")]
async fn get_project(
    user: CurrentUser,
    Component(db): Component<DatabaseConnection>,
    Path(id): Path<Uuid>,
) -> Result<Json<ProjectDto>> {
    access::project(&db, &user, id, Need::Read).await?;
    Ok(Json(load_project_dto(&db, &user, id).await?))
}

#[get("/api/projects/{id}/members")]
async fn list_members(
    user: CurrentUser,
    Component(db): Component<DatabaseConnection>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<MemberDto>>> {
    access::project(&db, &user, id, Need::Read).await?;
    Ok(Json(members_with_users(&db, id).await?))
}

async fn members_with_users(db: &DatabaseConnection, project_id: Uuid) -> Result<Vec<MemberDto>> {
    let members = project_members::Entity::find()
        .filter(project_members::Column::ProjectId.eq(project_id))
        .all(db)
        .await
        .ctx("list members")?;
    let ids: Vec<Uuid> = members.iter().map(|m| m.user_id).collect();
    let users = users::Entity::find()
        .filter(users::Column::Id.is_in(ids))
        .all(db)
        .await
        .ctx("load users")?;
    let mut out: Vec<MemberDto> = members
        .into_iter()
        .filter_map(|m| {
            let u = users.iter().find(|u| u.id == m.user_id)?;
            Some(MemberDto { user: u.to_dto(), role: m.role.parse().ok()? })
        })
        .collect();
    out.sort_by(|a, b| a.user.username.cmp(&b.user.username));
    Ok(out)
}

#[post("/api/projects/{id}/members")]
async fn add_member(
    user: CurrentUser,
    Component(db): Component<DatabaseConnection>,
    Path(id): Path<Uuid>,
    Json(req): Json<AddMemberRequest>,
) -> Result<Json<MemberDto>> {
    access::project(&db, &user, id, Need::Admin).await?;
    let target = users::Entity::find()
        .filter(users::Column::Username.eq(req.username.trim().to_lowercase()))
        .one(&db)
        .await
        .ctx("load user")?
        .ok_or_else(|| not_found("usuário não encontrado"))?;
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO project_members(project_id, user_id, role) VALUES ($1, $2, $3)
         ON CONFLICT (project_id, user_id) DO UPDATE SET role = EXCLUDED.role",
        [id.into(), target.id.into(), req.role.as_str().into()],
    ))
    .await
    .ctx("upsert member")?;
    Ok(Json(MemberDto { user: target.to_dto(), role: req.role }))
}

#[delete("/api/projects/{id}/members/{user_id}")]
async fn remove_member(
    user: CurrentUser,
    Component(db): Component<DatabaseConnection>,
    Path((id, user_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode> {
    access::project(&db, &user, id, Need::Admin).await?;
    let owners = project_members::Entity::find()
        .filter(project_members::Column::ProjectId.eq(id))
        .filter(project_members::Column::Role.eq("owner"))
        .all(&db)
        .await
        .ctx("load owners")?;
    if owners.len() == 1 && owners[0].user_id == user_id {
        return Err(bad_request("o projeto precisa de pelo menos um owner"));
    }
    project_members::Entity::delete_by_id((id, user_id))
        .exec(&db)
        .await
        .ctx("delete member")?;
    Ok(StatusCode::NO_CONTENT)
}
