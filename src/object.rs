use colabrodo_server::{
    server_http::AssetServerLink,
    server_messages::{ComponentReference, ServerEntityState},
};

pub struct ObjectRoot {
    pub published: Vec<uuid::Uuid>,
    pub root: Object,
}

pub struct Object {
    pub parts: Vec<ComponentReference<ServerEntityState>>,
    pub children: Vec<Object>,
}

impl ObjectRoot {
    pub fn prepare_remove(&self, link: &mut AssetServerLink) {
        for id in &self.published {
            link.remove_asset(*id);
        }
    }
}
