use std::sync::Arc;

use colabrodo_server::{server::tokio, server_http::AssetServerLink, server_messages::*};

pub struct ObjectRoot {
    pub published: Vec<uuid::Uuid>,
    pub root: Object,
}

pub struct Object {
    pub parts: Vec<EntityReference>,
    pub children: Vec<Object>,
}

impl ObjectRoot {
    pub async fn prepare_remove(&self, link: Arc<tokio::sync::Mutex<AssetServerLink>>) {
        let mut lock = link.lock().await;
        for id in &self.published {
            lock.remove_asset(*id).await;
        }
    }
}
