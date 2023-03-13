use colabrodo_common::{common::strings::TAG_USER_HIDDEN, components::ImageSource};
use colabrodo_server::{
    server_bufferbuilder::*,
    server_http::*,
    server_messages::*,
    server_state::{ServerState, ServerStatePtr},
};

use crate::{object::*, scene_import::*};

struct IntermediateConverter<'a> {
    assets: Vec<uuid::Uuid>,
    asset_store: AssetStorePtr,

    scene_images: Vec<ImageReference>,
    scene_sampler: Vec<SamplerReference>,
    scene_textures: Vec<TextureReference>,
    scene_materials: Vec<MaterialReference>,
    scene_meshes: Vec<GeometryReference>,

    scene: IntermediateScene,
    state: &'a mut ServerState,
}

impl<'a> IntermediateConverter<'a> {
    fn recurse_intermediate(
        &mut self,
        n: &IntermediateNode,
        parent: Option<&EntityReference>,
    ) -> Object {
        let mut ent = ServerEntityState {
            name: Some(n.name.clone()),
            ..Default::default()
        };

        if let Some(x) = parent {
            ent.mutable.parent = Some(x.clone());
        }

        let root = self.state.entities.new_component(ent);

        let mut ret = Object {
            parts: vec![root.clone()],
            children: Vec::new(),
        };

        for mid in &n.meshes {
            let mut sub_ent = ServerEntityState::default();

            sub_ent.mutable.parent = Some(root.clone());
            sub_ent.mutable.tags = Some(vec![TAG_USER_HIDDEN.to_string()]);

            sub_ent.mutable.representation = Some(ServerEntityRepresentation::new_render(
                ServerRenderRepresentation {
                    mesh: self.scene_meshes[*mid as usize].clone(),
                    instances: None,
                },
            ));

            ret.parts.push(self.state.entities.new_component(sub_ent));
        }

        for child in &n.children {
            let child_obj = self.recurse_intermediate(child, Some(&root));
            ret.children.push(child_obj);
        }

        ret
    }

    fn start(&mut self) -> ObjectRoot {
        for img in &self.scene.images {
            let id = create_asset_id();
            self.assets.push(id);

            let res = add_asset(
                self.asset_store.clone(),
                id,
                Asset::new_from_slice(img.bytes.as_slice()),
            );

            self.scene_images
                .push(self.state.images.new_component(ServerImageState {
                    name: None,
                    source: ImageSource::new_uri(res.parse().unwrap()),
                }));
        }

        for sampler in &self.scene.samplers {
            self.scene_sampler
                .push(self.state.samplers.new_component(SamplerState {
                    name: None,
                    mag_filter: sampler.mag_filter,
                    min_filter: sampler.min_filter,
                    wrap_s: sampler.wrap_s,
                    wrap_t: sampler.wrap_t,
                }));
        }

        for tex in &self.scene.textures {
            log::debug!("Convert: {tex:?}");
            self.scene_textures.push(
                self.state.textures.new_component(ServerTextureState {
                    name: None,
                    image: self.scene_images[tex.image as usize].clone(),
                    sampler: tex
                        .sampler
                        .map(|id| self.scene_sampler[id as usize].clone()),
                }),
            );
        }

        for mat in &self.scene.mats {
            let tex = mat.base_color_texture.map(|id| ServerTextureRef {
                texture: self.scene_textures[id as usize].clone(),
                transform: None,
                texture_coord_slot: None,
            });

            log::debug!("Convert: {mat:?} {tex:?}");

            self.scene_materials
                .push(self.state.materials.new_component(ServerMaterialState {
                    name: mat.name.clone(),
                    mutable: ServerMaterialStateUpdatable {
                        pbr_info: Some(ServerPBRInfo {
                            base_color: mat.base_color,
                            base_color_texture: tex,
                            metallic: Some(mat.metallic),
                            roughness: Some(mat.roughness),
                            metal_rough_texture: None,
                        }),
                        double_sided: if mat.doublesided { Some(true) } else { None },
                        ..Default::default()
                    },
                }))
        }

        for mesh in &self.scene.meshes {
            let source = VertexSource {
                name: None,
                vertex: &mesh.verts,
                index: IndexType::Triangles(&mesh.indices),
            };

            let pack = source.pack_bytes().unwrap();

            let id = create_asset_id();
            self.assets.push(id);

            let res = add_asset(
                self.asset_store.clone(),
                id,
                Asset::new_from_slice(pack.bytes.as_slice()),
            );

            let partial = source
                .build_states(self.state, BufferRepresentation::Url(res))
                .unwrap();

            self.scene_meshes
                .push(self.state.geometries.new_component(ServerGeometryState {
                    name: None,
                    patches: vec![ServerGeometryPatch {
                        attributes: partial.attributes,
                        vertex_count: partial.vertex_count,
                        indices: partial.indices,
                        patch_type: partial.patch_type,
                        material: self.scene_materials[mesh.material as usize].clone(),
                    }],
                }));
        }

        let node = self.scene.nodes.take().unwrap();

        ObjectRoot {
            published: Default::default(),
            root: self.recurse_intermediate(&node, None),
        }
    }
}

pub fn convert_intermediate(
    scene: IntermediateScene,
    state: ServerStatePtr,
    asset_store: AssetStorePtr,
) -> ObjectRoot {
    let mut lock = state.lock().unwrap();

    let mut c = IntermediateConverter {
        assets: Vec::new(),
        asset_store,
        scene_images: Vec::new(),
        scene_sampler: Vec::new(),
        scene_textures: Vec::new(),
        scene_materials: Vec::new(),
        scene_meshes: Vec::new(),
        scene,
        state: &mut lock,
    };

    let mut root = c.start();

    root.published = c.assets;

    root
}
