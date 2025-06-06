use std::borrow::BorrowMut;
use std::fmt::{Debug, Display};
use std::sync::{Arc, Weak};

use crate::CollabKVDB;
use anyhow::{anyhow, Error};
use arc_swap::{ArcSwap, ArcSwapOption};
use collab::core::collab::DataSource;
use collab::core::collab_plugin::CollabPersistence;
use collab::entity::EncodedCollab;
use collab::error::CollabError;
use collab::preclude::{Collab, CollabBuilder};
use collab_database::workspace_database::{DatabaseCollabService, WorkspaceDatabaseManager};
use collab_document::blocks::DocumentData;
use collab_document::document::Document;
use collab_entity::{CollabObject, CollabType};
use collab_folder::{Folder, FolderData, FolderNotify};
use collab_plugins::connect_state::{CollabConnectReachability, CollabConnectState};
use collab_plugins::local_storage::kv::snapshot::SnapshotPersistence;

if_native! {
use collab_plugins::local_storage::rocksdb::rocksdb_plugin::{RocksdbBackup, RocksdbDiskPlugin};
}

if_wasm! {
use collab_plugins::local_storage::indexeddb::IndexeddbDiskPlugin;
}

pub use crate::plugin_provider::CollabCloudPluginProvider;
use collab::lock::RwLock;
use collab_plugins::local_storage::kv::doc::CollabKVAction;
use collab_plugins::local_storage::kv::KVTransactionDB;
use collab_plugins::local_storage::CollabPersistenceConfig;
use collab_user::core::{UserAwareness, UserAwarenessNotifier};

use flowy_error::FlowyError;
use lib_infra::{if_native, if_wasm};
use tracing::{error, instrument, trace, warn};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub enum CollabPluginProviderType {
  Local,
  AppFlowyCloud,
}

pub enum CollabPluginProviderContext {
  Local,
  AppFlowyCloud {
    uid: i64,
    collab_object: CollabObject,
    local_collab: Weak<RwLock<dyn BorrowMut<Collab> + Send + Sync + 'static>>,
  },
}

impl Display for CollabPluginProviderContext {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    let str = match self {
      CollabPluginProviderContext::Local => "Local".to_string(),
      CollabPluginProviderContext::AppFlowyCloud {
        uid: _,
        collab_object,
        ..
      } => collab_object.to_string(),
    };
    write!(f, "{}", str)
  }
}

pub trait WorkspaceCollabIntegrate: Send + Sync {
  fn workspace_id(&self) -> Result<Uuid, FlowyError>;
  fn device_id(&self) -> Result<String, FlowyError>;
}

pub struct AppFlowyCollabBuilder {
  network_reachability: CollabConnectReachability,
  plugin_provider: ArcSwap<Arc<dyn CollabCloudPluginProvider>>,
  snapshot_persistence: ArcSwapOption<Arc<dyn SnapshotPersistence + 'static>>,
  #[cfg(not(target_arch = "wasm32"))]
  rocksdb_backup: ArcSwapOption<Arc<dyn RocksdbBackup>>,
  workspace_integrate: Arc<dyn WorkspaceCollabIntegrate>,
}

impl AppFlowyCollabBuilder {
  pub fn new(
    storage_provider: impl CollabCloudPluginProvider + 'static,
    workspace_integrate: impl WorkspaceCollabIntegrate + 'static,
  ) -> Self {
    Self {
      network_reachability: CollabConnectReachability::new(),
      plugin_provider: ArcSwap::new(Arc::new(Arc::new(storage_provider))),
      snapshot_persistence: Default::default(),
      #[cfg(not(target_arch = "wasm32"))]
      rocksdb_backup: Default::default(),
      workspace_integrate: Arc::new(workspace_integrate),
    }
  }

  pub fn set_snapshot_persistence(&self, snapshot_persistence: Arc<dyn SnapshotPersistence>) {
    self
      .snapshot_persistence
      .store(Some(snapshot_persistence.into()));
  }

