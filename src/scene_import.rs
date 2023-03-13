use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use colabrodo_common::components::MagFilter;
use colabrodo_common::components::MinFilter;
use colabrodo_common::components::SamplerMode;
use colabrodo_server::server_bufferbuilder;
use russimp::material::PropertyTypeInfo;
use russimp::material::Texture;
use russimp::material::TextureType;
use russimp::scene::PostProcess;
use russimp::scene::Scene;

// =============================================================================

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

// =============================================================================

#[derive(Debug)]
pub enum ImportError {
    UnableToOpenFile(String),
    UnableToImport(String),
}

pub fn import_file(path: &Path) -> Result<IntermediateScene, ImportError> {
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

    let scene = Scene::from_file(path_as_str, flags)
        .map_err(|x| ImportError::UnableToImport(x.to_string()))?;

    let mut intermediate = IntermediateScene::default();

    intermediate.consume(scene);

    Ok(intermediate)
}

// =============================================================================

#[derive(Debug, Default)]
pub struct IntermediateMesh {
    pub material: u32,
    pub verts: Vec<server_bufferbuilder::VertexFull>,
    pub indices: Vec<[u32; 3]>,
}

#[derive(Debug, Default)]
pub struct IntermediateImage {
    pub bytes: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct IntermediateTexture {
    pub image: u32,
    pub sampler: Option<u32>,
}

#[derive(Debug, Default)]
pub struct IntermediateMat {
    pub name: Option<String>,
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub doublesided: bool,

    pub base_color_texture: Option<u32>,
}

#[derive(Debug, Default)]
pub struct IntermediateNode {
    pub name: String,
    pub meshes: Vec<u32>,
    pub children: Vec<IntermediateNode>,
}

#[derive(Debug, Default)]
pub struct IntermediateSampler {
    pub name: Option<String>,

    pub mag_filter: Option<MagFilter>,
    pub min_filter: Option<MinFilter>,

    pub wrap_s: Option<SamplerMode>,
    pub wrap_t: Option<SamplerMode>,
}

#[derive(Default)]
pub struct IntermediateScene {
    pub published: Vec<uuid::Uuid>,
    pub images: Vec<IntermediateImage>,
    pub samplers: Vec<IntermediateSampler>,
    pub textures: Vec<IntermediateTexture>,
    pub mats: Vec<IntermediateMat>,
    pub meshes: Vec<IntermediateMesh>,
    pub nodes: Option<IntermediateNode>,
}

fn recurse_node(node: &Rc<RefCell<russimp::node::Node>>) -> IntermediateNode {
    let n = node.borrow_mut();

    log::debug!("Importing node: {}", n.name);

    let mut ret = IntermediateNode {
        name: n.name.clone(),
        meshes: n.meshes.clone(),
        children: Vec::new(),
    };

    for child in &n.children {
        let child_obj = recurse_node(child);
        ret.children.push(child_obj);
    }

    ret
}

impl IntermediateScene {
    fn build_image(&mut self, tex: Option<&Rc<RefCell<Texture>>>) -> Option<u32> {
        tex?;
        log::debug!("New ASSIMP image");

        let tex = tex.unwrap().borrow();

        let id = self.images.len() as u32;

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
                self.images.push(IntermediateImage {
                    bytes: bytes.clone(),
                });
                Some(id)
            }
        }
    }

    fn build_sampler(&mut self, props: Option<&MatPropSlot>) -> Option<u32> {
        let props = props?;
        log::debug!("New ASSIMP sampler");

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

        let id = self.samplers.len() as u32;

        self.samplers.push(IntermediateSampler {
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
        });

        Some(id)
    }

    fn build_texture(
        &mut self,
        props: Option<&MatPropSlot>,
        tex: Option<&Rc<RefCell<Texture>>>,
    ) -> Option<u32> {
        let id = self.textures.len() as u32;

        let texture = IntermediateTexture {
            image: self.build_image(tex)?,
            sampler: self.build_sampler(props),
        };

        log::debug!("New ASSIMP texture");

        self.textures.push(texture);

        Some(id)
    }

    fn build_material(&mut self, mat: &russimp::material::Material) {
        log::debug!("New ASSIMP material");
        let props = MatProps::new(mat);

        // finding the shading model is difficult. For now we just find the keys that make sense for us.

        const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

        let mut name: Option<String> = None;

        let mut material = IntermediateMat::default();

        if let Some(props) = props.by_type(TextureType::None) {
            name = props.find_string(MK_NAME);
            material.base_color = props
                .find_color(MK_COLOR_BASE)
                .or(props.find_color(MK_COLOR_DIFF))
                .unwrap_or(WHITE);

            material.metallic = props.find_float(MK_METALLIC_FACTOR).unwrap_or(0.0);

            material.roughness = props.find_float(MK_ROUGHNESS_FACTOR).unwrap_or(0.5);

            material.doublesided = props.find(MK_DOUBLESIDED).is_some();
        }

        material.name = name;

        // =========
        {
            let base_tex = mat
                .textures
                .get(&TextureType::BaseColor)
                .or(mat.textures.get(&TextureType::Diffuse));

            let base_tex_props = props
                .by_type(TextureType::BaseColor)
                .or(props.by_type(TextureType::Diffuse));

            material.base_color_texture = self.build_texture(base_tex_props, base_tex);
        }

        // =========

        self.mats.push(material);
    }

    fn consume_mesh(&mut self, mesh: &russimp::mesh::Mesh) {
        log::debug!("New ASSIMP mesh");
        let mut verts = Vec::<server_bufferbuilder::VertexFull>::new();

        let def_vert = server_bufferbuilder::VertexFull {
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
        log::debug!("Mesh: Material {}", mesh.material_index);

        self.meshes.push(IntermediateMesh {
            material: mesh.material_index,
            verts,
            indices: new_faces,
        });
    }

    fn consume_materials(&mut self, scene: &Scene) {
        log::debug!("Total materials: {}", scene.materials.len());
        for mat in &scene.materials {
            self.build_material(mat);
        }
    }
    fn consume_meshs(&mut self, scene: &mut Scene) {
        log::debug!("Total meshes: {}", scene.meshes.len());
        for mesh in &scene.meshes {
            self.consume_mesh(mesh);
        }
    }
    fn consume(&mut self, mut scene: Scene) {
        // we need to do materials first, as they will be referenced by meshes
        self.consume_materials(&scene);
        self.consume_meshs(&mut scene);

        self.nodes = Some(recurse_node(scene.root.as_ref().unwrap()));
    }
}

// =============================================================================
#[inline]
fn mod_fill<T, F>(from: &[T], to: &mut [server_bufferbuilder::VertexFull], f: F)
where
    F: Fn(&T, &mut server_bufferbuilder::VertexFull),
{
    for (a, b) in from.iter().zip(to.iter_mut()) {
        f(a, b);
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

// =============================================================================

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
