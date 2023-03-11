use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::Mutex;

use colabrodo_common::common::strings::TAG_USER_HIDDEN;
use colabrodo_common::components::BufferViewState;
use colabrodo_common::components::MagFilter;
use colabrodo_common::components::MinFilter;
use colabrodo_common::components::SamplerMode;
use colabrodo_server::server_bufferbuilder;
use colabrodo_server::server_bufferbuilder::BufferRepresentation;
use colabrodo_server::server_bufferbuilder::VertexFull;
use colabrodo_server::server_bufferbuilder::VertexSource;
use colabrodo_server::server_http::create_asset_id;
use colabrodo_server::server_http::Asset;
use colabrodo_server::server_http::AssetServerLink;
use colabrodo_server::server_messages::*;
use colabrodo_server::server_state::*;
use russimp::material::PropertyTypeInfo;
use russimp::material::Texture;
use russimp::material::TextureType;
use russimp::scene::PostProcess;
use russimp::scene::Scene;

use crate::object::Object;
use crate::object::ObjectRoot;

// Eventually we can do this
//const M_SAMPLER_FILTER_NEAREST: f32 = f32::from_le_bytes(9728_i32.to_le_bytes());

pub struct ImportedScene {
    data: Scene,
    link: Arc<Mutex<AssetServerLink>>,
}

impl ImportedScene {
    pub fn import_file(
        path: &Path,
        link: Arc<Mutex<AssetServerLink>>,
    ) -> Result<Self, ImportError> {
        if !path.try_exists().unwrap_or(false) {
            return Err(ImportError::UnableToOpenFile(
                "File does not exist.".to_string(),
            ));
        }

        let path_as_str = path.to_str().ok_or(ImportError::UnableToOpenFile(
            "Invalid filename".to_string(),
        ))?;

        let flags = vec![
            PostProcess::Triangulate,
            PostProcess::JoinIdenticalVertices,
            PostProcess::GenerateSmoothNormals,
            PostProcess::CalculateTangentSpace,
            PostProcess::SortByPrimitiveType,
            PostProcess::GenerateBoundingBoxes,
            PostProcess::SplitLargeMeshes,
        ];

        let scene = Scene::from_file(path_as_str, flags);

        match scene {
            Err(x) => Err(ImportError::UnableToImport(x.to_string())),
            Ok(x) => Ok(Self { data: x, link }),
        }
    }

    pub async fn build_objects(
        &self,
        max_buffer_size: u64,
        link: Arc<Mutex<AssetServerLink>>,
        state: ServerStatePtr,
    ) -> ObjectRoot {
        let scratch = ImportScratch::new(link, state, max_buffer_size);

        scratch.build(&self.data).await
    }
}

#[derive(Clone)]
struct AssimpTexture(Rc<RefCell<russimp::material::Texture>>);

impl core::hash::Hash for AssimpTexture {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::ptr::hash(&*self.0, state);
    }
}

impl PartialEq for AssimpTexture {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for AssimpTexture {}

struct ImportScratch {
    link: Arc<Mutex<AssetServerLink>>,
    state: ServerStatePtr,
    max_mesh_size: u64,
    published: Vec<uuid::Uuid>,
    images: HashMap<AssimpTexture, ImageReference>,
    materials: Vec<MaterialReference>,
    meshes: Vec<GeometryReference>,