  #[cfg(not(target_arch = "wasm32"))]
  pub fn set_rocksdb_backup(&self, rocksdb_backup: Arc<dyn RocksdbBackup>) {
    self.rocksdb_backup.store(Some(rocksdb_backup.into()));
  }

  pub fn update_network(&self, reachable: bool) {
    if reachable {
      self
        .network_reachability
        .set_state(CollabConnectState::Connected)
    } else {
      self
        .network_reachability
        .set_state(CollabConnectState::Disconnected)
    }
  }

  pub fn collab_object(
    &self,
    workspace_id: &Uuid,
    uid: i64,
    object_id: &Uuid,
    collab_type: CollabType,
  ) -> Result<CollabObject, Error> {
    // Compare the workspace_id with the currently opened workspace_id. Return an error if they do not match.
    // This check is crucial in asynchronous code contexts where the workspace_id might change during operation.
    let actual_workspace_id = self.workspace_integrate.workspace_id()?;
    if workspace_id != &actual_workspace_id {
      return Err(anyhow::anyhow!(
        "workspace_id not match when build collab. expect workspace_id: {}, actual workspace_id: {}",
        workspace_id,
        actual_workspace_id
      ));
    }
    let device_id = self.workspace_integrate.device_id()?;
    Ok(CollabObject::new(
      uid,
      object_id.to_string(),
      collab_type,
      workspace_id.to_string(),
      device_id,
    ))
  }

