use colabrodo_server::{server_http::*, server_messages::*};

use nalgebra_glm::*;

pub struct ObjectRoot {
    pos: Vec3,
    rot: Quat,
    scale: Vec3,

    pub published: Vec<uuid::Uuid>,
    pub root: Object,
    link: AssetStorePtr,
}

pub struct Object {
    // first entity is the root
    pub parts: Vec<EntityReference>,
    pub children: Vec<Object>,
}

impl Drop for ObjectRoot {
    fn drop(&mut self) {
        for id in &self.published {
            remove_asset(self.link.clone(), *id);
        }
    }
}

impl ObjectRoot {
    pub fn new(root: Object, assets: Vec<uuid::Uuid>, link: AssetStorePtr) -> Self {
        Self {
            pos: Vec3::zeros(),
            rot: Quat::default(),
            scale: Vec3::repeat(1.0),
            published: assets,
            root,
            link,
        }
    }

    pub fn set_position(&mut self, p: Vec3) {
        self.pos = p;
        self.update_transform();
    }

    pub fn set_rotation(&mut self, q: Quat) {
        self.rot = q;
        self.update_transform();
    }

    pub fn set_scale(&mut self, s: Vec3) {
        self.scale = s;
        self.update_transform();
    }

    pub fn update_transform(&mut self) {
        let mut tf = Mat4::new_translation(&self.pos);
        tf *= quat_to_mat4(&self.rot);
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
