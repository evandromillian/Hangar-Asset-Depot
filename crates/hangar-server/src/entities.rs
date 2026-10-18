//! SeaORM entities. The schema itself lives in `migrations/*.sql`.

pub mod users {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "users")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub username: String,
        pub display_name: String,
        pub password_hash: String,
        pub is_admin: bool,
        pub created_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}

    impl Model {
        pub fn to_dto(&self) -> hangar_core::UserDto {
            hangar_core::UserDto {
                id: self.id,
                username: self.username.clone(),
                display_name: self.display_name.clone(),
                is_admin: self.is_admin,
                created_at: self.created_at,
            }
        }
    }
}

pub mod projects {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "projects")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub slug: String,
        pub name: String,
        pub description: String,
        pub created_by: Option<Uuid>,
        pub created_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod project_members {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "project_members")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub project_id: Uuid,
        #[sea_orm(primary_key, auto_increment = false)]
        pub user_id: Uuid,
        pub role: String,
        pub added_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod blobs {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "blobs")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub hash: String,
        pub size: i64,
        pub mime: Option<String>,
        pub created_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

pub mod assets {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "assets")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub project_id: Uuid,
        pub slug: String,
        pub name: String,
        pub kind: String,
        pub description: String,
        #[sea_orm(column_type = "JsonBinary")]
        pub tags: Json,
        #[sea_orm(column_type = "JsonBinary")]
        pub source: Json,
        pub head_version_id: Option<Uuid>,
        pub created_by: Option<Uuid>,
        pub created_at: DateTimeUtc,
        pub updated_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}

    impl Model {
        pub fn tags_vec(&self) -> Vec<String> {
            serde_json::from_value(self.tags.clone()).unwrap_or_default()
        }
        pub fn source_struct(&self) -> hangar_core::AssetSource {
            serde_json::from_value(self.source.clone()).unwrap_or_default()
        }
        pub fn kind_enum(&self) -> hangar_core::AssetKind {
            self.kind.parse().unwrap_or(hangar_core::AssetKind::Other)
        }
    }
}

pub mod asset_versions {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "asset_versions")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub asset_id: Uuid,
        pub number: i32,
        pub parent_id: Option<Uuid>,
        pub message: String,
        pub author_id: Option<Uuid>,
        pub status: String,
        pub error: Option<String>,
        #[sea_orm(column_type = "JsonBinary", nullable)]
        pub preview: Option<Json>,
        pub file_count: i32,
        pub total_size: i64,
        pub created_at: DateTimeUtc,
        pub processed_at: Option<DateTimeUtc>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}

    impl Model {
        pub fn status_enum(&self) -> hangar_core::VersionStatus {
            self.status.parse().unwrap_or(hangar_core::VersionStatus::Pending)
        }
        pub fn preview_manifest(&self) -> Option<hangar_core::PreviewManifest> {
            self.preview.clone().and_then(|v| serde_json::from_value(v).ok())
        }
    }
}

pub mod version_files {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "version_files")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub version_id: Uuid,
        #[sea_orm(primary_key, auto_increment = false)]
        pub path: String,
        pub blob_hash: String,
        pub size: i64,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}

    impl Model {
        pub fn to_entry(&self) -> hangar_core::FileEntry {
            hangar_core::FileEntry { path: self.path.clone(), hash: self.blob_hash.clone(), size: self.size }
        }
    }
}

pub mod skeletons {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "skeletons")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub slug: String,
        pub name: String,
        pub description: String,
        pub asset_id: Uuid,
        pub created_by: Option<Uuid>,
        pub created_at: DateTimeUtc,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}
