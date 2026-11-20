pub mod accounts;
pub mod archives;
pub mod assets;
pub mod auth;
pub mod content;
mod copies;
pub mod entries;
pub mod instance;
pub mod middleware;
pub mod mutations;
pub mod removals;
pub mod setup;
mod shares;
pub mod status;
pub mod uploads;
pub mod usage;

use crate::domain::{
    authz::PasswordLimiter, coordinator::MutationCoordinator, upload::UploadStorage,
};
use crate::persistence::sqlite::SqliteWorkers;
use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{get, post, put},
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

pub struct AnonymousSession {
    pub csrf_verifier: [u8; 32],
    pub expires_at: i64,
}
pub type AnonymousSessions = HashMap<[u8; 32], AnonymousSession>;

#[derive(Clone)]
pub struct AuthState {
    pub workers: SqliteWorkers,
    pub origin: String,
    pub setup_authorization: String,
    pub limiter: PasswordLimiter,
    pub anonymous: Arc<Mutex<AnonymousSessions>>,
    pub coordinator: MutationCoordinator,
    pub cleanup_wake: Arc<tokio::sync::Notify>,
    pub storage: Option<Arc<UploadStorage>>,
    pub instance_settings: Option<Arc<crate::persistence::instance::InstanceSettings>>,
    share_downloads: Arc<tokio::sync::Semaphore>,
    pub archive_tickets: Arc<Mutex<archives::ArchiveTickets>>,
    /// Held by each live archive body and payload verification; tests count the holders.
    #[cfg(feature = "test-harness")]
    pub archive_activity: Arc<()>,
    #[cfg(feature = "test-harness")]
    pub upload_cancel_test_barrier: Option<Arc<uploads::UploadCancelTestBarrier>>,
    #[cfg(feature = "test-harness")]
    pub upload_commit_test_barrier: Option<Arc<uploads::UploadCommitTestBarrier>>,
    #[cfg(feature = "test-harness")]
    pub copy_commit_hook: Option<Arc<dyn Fn() -> rusqlite::Result<()> + Send + Sync>>,
    #[cfg(feature = "test-harness")]
    pub removal_commit_hook:
        Option<Arc<dyn Fn(&'static str) -> rusqlite::Result<()> + Send + Sync>>,
}

impl AuthState {
    pub fn new(workers: SqliteWorkers, origin: String) -> Self {
        Self::new_with_setup(workers, origin, String::new())
    }

    pub fn new_with_setup(
        workers: SqliteWorkers,
        origin: String,
        setup_authorization: String,
    ) -> Self {
        let coordinator = MutationCoordinator::new(workers.clone());
        Self {
            workers,
            origin,
            setup_authorization,
            limiter: PasswordLimiter::default(),
            anonymous: Arc::default(),
            coordinator,
            cleanup_wake: Arc::default(),
            storage: None,
            instance_settings: None,
            share_downloads: Arc::new(tokio::sync::Semaphore::new(shares::DOWNLOAD_SLOTS)),
            archive_tickets: Arc::default(),
            #[cfg(feature = "test-harness")]
            archive_activity: Arc::default(),
            #[cfg(feature = "test-harness")]
            upload_cancel_test_barrier: None,
            #[cfg(feature = "test-harness")]
            upload_commit_test_barrier: None,
            #[cfg(feature = "test-harness")]
            removal_commit_hook: None,
            #[cfg(feature = "test-harness")]
            copy_commit_hook: None,
        }
    }

    pub fn with_storage_root(
        mut self,
        root: &std::path::Path,
    ) -> Result<Self, crate::domain::upload::StorageOpenError> {
        self.storage = Some(Arc::new(UploadStorage::open(root)?));
        self.instance_settings = Some(Arc::new(
            crate::persistence::instance::InstanceSettings::open(root)
                .map_err(|_| crate::domain::upload::StorageOpenError::InstanceSettings)?,
        ));
        Ok(self)
    }

