use colabrodo_common::{common::strings::TAG_USER_HIDDEN, components::ImageSource};
use colabrodo_server::server::tokio;
use colabrodo_server::{
    server_bufferbuilder::*,
    server_http::{create_asset_id, Asset, AssetServerLink},
    server_messages::*,
    server_state::{ServerState, ServerStatePtr},
};
use std::sync::Arc;

use crate::{object::*, scene_import::*};

pub struct ReadyBuffer {
    url: String,
    id: uuid::Uuid,
}

pub async fn convert_images(
    images: &Vec<IntermediateImage>,
    link: Arc<tokio::sync::Mutex<AssetServerLink>>,
) -> Vec<ReadyBuffer> {
    let mut result = Vec::new();
    for img in images {
        let id = create_asset_id();
        let res = link
            .lock()
            .await
            .add_asset(id, Asset::new_from_slice(img.bytes.as_slice()))
            .await;
        result.push(ReadyBuffer { url: res, id });
    }
    result
}

pub async fn convert_meshes(
    meshes: &Vec<IntermediateMesh>,
    link: Arc<tokio::sync::Mutex<AssetServerLink>>,
) -> Vec<ReadyBuffer> {
    let mut result = Vec::new();
    for mesh in meshes {
        let pack = {
            let source = VertexSource {
                name: None,
                vertex: mesh.verts.as_slice(),
                index: colabrodo_server::server_bufferbuilder::IndexType::Triangles(
                    mesh.indices.as_slice(),
                ),
            };
            source.pack_bytes()
        }
        .unwrap();

        let id = create_asset_id();
        let res = {
            link.lock()
                .await
                .add_asset(id, Asset::new_from_slice(pack.bytes.as_slice()))
        }
        .await;
        result.push(ReadyBuffer { url: res, id });
    }
    result
}

struct IntermediateConverter<'a> {
    images: Vec<ReadyBuffer>,
    meshes: Vec<ReadyBuffer>,

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
        for img in &self.images {
            self.scene_images
                .push(self.state.images.new_component(ServerImageState {
                    name: None,
                    source: ImageSource::new_uri(img.url.parse().unwrap()),
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

        for (reg_mesh, mesh) in self.meshes.iter().zip(self.scene.meshes.iter()) {
            let source = VertexSource {
                name: None,
                vertex: &mesh.verts,
                index: IndexType::Triangles(&mesh.indices),
            };

            let partial = source
                .build_states(self.state, BufferRepresentation::Url(reg_mesh.url.clone()))
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
            published: self
                .images
                .iter()
                .map(|f| f.id)
                .chain(self.meshes.iter().map(|f| f.id))
                .collect(),
            root: self.recurse_intermediate(&node, None),
        }
    }
}

pub fn convert_intermediate(
    images: Vec<ReadyBuffer>,
    meshes: Vec<ReadyBuffer>,
    scene: IntermediateScene,
    state: ServerStatePtr,
) -> ObjectRoot {
    let mut lock = state.lock().unwrap();

    let mut c = IntermediateConverter {
        images,
        meshes,
        scene_images: Vec::new(),
        scene_sampler: Vec::new(),
        scene_textures: Vec::new(),
        scene_materials: Vec::new(),
        scene_meshes: Vec::new(),
        scene,
        state: &mut lock,
    };

    c.start()
}
