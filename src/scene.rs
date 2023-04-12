use colabrodo_server::{server_http::*, server_messages::*};

use nalgebra_glm::*;

/// A scene; a collection of renderable objects
pub struct Scene {
    position: Vec3,
    rotation: Quat,
    scale: Vec3,

    /// A list of related binary assets published on the http server
    pub published: Vec<uuid::Uuid>,

    /// The root scene object
    pub root: SceneObject,

    /// A reference to the http server. Needed when we drop to unpublish assets.
    asset_store: AssetStorePtr,
}

/// Some file formats have a heirarchy. Some don't. This tries to cater to both.
pub struct SceneObject {
    /// A list of entities at this level.
    ///
    /// For some files, everything is a sibling. For others, we have to split one object into multiple entities.
    pub parts: Vec<EntityReference>,

    /// Some files have a heirarchy. Children of this node.
    pub children: Vec<SceneObject>,
}

impl Drop for Scene {
    fn drop(&mut self) {
        for id in &self.published {
            remove_asset(self.asset_store.clone(), *id);
        }
    }
}

impl Scene {
    /// Create a new scene from a root object, assets used, and a link to the http server.
    pub fn new(root: SceneObject, assets: Vec<uuid::Uuid>, asset_store: AssetStorePtr) -> Self {
        Self {
            position: Vec3::zeros(),
            rotation: Quat::default(),
            scale: Vec3::repeat(1.0),
            published: assets,
            root,
            asset_store,
        }
    }

    /// Update the position of this scene
    pub fn set_position(&mut self, p: Vec3) {
        self.position = p;
        self.update_transform();
    }

    /// Update the rotation of the scene
    pub fn set_rotation(&mut self, q: Quat) {
        self.rotation = q;
        self.update_transform();
    }

    /// Update the scale of the scene
    pub fn set_scale(&mut self, s: Vec3) {
        self.scale = s;
        self.update_transform();
    }

    /// Refresh the transformation matrix of this scene
    pub fn update_transform(&mut self) {
        let mut tf = Mat4::new_translation(&self.position);
        tf *= quat_to_mat4(&self.rotation);
        tf *= Mat4::new_nonuniform_scaling(&self.scale);

        if log::log_enabled!(log::Level::Debug) {
            log::debug!("Update object transform: {tf:?}");
        }

        if let Some(first) = self.root.parts.first() {
            let update = ServerEntityStateUpdatable {
                transform: Some(tf.as_slice().try_into().unwrap()),
                ..Default::default()
            };

            update.patch(first);
        }
    }
}