    pub fn start_cleanup(&self) -> Option<crate::persistence::cleanup::Runner> {
        self.storage.as_ref().map(|storage| {
            crate::persistence::cleanup::Runner::start(
                self.workers.clone(),
                Arc::clone(&storage.payloads),
                Arc::clone(&self.cleanup_wake),
            )
        })
    }

    #[cfg(feature = "test-harness")]
    pub fn with_upload_cancel_test_barrier(
        mut self,
        barrier: Arc<uploads::UploadCancelTestBarrier>,
    ) -> Self {
        self.upload_cancel_test_barrier = Some(barrier);
        self
    }
}

pub fn router(state: AuthState) -> Router {
    Router::new()
        .route("/folders/{folder_id}/path", get(entries::folder_path))
        .route(
            "/folders/{folder_id}/children",
            get(entries::listing_folder).post(entries::create_in_folder),
        )
        .route(
            "/folders/children",
            get(entries::listing).post(entries::create),
        )
        .route(
            "/entries/{id}",
            axum::routing::patch(entries::rename).delete(entries::trash),
        )
        .route("/trash", get(entries::trash_listing))
        .route("/trash/removals/preview", post(removals::preview))
        .route("/trash/removals", post(removals::execute))
        .route("/trash/removals/{id}", get(removals::status))
        .route("/entries/search", get(entries::search))
        .route("/entries/{id}/move", post(entries::move_entry))
        .route("/entries/{id}/copy-ticket", post(copies::prepare))
        .route("/copies", post(copies::execute))
        .route("/copies/status", post(copies::status))
        .route("/trash/{id}/restore", axum::routing::post(entries::restore))
        .route("/instance", get(instance::get).patch(instance::update))
        .route(
            "/instance/appearance",
            get(instance::appearance)
                .put(instance::update_appearance)
                .layer(DefaultBodyLimit::max(
                    crate::persistence::instance::APPEARANCE_BODY_BYTES,
                )),
        )
        .route(
            "/instance/appearance/assets/{digest}",
            get(instance::appearance_asset),
        )
        .route("/setup", get(setup::status))
        .route("/setup/admin", post(setup::create))
        .route("/auth/csrf", get(auth::csrf))
        .route("/auth/local", post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .route("/me", get(auth::me))
        .route("/me/password", post(accounts::change_password))
        .route("/me/usage", get(usage::mine))
        .route("/accounts", get(accounts::list).post(accounts::create))
        .route("/accounts/usage", get(usage::all))
        .route("/accounts/{id}/disable", post(accounts::disable))
        .route("/accounts/{id}/enable", post(accounts::enable))
        .route("/accounts/{id}/role", post(accounts::role))
        .route("/accounts/{id}/reset", post(accounts::reset))
        .route("/livez", get(status::live))
        .route("/readyz", get(status::ready))
        .route("/operator/status", get(status::operator_status))
        .route("/mutation-tickets", post(uploads::mutation_ticket))
        .route("/uploads", post(uploads::admit))
        .route(
            "/uploads/{id}",
            get(uploads::status).delete(uploads::cancel),
        )
        .route(
            "/uploads/{id}/content",
            put(uploads::content).patch(uploads::append),
        )
        .route("/uploads/{id}/commit", post(uploads::commit))
        .route("/share-links", get(shares::list).post(shares::create))
        .route("/share-links/{id}", axum::routing::delete(shares::revoke))
        .route("/share/{*token}", axum::routing::any(shares::download))
        .route("/share/", axum::routing::any(shares::missing))
        .route("/share", axum::routing::any(shares::missing))
        .route("/archives", post(archives::issue))
        .route("/archives/{token}", get(archives::download))
        .route(
            "/entries/{id}/content",
            get(content::get).head(content::head),
        )
        .layer(DefaultBodyLimit::max(64 * 1024))
        .with_state(state)
}
