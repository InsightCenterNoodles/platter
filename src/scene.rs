use colabrodo_server::{server_http::*, server_messages::*};
use nalgebra::{Isometry3, Matrix4, Quaternion, Scale3, Translation3, UnitQuaternion, Vector3};

/// A scene; a collection of renderable objects
pub struct Scene {
    position: Translation3<f32>,
    rotation: UnitQuaternion<f32>,
    scale: Scale3<f32>,

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
            position: Translation3::identity(),
            rotation: UnitQuaternion::identity(),
            scale: Scale3::identity(),
            published: assets,
            root,
            asset_store,
        }
    }

    /// Update the position of this scene
    pub fn set_position(&mut self, p: Vector3<f32>) {
        log::debug!("Setting position: {p:?}");
        self.position = Translation3::new(p.x, p.y, p.z);
        self.update_transform();
    }

    /// Update the rotation of the scene
    pub fn set_rotation(&mut self, q: Quaternion<f32>) {
        log::debug!("Setting rotation: {q:?}");
        self.rotation = UnitQuaternion::from_quaternion(q);
        self.update_transform();
    }

    /// Update the scale of the scene
    pub fn set_scale(&mut self, s: Vector3<f32>) {
        log::debug!("Setting scales: {s:?}");
        self.scale = Scale3::new(s.x, s.y, s.z);
        self.update_transform();
    }

    /// Refresh the transformation matrix of this scene
    pub fn update_transform(&mut self) -> Matrix4<f32> {
        let iso = Isometry3::from_parts(self.position, self.rotation);
        let tf = iso.to_homogeneous() * self.scale.to_homogeneous();

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
    use nalgebra::{point, vector, Matrix4, Quaternion};

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

        s.set_position(vector![1.0, 2.0, 3.0]);

        let tf = s.update_transform();

        assert_relative_eq!(
            tf,
            &Matrix4::from_columns(&[
                [1.0, 0.0, 0.0, 0.0].into(),
                [0.0, 1.0, 0.0, 0.0].into(),
                [0.0, 0.0, 1.0, 0.0].into(),
                [1.0, 2.0, 3.0, 1.0].into(),
            ])
        );

        s.set_position(vector![0.0, 0.0, 0.0]);

        let tf = s.update_transform();

        assert_relative_eq!(
            tf,
            &Matrix4::from_columns(&[
                [1.0, 0.0, 0.0, 0.0].into(),
                [0.0, 1.0, 0.0, 0.0].into(),
                [0.0, 0.0, 1.0, 0.0].into(),
                [0.0, 0.0, 0.0, 1.0].into(),
            ]),
            max_relative = 0.001,
        );

        s.set_rotation(Quaternion::new(0.7071, 0.7071, 0.0, 0.0));

        let tf = s.update_transform();

        assert_relative_eq!(
            tf,
            &Matrix4::from_columns(&[
                [1.0, 0.0, 0.0, 0.0].into(),
                [0.0, 0.0, 1.0, 0.0].into(),
                [0.0, -1.0, 0.0, 0.0].into(),
                [0.0, 0.0, 0.0, 1.0].into(),
            ]),
            max_relative = 0.001,
        );

        s.set_rotation(Quaternion::identity());

        let tf = s.update_transform();

        assert_relative_eq!(
            tf,
            &Matrix4::from_columns(&[
                [1.0, 0.0, 0.0, 0.0].into(),
                [0.0, 1.0, 0.0, 0.0].into(),
                [0.0, 0.0, 1.0, 0.0].into(),
                [0.0, 0.0, 0.0, 1.0].into(),
            ]),
            max_relative = 0.001,
        );

        s.set_scale(vector![2.0, 3.0, 4.0]);

        let tf = s.update_transform();

        assert_relative_eq!(
            tf,
            &Matrix4::from_columns(&[
                [2.0, 0.0, 0.0, 0.0].into(),
                [0.0, 3.0, 0.0, 0.0].into(),
                [0.0, 0.0, 4.0, 0.0].into(),
                [0.0, 0.0, 0.0, 1.0].into(),
            ]),
            max_relative = 0.001,
        );

        s.set_scale(vector![1.0, 1.0, 1.0]);

        let tf = s.update_transform();

        assert_relative_eq!(
            tf,
            &Matrix4::from_columns(&[
                [1.0, 0.0, 0.0, 0.0].into(),
                [0.0, 1.0, 0.0, 0.0].into(),
                [0.0, 0.0, 1.0, 0.0].into(),
                [0.0, 0.0, 0.0, 1.0].into(),
            ]),
            max_relative = 0.001,
        );

        s.set_scale(vector![2.0, 2.0, 2.0]);
        s.set_position(vector![3.0, 3.0, 3.0]);

        let tf = s.update_transform();

        assert_relative_eq!(
            tf,
            &Matrix4::from_columns(&[
                [2.0, 0.0, 0.0, 0.0].into(),
                [0.0, 2.0, 0.0, 0.0].into(),
                [0.0, 0.0, 2.0, 0.0].into(),
                [3.0, 3.0, 3.0, 1.0].into(),
            ]),
            max_relative = 0.001,
        );

        assert_relative_eq!(
            tf.transform_point(&point![4.0, 4.0, 4.0]),
            &point!(11.0, 11.0, 11.0)
        );

        s.set_scale(vector![2.0, 1.0, 0.5]);

        let (sin, cos) = (30.0_f32.to_radians() / 2.0_f32).sin_cos();

        s.set_rotation(Quaternion::new(cos, sin, 0.0, 0.0));
        s.set_position(vector![10.0, 20.0, 30.0]);

        let tf = s.update_transform();

        assert_relative_eq!(
            tf,
            &Matrix4::from_columns(&[
                [2.0, 0.0, 0.0, 0.0].into(),
                [0.0, 0.866, 0.5, 0.0].into(),
                [0.0, -0.25, 0.433, 0.0].into(),
                [10.0, 20.0, 30.0, 1.0].into(),
            ]),
            max_relative = 0.001
        );
    }
}