  #[allow(clippy::too_many_arguments)]
  #[instrument(
    level = "trace",
    skip(self, data_source, collab_db, builder_config, data)
  )]
  pub async fn create_document(
    &self,
    object: CollabObject,
    data_source: DataSource,
    collab_db: Weak<CollabKVDB>,
    builder_config: CollabBuilderConfig,
    data: Option<DocumentData>,
  ) -> Result<Arc<RwLock<Document>>, Error> {
    let expected_collab_type = CollabType::Document;
    assert_eq!(object.collab_type, expected_collab_type);
    let mut collab = self.build_collab(&object, &collab_db, data_source).await?;
    collab.enable_undo_redo();

    let document = match data {
      None => Document::open(collab)?,
      Some(data) => {
        let document = Document::create_with_data(collab, data)?;
        if let Err(err) = self.write_collab_to_disk(
          object.uid,
          &object.workspace_id,
          &object.object_id,
          collab_db.clone(),
          &object.collab_type,
          &document,
        ) {
          error!(
            "build_collab: flush document collab to disk failed: {}",
            err
          );
        }
        document
      },
    };
    let document = Arc::new(RwLock::new(document));
    self.finalize(object, builder_config, document)
  }

  #[allow(clippy::too_many_arguments)]
  #[instrument(
    level = "trace",
    skip(self, object, doc_state, collab_db, builder_config, folder_notifier)
  )]
  pub async fn create_folder(
    &self,
    object: CollabObject,
    doc_state: DataSource,
    collab_db: Weak<CollabKVDB>,
    builder_config: CollabBuilderConfig,
    folder_notifier: Option<FolderNotify>,
    folder_data: Option<FolderData>,
  ) -> Result<Arc<RwLock<Folder>>, Error> {
    let expected_collab_type = CollabType::Folder;
    assert_eq!(object.collab_type, expected_collab_type);
    let folder = match folder_data {
      None => {
        let collab = self.build_collab(&object, &collab_db, doc_state).await?;
        Folder::open(object.uid, collab, folder_notifier)?
      },
      Some(data) => {
        let collab = self.build_collab(&object, &collab_db, doc_state).await?;
        let folder = Folder::create(object.uid, collab, folder_notifier, data);
        if let Err(err) = self.write_collab_to_disk(
          object.uid,
          &object.workspace_id,
          &object.object_id,
          collab_db.clone(),
          &object.collab_type,
          &folder,
        ) {
          error!("build_collab: flush folder collab to disk failed: {}", err);
        }
        folder
      },
    };
    let folder = Arc::new(RwLock::new(folder));
    self.finalize(object, builder_config, folder)
  }

  #[allow(clippy::too_many_arguments)]
  #[instrument(
    level = "trace",
    skip(self, object, doc_state, collab_db, builder_config, notifier)
  )]
  pub async fn create_user_awareness(
    &self,
    object: CollabObject,
    doc_state: DataSource,
    collab_db: Weak<CollabKVDB>,
    builder_config: CollabBuilderConfig,
    notifier: Option<UserAwarenessNotifier>,
  ) -> Result<Arc<RwLock<UserAwareness>>, Error> {
    let expected_collab_type = CollabType::UserAwareness;
    assert_eq!(object.collab_type, expected_collab_type);
    let collab = self.build_collab(&object, &collab_db, doc_state).await?;
    let user_awareness = UserAwareness::create(collab, notifier)?;
    let user_awareness = Arc::new(RwLock::new(user_awareness));
    self.finalize(object, builder_config, user_awareness)
  }

  #[allow(clippy::too_many_arguments)]
  #[instrument(level = "trace", skip_all)]
  pub fn create_workspace_database_manager(
    &self,
    object: CollabObject,
    collab: Collab,
    _collab_db: Weak<CollabKVDB>,
    builder_config: CollabBuilderConfig,
    collab_service: impl DatabaseCollabService,
  ) -> Result<Arc<RwLock<WorkspaceDatabaseManager>>, Error> {
    let expected_collab_type = CollabType::WorkspaceDatabase;
    assert_eq!(object.collab_type, expected_collab_type);
    let workspace = WorkspaceDatabaseManager::open(&object.object_id, collab, collab_service)?;
    let workspace = Arc::new(RwLock::new(workspace));
    self.finalize(object, builder_config, workspace)
  }

  pub async fn build_collab(
    &self,
    object: &CollabObject,
    collab_db: &Weak<CollabKVDB>,
    data_source: DataSource,
  ) -> Result<Collab, Error> {
    let object = object.clone();
    let collab_db = collab_db.clone();
    let device_id = self.workspace_integrate.device_id()?;
    let collab = tokio::task::spawn_blocking(move || {
      let mut collab = CollabBuilder::new(object.uid, &object.object_id, data_source)
        .with_device_id(device_id)
        .build()?;
      let persistence_config = CollabPersistenceConfig::default();
      let db_plugin = RocksdbDiskPlugin::new_with_config(
        object.uid,
        object.workspace_id.clone(),
        object.object_id.to_string(),
        object.collab_type,
        collab_db,
        persistence_config,
      );
      collab.add_plugin(Box::new(db_plugin));
      collab.initialize();
      Ok::<_, Error>(collab)
    })
    .await??;

    Ok(collab)
  }

  pub fn finalize<T>(
    &self,
    object: CollabObject,
    build_config: CollabBuilderConfig,
    collab: Arc<RwLock<T>>,
  ) -> Result<Arc<RwLock<T>>, Error>
  where
    T: BorrowMut<Collab> + Send + Sync + 'static,
  {
    let mut write_collab = collab.try_write()?;
    let has_cloud_plugin = write_collab.borrow().has_cloud_plugin();
    if has_cloud_plugin {
      drop(write_collab);
      return Ok(collab);
    }

    if build_config.sync_enable {
      trace!("🚀finalize collab:{}", object);
      let plugin_provider = self.plugin_provider.load_full();
      let provider_type = plugin_provider.provider_type();
      let span =
        tracing::span!(tracing::Level::TRACE, "collab_builder", object_id = %object.object_id);
      let _enter = span.enter();
      match provider_type {
        CollabPluginProviderType::AppFlowyCloud => {
          let local_collab = Arc::downgrade(&collab);
          let plugins = plugin_provider.get_plugins(CollabPluginProviderContext::AppFlowyCloud {
            uid: object.uid,
            collab_object: object,
            local_collab,
          });

          // at the moment when we get the lock, the collab object is not yet exposed outside
          for plugin in plugins {
            write_collab.borrow().add_plugin(plugin);
          }
        },
        CollabPluginProviderType::Local => {},
      }
    }

    (*write_collab).borrow_mut().initialize();
    drop(write_collab);
    Ok(collab)
  }

  /// Remove all updates in disk and write the final state vector to disk.
  #[instrument(level = "trace", skip_all, err)]
  pub fn write_collab_to_disk<T>(
    &self,
    uid: i64,
    workspace_id: &str,
    object_id: &str,
    collab_db: Weak<CollabKVDB>,
    collab_type: &CollabType,
    collab: &T,
  ) -> Result<(), Error>
  where
    T: BorrowMut<Collab> + Send + Sync + 'static,
  {
    if let Some(collab_db) = collab_db.upgrade() {
      let write_txn = collab_db.write_txn();
      trace!("flush collab:{}-{}-{} to disk", uid, collab_type, object_id);
      let collab: &Collab = collab.borrow();
      let encode_collab =
        collab.encode_collab_v1(|collab| collab_type.validate_require_data(collab))?;
      write_txn.flush_doc(
        uid,
        workspace_id,
        object_id,
        encode_collab.state_vector.to_vec(),
        encode_collab.doc_state.to_vec(),
      )?;
      write_txn.commit_transaction()?;
    } else {
      error!("collab_db is dropped");
    }

    Ok(())
  }
}