    nodes: Option<Object>,
}

impl ImportScratch {
    fn new(link: Arc<Mutex<AssetServerLink>>, state: ServerStatePtr, max_mesh_size: u64) -> Self {
        Self {
            link,
            state,
            max_mesh_size,
            published: Default::default(),
            images: Default::default(),
            materials: Default::default(),
            meshes: Default::default(),
            nodes: Default::default(),
        }
    }
    fn recurse_node(
        &mut self,
        parent: Option<&EntityReference>,
        node: &Rc<RefCell<russimp::node::Node>>,
    ) -> Object {
        let n = node.borrow();

        log::debug!("Importing node: {}", n.name);

        let mut ent = ServerEntityState {
            name: Some(n.name.clone()),
            ..Default::default()
        };

        if let Some(x) = parent {
            ent.mutable.parent = Some(x.clone());
        }

        let root = self.state.lock().unwrap().entities.new_component(ent);

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
                    mesh: self.meshes[*mid as usize].clone(),
                    instances: None,
                },
            ));

            ret.parts
                .push(self.state.lock().unwrap().entities.new_component(sub_ent));
        }

        for child in &n.children {
            let child_obj = self.recurse_node(Some(&root), child);
            ret.children.push(child_obj);
        }

        ret
    }

    fn fetch_or_build_image(&mut self, tex_ref: &AssimpTexture) -> Option<ImageReference> {
        if let Some(ret) = self.images.get(tex_ref) {
            return Some(ret.clone());
        }

        let tex = tex_ref.0.borrow();

        if tex.height != 0 {
            log::warn!("Uncompressed textures are not supported at this time.");
            return None;
        }

        match &tex.data {
            russimp::material::DataContent::Texel(_) => {
                log::warn!("Uncompressed textures are not supported at this time.");
                None
            }
            russimp::material::DataContent::Bytes(bytes) => {
                let mut lock = self.state.lock().unwrap();
                let buff = lock
                    .buffers
                    .new_component(BufferState::new_from_bytes(bytes.clone()));

                let buffview = lock
                    .buffer_views
                    .new_component(BufferViewState::new_from_whole_buffer(buff));

                let image = lock
                    .images
                    .new_component(ServerImageState::new_from_buffer(buffview));

                self.images.insert(tex_ref.clone(), image.clone());

                Some(image)
            }
        }
    }

    fn build_texture(
        &mut self,
        props: Option<&MatPropSlot>,
        tex: Option<&Rc<RefCell<Texture>>>,
    ) -> Option<ServerTextureRef> {
        let props = props?;
        let tex = tex?;

        let image = self.fetch_or_build_image(&AssimpTexture(tex.clone()))?;

        let mut texture = ServerTextureState {
            name: None,
            image,
            sampler: None,
        };

        {
            // WARNING we need to use a hack here. The library is not providing the values as ints, just as ints -> floats.
            fn compute_sampler_hack(v: f32) -> u32 {
                u32::from_le_bytes(v.to_le_bytes())
            }

            fn compute_mag_filter(v: u32) -> MagFilter {
                match v {
                    9728 => MagFilter::Nearest,
                    9729 => MagFilter::Linear,
                    _ => MagFilter::Linear,
                }
            }

            fn compute_min_filter(v: u32) -> MinFilter {
                match v {
                    9728 => MinFilter::Nearest,
                    9729 => MinFilter::Linear,
                    _ => MinFilter::LinearMipmapLinear,
                }
            }

            fn compute_wrap(v: u32) -> SamplerMode {
                match v {
                    0x0 => SamplerMode::Repeat,
                    0x1 => SamplerMode::Clamp,
                    0x2 => SamplerMode::MirrorRepeat,
                    _ => SamplerMode::Clamp,
                }
            }

            let mag = props
                .find_float(MK_FILTER_MAG)
                .map(compute_sampler_hack)
                .map(compute_mag_filter);
            let min = props
                .find_float(MK_FILTER_MIN)
                .map(compute_sampler_hack)
                .map(compute_min_filter);

            log::debug!("Found texture sampler filters: {min:?} {mag:?}");

            texture.sampler = Some(
                self.state
                    .lock()
                    .unwrap()
                    .samplers
                    .new_component(SamplerState {
                        mag_filter: mag,
                        min_filter: min,
                        wrap_s: props
                            .find_float(MK_WRAP_U)
                            .map(compute_sampler_hack)
                            .map(compute_wrap),
                        wrap_t: props
                            .find_float(MK_WRAP_V)
                            .map(compute_sampler_hack)
                            .map(compute_wrap),
                        ..Default::default()
                    }),
            );
        }

        Some(ServerTextureRef {
            texture: self.state.lock().unwrap().textures.new_component(texture),
            transform: None,
            texture_coord_slot: None,
        })
    }

    fn build_material(&mut self, mat: &russimp::material::Material) {
        let props = MatProps::new(mat);

        // finding the shading model is difficult. For now we just find the keys that make sense for us.

        const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

        let mut name: Option<String> = None;

        let mut pbr = ServerPBRInfo {
            ..Default::default()
        };

        if let Some(props) = props.by_type(TextureType::None) {
            name = props.find_string(MK_NAME);
            pbr.base_color = props
                .find_color(MK_COLOR_BASE)
                .or(props.find_color(MK_COLOR_DIFF))
                .unwrap_or(WHITE);

            pbr.metallic = Some(props.find_float(MK_METALLIC_FACTOR).unwrap_or(0.0));

            pbr.roughness = Some(props.find_float(MK_ROUGHNESS_FACTOR).unwrap_or(0.75));
        }

        // =========
        {
            let base_tex = mat
                .textures
                .get(&TextureType::BaseColor)
                .or(mat.textures.get(&TextureType::Diffuse));

            let base_tex_props = props
                .by_type(TextureType::BaseColor)
                .or(props.by_type(TextureType::Diffuse));

            pbr.base_color_texture = self.build_texture(base_tex_props, base_tex);
        }

        // =========

        let new_mat = ServerMaterialState {
            name,
            mutable: ServerMaterialStateUpdatable {
                pbr_info: Some(pbr),
                double_sided: props
                    .by_type(TextureType::None)
                    .and_then(|x| x.find(MK_DOUBLESIDED))
                    .map(|_| true),
                ..Default::default()
            },
        };

        self.materials
            .push(self.state.lock().unwrap().materials.new_component(new_mat));
    }

    async fn build_mesh(&mut self, mesh: &russimp::mesh::Mesh) {
        let mut verts = Vec::<server_bufferbuilder::VertexFull>::new();

        let def_vert = VertexFull {
            position: [0.0; 3],
            normal: [0.0; 3],
            tangent: [0.0; 3],
            texture: [0, 0],
            color: [255; 4],
        };

        verts.resize(mesh.vertices.len(), def_vert);

        mod_fill(mesh.vertices.as_slice(), verts.as_mut_slice(), |i, o| {
            o.position = [i.x, i.y, i.z];
        });

        mod_fill(mesh.normals.as_slice(), verts.as_mut_slice(), |i, o| {
            o.normal = [i.x, i.y, i.z];
        });

        mod_fill(mesh.tangents.as_slice(), verts.as_mut_slice(), |i, o| {
            o.tangent = [i.x, i.y, i.z];
        });

        // only the first for now
        if let Some(list) = &mesh.texture_coords[0] {
            mod_fill(list.as_slice(), verts.as_mut_slice(), |i, o| {
                o.texture = convert_tex(*i);
            });
        }

        // again only the first
        if let Some(list) = &mesh.colors[0] {
            mod_fill(list.as_slice(), verts.as_mut_slice(), |i, o| {
                o.color = convert_color(*i);
            });
        }

        // fill faces

        let mut new_faces = Vec::<[u32; 3]>::new();

        new_faces.reserve(mesh.faces.len());

        for face in &mesh.faces {
            let mut nf: [u32; 3] = [0, 0, 0];

            fill_array(&face.0, &mut nf);

            new_faces.push(nf);
        }

        // find the material
        let mat = self.materials[mesh.material_index as usize].clone();

        let source = VertexSource {
            name: Some(mesh.name.clone()),
            vertex: verts.as_slice(),
            index: server_bufferbuilder::IndexType::Triangles(new_faces.as_slice()),
        };

        let pack = source.pack_bytes().unwrap();

        let (buffrep, opt_source) = if self.max_mesh_size < (pack.bytes.len() as u64) {
            let link = publish_mesh(pack.bytes, self.link.clone()).await;
            (BufferRepresentation::Url(link.1), Some(link.0))
        } else {
            (BufferRepresentation::Bytes(pack.bytes), None)
        };

        {
            let mut server_state = self.state.lock().unwrap();

            let intermediate = source.build_states(&mut server_state, buffrep).unwrap();

            let patch = ServerGeometryPatch {
                attributes: intermediate.attributes,
                vertex_count: intermediate.vertex_count,
                indices: intermediate.indices,
                patch_type: intermediate.patch_type,
                material: mat,
            };

            log::debug!("Made patch: {patch:?}");

            self.meshes
                .push(server_state.geometries.new_component(ServerGeometryState {
                    name: None,
                    patches: vec![patch],
                }));
        }

        if let Some(pub_id) = opt_source {
            self.published.push(pub_id);
        }
    }

    fn build_materials(&mut self, scene: &Scene) {
        for scene_mat in &scene.materials {
            self.build_material(scene_mat);
        }
    }

    async fn build_meshes(&mut self, scene: &Scene) {
        for scene_mesh in &scene.meshes {
            self.build_mesh(scene_mesh).await;
        }
    }

    pub async fn build(mut self, scene: &Scene) -> ObjectRoot {
        // we need to do materials first, as they will be referenced by meshes
        self.build_materials(scene);
        self.build_meshes(scene).await;

        self.nodes = Some(self.recurse_node(None, scene.root.as_ref().unwrap()));

        ObjectRoot {
            published: self.published,
            root: self.nodes.unwrap(),
        }
    }
}

