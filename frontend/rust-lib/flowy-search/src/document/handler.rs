use flowy_error::FlowyResult;
use flowy_folder::{manager::FolderManager, ViewLayout};
use flowy_search_pub::cloud::SearchCloudService;
use lib_infra::async_trait::async_trait;
use std::str::FromStr;
use std::sync::Arc;
use tracing::{trace, warn};
use uuid::Uuid;

use crate::{
  entities::{IndexTypePB, ResultIconPB, ResultIconTypePB, SearchFilterPB, SearchResultPB},
  services::manager::{SearchHandler, SearchType},
};

pub struct DocumentSearchHandler {
  pub cloud_service: Arc<dyn SearchCloudService>,
  pub folder_manager: Arc<FolderManager>,
}

impl DocumentSearchHandler {
  pub fn new(
    cloud_service: Arc<dyn SearchCloudService>,
    folder_manager: Arc<FolderManager>,
  ) -> Self {
    Self {
      cloud_service,
      folder_manager,
    }
  }
}

#[async_trait]
impl SearchHandler for DocumentSearchHandler {
  fn search_type(&self) -> SearchType {
    SearchType::Document
  }

  async fn perform_search(
    &self,
    query: String,
    filter: Option<SearchFilterPB>,
  ) -> FlowyResult<Vec<SearchResultPB>> {
    let filter = match filter {
      Some(filter) => filter,
      None => return Ok(vec![]),
    };

    let workspace_id = match filter.workspace_id {
      Some(workspace_id) => workspace_id,
      None => return Ok(vec![]),
    };

    let workspace_id = Uuid::from_str(&workspace_id)?;
    let results = self
      .cloud_service
      .document_search(&workspace_id, query)
      .await?;
    trace!("[Search] remote search results: {:?}", results);

    // Grab all views from folder cache
    // Notice that `get_all_view_pb` returns Views that don't include trashed and private views
    let views = self.folder_manager.get_all_views_pb().await?;
    let mut search_results: Vec<SearchResultPB> = vec![];

    for result in results {
      if let Some(view) = views.iter().find(|v| v.id == result.object_id.to_string()) {
        // If there is no View for the result, we don't add it to the results
        // If possible we will extract the icon to display for the result
        let icon: Option<ResultIconPB> = match view.icon.clone() {
          Some(view_icon) => Some(ResultIconPB::from(view_icon)),
          None => {
            let view_layout_ty: i64 = ViewLayout::from(view.layout.clone()).into();
            Some(ResultIconPB {
              ty: ResultIconTypePB::Icon,
              value: view_layout_ty.to_string(),
            })
          },
        };

        search_results.push(SearchResultPB {
          index_type: IndexTypePB::Document,
          view_id: result.object_id.to_string(),
          id: result.object_id.to_string(),
          data: view.name.clone(),
          icon,
          score: result.score,
          workspace_id: result.workspace_id.to_string(),
          preview: result.preview,
        });
      } else {
        warn!("No view found for search result: {:?}", result);
      }
    }

    trace!("[Search] showing results: {:?}", search_results);
    Ok(search_results)
  }

  /// Ignore for [DocumentSearchHandler]
  fn index_count(&self) -> u64 {
    0
  }
}