pub struct CollabBuilderConfig {
  pub sync_enable: bool,
}

impl Default for CollabBuilderConfig {
  fn default() -> Self {
    Self { sync_enable: true }
  }
}

impl CollabBuilderConfig {
  pub fn sync_enable(mut self, sync_enable: bool) -> Self {
    self.sync_enable = sync_enable;
    self
  }
}

pub struct CollabPersistenceImpl {
  pub db: Weak<CollabKVDB>,
  pub uid: i64,
  pub workspace_id: Uuid,
}

impl CollabPersistenceImpl {
  pub fn new(db: Weak<CollabKVDB>, uid: i64, workspace_id: Uuid) -> Self {
    Self {
      db,
      uid,
      workspace_id,
    }
  }

  pub fn into_data_source(self) -> DataSource {
    DataSource::Disk(Some(Box::new(self)))
  }
}

impl CollabPersistence for CollabPersistenceImpl {
  fn load_collab_from_disk(&self, collab: &mut Collab) -> Result<(), CollabError> {
    let collab_db = self
      .db
      .upgrade()
      .ok_or_else(|| CollabError::Internal(anyhow!("collab_db is dropped")))?;

    let object_id = collab.object_id().to_string();
    let rocksdb_read = collab_db.read_txn();
    let workspace_id = self.workspace_id.to_string();

    if rocksdb_read.is_exist(self.uid, &workspace_id, &object_id) {
      let mut txn = collab.transact_mut();
      match rocksdb_read.load_doc_with_txn(self.uid, &workspace_id, &object_id, &mut txn) {
        Ok(update_count) => {
          trace!(
            "did load collab:{}-{} from disk, update_count:{}",
            self.uid,
            object_id,
            update_count
          );
        },
        Err(err) => {
          error!("🔴 load doc:{} failed: {}", object_id, err);
        },
      }
      drop(rocksdb_read);
      txn.commit();
      drop(txn);
    }
    Ok(())
  }

  fn save_collab_to_disk(
    &self,
    object_id: &str,
    encoded_collab: EncodedCollab,
  ) -> Result<(), CollabError> {
    let workspace_id = self.workspace_id.to_string();
    let collab_db = self
      .db
      .upgrade()
      .ok_or_else(|| CollabError::Internal(anyhow!("collab_db is dropped")))?;
    let write_txn = collab_db.write_txn();
    write_txn
      .flush_doc(
        self.uid,
        workspace_id.as_str(),
        object_id,
        encoded_collab.state_vector.to_vec(),
        encoded_collab.doc_state.to_vec(),
      )
      .map_err(|err| CollabError::Internal(err.into()))?;

    write_txn
      .commit_transaction()
      .map_err(|err| CollabError::Internal(err.into()))?;
    Ok(())
  }
}