#[derive(Default)]
struct MatProps {
    props: HashMap<TextureType, MatPropSlot>,
}

#[derive(Default)]
struct MatPropSlot {
    props: HashMap<String, PropertyTypeInfo>,
}

impl MatProps {
    fn new(mat: &russimp::material::Material) -> Self {
        let mut ret = MatProps::default();

        for prop in &mat.properties {
            log::debug!("Adding property {}: {:?}", prop.key, prop);

            let v = ret.props.entry(prop.semantic).or_insert(Default::default());

            v.props.entry(prop.key.clone()).or_insert(prop.data.clone());
        }

        ret
    }

    fn by_type(&self, t: TextureType) -> Option<&MatPropSlot> {
        self.props.get(&t)
    }
}

impl MatPropSlot {
    fn find_string(&self, key: &str) -> Option<String> {
        let v = self.props.get(key)?;
        match v {
            PropertyTypeInfo::String(x) => Some(x.clone()),
            _ => None,
        }
    }

    fn find(&self, key: &str) -> Option<()> {
        log::debug!("Looking up void property {key}");
        let _ = self.props.get(key)?;
        Some(())
    }

    fn _find_int(&self, key: &str) -> Option<i32> {
        let v = self.props.get(key)?;
        log::debug!("Looking up int property {key}: {v:?}");
        match v {
            PropertyTypeInfo::IntegerArray(x) => Some(x[0]),
            PropertyTypeInfo::Buffer(_) => None,
            PropertyTypeInfo::FloatArray(x) => Some(x[0] as i32),
            PropertyTypeInfo::String(x) => x.parse::<i32>().ok(),
        }
    }

    fn find_float(&self, key: &str) -> Option<f32> {
        let v = self.props.get(key)?;
        match v {
            PropertyTypeInfo::FloatArray(x) => Some(x[0]),
            PropertyTypeInfo::IntegerArray(x) => Some(x[0] as f32),
            PropertyTypeInfo::String(x) => x.parse::<f32>().ok(),
            _ => None,
        }
    }

    fn find_color(&self, key: &str) -> Option<[f32; 4]> {
        let v = self.props.get(key)?;
        match v {
            PropertyTypeInfo::FloatArray(x) => {
                let mut ret: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
                fill_array(x, &mut ret);
                Some(ret)
            }
            _ => None,
        }
    }
}

pub async fn publish_mesh(
    bytes: Vec<u8>,
    link: Arc<Mutex<AssetServerLink>>,
) -> (uuid::Uuid, String) {
    let a_id = create_asset_id();

    let link = link
        .lock()
        .unwrap()
        .add_asset(a_id, Asset::new_from_slice(bytes.as_slice()))
        .await;

    (a_id, link)
}
