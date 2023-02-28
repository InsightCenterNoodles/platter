use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use colabrodo_common::common::strings::TAG_USER_HIDDEN;
use colabrodo_common::components::BufferViewState;
use colabrodo_common::components::MagFilter;
use colabrodo_common::components::MinFilter;
use colabrodo_common::components::SamplerMode;
use colabrodo_server::server_bufferbuilder;
use colabrodo_server::server_bufferbuilder::VertexFull;
use colabrodo_server::server_bufferbuilder::VertexSource;
use colabrodo_server::server_messages::*;
use colabrodo_server::server_state::*;
use russimp::material::PropertyTypeInfo;
use russimp::material::Texture;
use russimp::material::TextureType;
use russimp::scene::PostProcess;
use russimp::scene::Scene;

use crate::object::Object;
use crate::object::ObjectRoot;
use crate::platter_state::PlatterState;

const MK_NAME: &str = "?mat.name";

const MK_DOUBLESIDED: &str = "$mat.twosided";

const MK_COLOR_DIFF: &str = "$clr.diffuse";
const MK_COLOR_BASE: &str = "$clr.base";

const MK_METALLIC_FACTOR: &str = "$mat.metallicFactor";
const MK_ROUGHNESS_FACTOR: &str = "$mat.roughnessFactor";

const MK_FILTER_MAG: &str = "$tex.mappingfiltermag";
const MK_FILTER_MIN: &str = "$tex.mappingfiltermin";

const MK_WRAP_U: &str = "$tex.mapmodeu";
const MK_WRAP_V: &str = "$tex.mapmodev";

// Eventually we can do this
//const M_SAMPLER_FILTER_NEAREST: f32 = f32::from_le_bytes(9728_i32.to_le_bytes());

#[derive(Debug)]
pub enum ImportError {
    UnableToOpenFile(String),
    UnableToImport(String),
}

pub struct ImportedScene {
    data: Scene,
}

impl ImportedScene {
    pub fn import_file(path: &Path) -> Result<Self, ImportError> {
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
            Ok(x) => Ok(Self { data: x }),
        }
    }

    pub fn build_objects(&self, state: &mut PlatterState) -> ObjectRoot {
        let scratch = ImportScratch::default();

        scratch.build(&self.data, state)
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

#[derive(Default)]
struct ImportScratch {
    published: Vec<uuid::Uuid>,
    images: HashMap<AssimpTexture, ComponentReference<ServerImageState>>,
    materials: Vec<ComponentReference<ServerMaterialState>>,
    meshes: Vec<ComponentReference<ServerGeometryState>>,

    nodes: Option<Object>,
}

impl ImportScratch {
    fn recurse_node(
        &mut self,
        parent: Option<&ComponentReference<ServerEntityState>>,
        node: &Rc<RefCell<russimp::node::Node>>,
        state: &mut ServerState,
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

        let root = state.entities.new_component(ent);

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

            ret.parts.push(state.entities.new_component(sub_ent));
        }

        for child in &n.children {
            let child_obj = self.recurse_node(Some(&root), child, state);
            ret.children.push(child_obj);
        }

        ret
    }

    fn fetch_or_build_image(
        &mut self,
        tex_ref: &AssimpTexture,
        state: &mut ServerState,
    ) -> Option<ComponentReference<ServerImageState>> {
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
                let buff = state
                    .buffers
                    .new_component(BufferState::new_from_bytes(bytes.clone()));

                let buffview = state
                    .buffer_views
                    .new_component(BufferViewState::new_from_whole_buffer(buff));

                let image = state
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
        state: &mut ServerState,
    ) -> Option<ServerTextureRef> {
        let props = props?;
        let tex = tex?;

        let image = self.fetch_or_build_image(&AssimpTexture(tex.clone()), state)?;

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
                state.samplers.new_component(SamplerState {
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
            texture: state.textures.new_component(texture),
            transform: None,
            texture_coord_slot: None,
        })
    }

    fn build_material(&mut self, mat: &russimp::material::Material, state: &mut ServerState) {
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

            pbr.base_color_texture = self.build_texture(base_tex_props, base_tex, state);
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

        self.materials.push(state.materials.new_component(new_mat));
    }

    fn build_mesh(&mut self, mesh: &russimp::mesh::Mesh, state: &mut PlatterState) {
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

        let (packed_mesh_info, pub_id) = state.generate_mesh(source);

        if let Some(pub_id) = pub_id {
            self.published.push(pub_id);
        }

        let patch = ServerGeometryPatch {
            attributes: packed_mesh_info.attributes,
            vertex_count: packed_mesh_info.vertex_count,
            indices: packed_mesh_info.indices,
            patch_type: packed_mesh_info.patch_type,
            material: mat,
        };

        log::debug!("Made patch: {patch:?}");

        self.meshes.push(
            state
                .mut_state()
                .geometries
                .new_component(ServerGeometryState {
                    name: None,
                    patches: vec![patch],
                }),
        );
    }

    fn build_materials(&mut self, scene: &Scene, state: &mut ServerState) {
        for scene_mat in &scene.materials {
            self.build_material(scene_mat, state);
        }
    }

    fn build_meshes(&mut self, scene: &Scene, state: &mut PlatterState) {
        for scene_mesh in &scene.meshes {
            self.build_mesh(scene_mesh, state);
        }
    }

    pub fn build(mut self, scene: &Scene, state: &mut PlatterState) -> ObjectRoot {
        // we need to do materials first, as they will be referenced by meshes
        self.build_materials(scene, &mut state.mut_state());
        self.build_meshes(scene, state);

        self.nodes = Some(self.recurse_node(None, scene.root.as_ref().unwrap(), state.mut_state()));

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

#[inline]
fn normalize_to_u8(v: f32) -> u8 {
    (v * (u8::MAX as f32)) as u8
}

#[inline]
fn normalize_to_u16(v: f32) -> u16 {
    (v * (u16::MAX as f32)) as u16
}

fn mod_fill<T, F>(from: &[T], to: &mut [VertexFull], f: F)
where
    F: Fn(&T, &mut VertexFull),
{
    for (a, b) in from.iter().zip(to.iter_mut()) {
        f(a, b);
    }
}

#[inline]
fn convert_tex(v: russimp::Vector3D) -> [u16; 2] {
    [normalize_to_u16(v.x), normalize_to_u16(v.y)]
}

#[inline]
fn convert_color(v: russimp::Color4D) -> [u8; 4] {
    [
        normalize_to_u8(v.r),
        normalize_to_u8(v.g),
        normalize_to_u8(v.b),
        normalize_to_u8(v.a),
    ]
}

#[inline]
fn fill_array<T, const N: usize>(src: &Vec<T>, dst: &mut [T; N])
where
    T: Copy,
{
    for (d, s) in dst.iter_mut().zip(src) {
        *d = *s;
    }
}
