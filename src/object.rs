use colabrodo_server::{server_http::*, server_messages::*};

pub struct ObjectRoot {
    pub published: Vec<uuid::Uuid>,
    pub root: Object,
}

pub struct Object {
    pub parts: Vec<EntityReference>,
    pub children: Vec<Object>,
}

impl ObjectRoot {
    pub fn prepare_remove(&self, link: AssetStorePtr) {
        for id in &self.published {
            remove_asset(link.clone(), *id);
        }
    }
}
