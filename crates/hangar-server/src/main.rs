//! Hangar server — digital asset management for 3D/game content.

mod access;
mod archive;
mod auth;
mod config;
mod db;
mod dto;
mod entities;
mod error;
mod jobs;
mod routes;
mod storage;

use config::HangarConfig;
use jobs::{JobQueue, WorkerCtx};
use storage::BlobStore;
use summer::app::AppBuilder;
use summer::config::ConfigRegistry;
use summer::plugin::{ComponentRegistry, MutableComponentRegistry, Plugin};
use summer::{App, async_trait, auto_config};
use summer_opendal::{OpenDALPlugin, Op};
use summer_sa_token::{PathAuthBuilder, SaTokenAuthConfigurator, SaTokenPlugin};
use summer_sea_orm::{DbConn, SeaOrmPlugin};
use summer_web::{WebConfigurator, WebPlugin};

/// Wires Hangar's own components once the database and storage plugins are up:
/// runs migrations, registers `BlobStore` / `JobQueue` and schedules the workers.
struct HangarPlugin;

#[async_trait]
impl Plugin for HangarPlugin {
    async fn build(&self, app: &mut AppBuilder) {
        let cfg = app.get_config::<HangarConfig>().expect("[hangar] config");
        let db: DbConn = app.get_component::<DbConn>().expect("SeaOrmPlugin must be added");
        let op: Op = app.get_component::<Op>().expect("OpenDALPlugin must be added");

        db::migrate(&db).await.expect("database migration failed");

        let store = BlobStore::new(op, db.clone(), cfg.temp_root().join("hangar-uploads"));
        let queue = JobQueue::new(db.clone());
        let worker = WorkerCtx {
            db: db.clone(),
            store: store.clone(),
            queue: queue.clone(),
            pipeline: cfg.pipeline(),
            cfg,
        };
        app.add_component(store);
        app.add_component(queue);
        app.add_scheduler(move |_app| Box::new(jobs::run_workers(worker)));
    }

    fn name(&self) -> &str {
        "hangar::HangarPlugin"
    }

    fn dependencies(&self) -> Vec<&str> {
        vec![std::any::type_name::<SeaOrmPlugin>(), std::any::type_name::<OpenDALPlugin>()]
    }
}

#[auto_config(WebConfigurator)]
#[tokio::main]
async fn main() {
    App::new()
        .add_plugin(SeaOrmPlugin)
        .add_plugin(OpenDALPlugin)
        .add_plugin(SaTokenPlugin)
        .add_plugin(HangarPlugin)
        .add_plugin(WebPlugin)
        .sa_token_configure(
            PathAuthBuilder::new()
                .include("/api/**")
                .exclude("/api/health")
                .exclude("/api/setup")
                .exclude("/api/auth/login")
                .exclude("/api/auth/register"),
        )
        .run()
        .await
}
