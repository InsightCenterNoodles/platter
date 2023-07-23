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
    asset_store: Option<AssetStorePtr>,
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
        if let Some(ptr) = &self.asset_store {
            for id in &self.published {
                remove_asset(ptr.clone(), *id);
            }
        }
    }
}

impl Scene {
    /// Create a new scene from a root object, assets used, and a link to the http server.
    pub fn new(
        root: SceneObject,
        assets: Vec<uuid::Uuid>,
        asset_store: Option<AssetStorePtr>,
    ) -> Self {
        Self {
            position: Vec3::zeros(),
            rotation: Quat::identity(),
            scale: Vec3::repeat(1.0),
            published: assets,
            root,
            asset_store,
        }
    }

    /// Update the position of this scene
    pub fn set_position(&mut self, p: Vec3) {
        log::debug!("Setting position: {p:?}");
        self.position = p;
        self.update_transform();
    }

    /// Update the rotation of the scene
    pub fn set_rotation(&mut self, q: Quat) {
        log::debug!("Setting rotation: {q:?}");
        self.rotation = q;
        self.update_transform();
    }

    /// Update the scale of the scene
    pub fn set_scale(&mut self, s: Vec3) {
        log::debug!("Setting scales: {s:?}");
        self.scale = s;
        self.update_transform();
    }

    /// Refresh the transformation matrix of this scene
    pub fn update_transform(&mut self) -> Mat4 {
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

        tf
    }
}

#[cfg(test)]
mod test {
    use super::Scene;
    use approx::assert_relative_eq;
    use nalgebra_glm::*;

    #[test]
    fn test_scene_transforms() {
        let mut s = Scene::new(
            super::SceneObject {
                parts: Vec::new(),
                children: Vec::new(),
            },
            Vec::new(),
            None,
        );

        s.set_position(Vec3::new(1.0, 2.0, 3.0));

        let tf = s.update_transform();

        assert_relative_eq!(
            tf,
            &Mat4::from_columns(&[
                [1.0, 0.0, 0.0, 0.0].into(),
                [0.0, 1.0, 0.0, 0.0].into(),
                [0.0, 0.0, 1.0, 0.0].into(),
                [1.0, 2.0, 3.0, 1.0].into(),
            ]),
            max_relative = 0.001,
        );
    }
}
