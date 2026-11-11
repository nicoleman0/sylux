pub mod accounts;
pub mod assets;
pub mod auth;
pub mod content;
pub mod entries;
pub mod middleware;
pub mod mutations;
pub mod setup;
pub mod status;
pub mod uploads;

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
    pub storage: Option<Arc<UploadStorage>>,
    #[cfg(feature = "test-harness")]
    pub upload_cancel_test_barrier: Option<Arc<uploads::UploadCancelTestBarrier>>,
    #[cfg(feature = "test-harness")]
    pub upload_commit_test_barrier: Option<Arc<uploads::UploadCommitTestBarrier>>,
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
            storage: None,
            #[cfg(feature = "test-harness")]
            upload_cancel_test_barrier: None,
            #[cfg(feature = "test-harness")]
            upload_commit_test_barrier: None,
        }
    }

    pub fn with_storage_root(
        mut self,
        root: &std::path::Path,
    ) -> Result<Self, crate::domain::upload::StorageOpenError> {
        self.storage = Some(Arc::new(UploadStorage::open(root)?));
        Ok(self)
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
        .route("/entries/{id}/move", post(entries::move_entry))
        .route("/trash/{id}/restore", axum::routing::post(entries::restore))
        .route("/setup", get(setup::status))
        .route("/setup/admin", post(setup::create))
        .route("/auth/csrf", get(auth::csrf))
        .route("/auth/local", post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .route("/me", get(auth::me))
        .route("/me/password", post(accounts::change_password))
        .route("/accounts", get(accounts::list).post(accounts::create))
        .route("/accounts/{id}/disable", post(accounts::disable))
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
        .route("/uploads/{id}/content", put(uploads::content))
        .route("/uploads/{id}/commit", post(uploads::commit))
        .route(
            "/entries/{id}/content",
            get(content::get).head(content::head),
        )
        .layer(DefaultBodyLimit::max(64 * 1024))
        .with_state(state)
}
