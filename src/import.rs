use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use colabrodo_core::common::strings::TAG_USER_HIDDEN;
use colabrodo_core::server_bufferbuilder;
use colabrodo_core::server_messages::ComponentReference;
use colabrodo_core::server_messages::EntityRepresentation;
use colabrodo_core::server_messages::EntityState;
use colabrodo_core::server_messages::GeometryPatch;
use colabrodo_core::server_messages::GeometryState;
use colabrodo_core::server_messages::MaterialState;
use colabrodo_core::server_messages::MaterialStateUpdatable;
use colabrodo_core::server_messages::PBRInfo;
use colabrodo_core::server_messages::RenderRepresentation;
use colabrodo_core::server_state::ServerState;
use russimp::material::PropertyTypeInfo;
use russimp::scene::PostProcess;
use russimp::scene::Scene;

const MK_NAME: &str = "?mat.name";
const MK_COLOR_DIFF: &str = "$clr.diffuse";
const MK_COLOR_BASE: &str = "$clr.base";

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
            PostProcess::SortByPrimitiveType,
            PostProcess::GenerateBoundingBoxes,
        ];

        let scene = Scene::from_file(path_as_str, flags);

        match scene {
            Err(x) => Err(ImportError::UnableToImport(x.to_string())),
            Ok(x) => Ok(Self { data: x }),
        }
    }

    pub fn build_objects(&self, state: &mut ServerState) -> Object {
        let mut scratch = ImportScratch::default();

        scratch.build(&self.data, state);

        scratch.nodes.unwrap()
    }
}

pub struct Object {
    pub parts: Vec<ComponentReference<EntityState>>,
    pub children: Vec<Object>,
}

#[derive(Default)]
struct ImportScratch {
    materials: Vec<ComponentReference<MaterialState>>,
    meshes: Vec<ComponentReference<GeometryState>>,

    nodes: Option<Object>,
}

impl ImportScratch {
    fn recurse_node(
        &mut self,
        parent: Option<&ComponentReference<EntityState>>,
        node: &Rc<RefCell<russimp::node::Node>>,
        state: &mut ServerState,
    ) -> Object {
        let n = node.borrow();

        log::debug!("Importing node: {}", n.name);

        let mut ent = EntityState {
            name: Some(n.name.clone()),
            ..Default::default()
        };

        if let Some(x) = parent {
            ent.extra.parent = Some(x.clone());
        }

        let root = state.entities.new_component(ent);

        let mut ret = Object {
            parts: vec![root.clone()],
            children: Vec::new(),
        };

        for mid in &n.meshes {
            let mut sub_ent = EntityState::default();

            sub_ent.extra.parent = Some(root.clone());
            sub_ent.extra.tags = Some(vec![TAG_USER_HIDDEN.to_string()]);

            sub_ent.extra.representation =
                Some(EntityRepresentation::Render(RenderRepresentation {
                    mesh: self.meshes[*mid as usize].clone(),
                    instances: None,
                }));

            ret.parts.push(state.entities.new_component(sub_ent));
        }

        //n.name;
        //server_bufferbuilder::create_mesh(state, source, material)
        //source.

        for child in &n.children {
            let child_obj = self.recurse_node(Some(&root), child, state);
            ret.children.push(child_obj);
        }

        ret
    }

    fn build_material(&mut self, mat: &russimp::material::Material, state: &mut ServerState) {
        //mat.textures.get(TextureTye)

        let props = MatProps::new(mat);

        // finding the shading model is difficult. For now we just find the keys that make sense for us.

        const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

        let mut pbr = PBRInfo {
            metallic: Some(0.0),
            roughness: Some(0.75),
            ..Default::default()
        };

        pbr.base_color = props
            .find_color(MK_COLOR_BASE)
            .or(props.find_color(MK_COLOR_DIFF))
            .unwrap_or(WHITE);

        let new_mat = MaterialState {
            name: props.find_string(MK_NAME),
            extra: MaterialStateUpdatable {
                pbr_info: Some(pbr),
                ..Default::default()
            },
        };

        self.materials.push(state.materials.new_component(new_mat));
    }

    fn build_mesh(&mut self, mesh: &russimp::mesh::Mesh, state: &mut ServerState) {
        let mut source = server_bufferbuilder::VertexSource {
            positions: convert_vec3d(&mesh.vertices),
            ..Default::default()
        };

        if !mesh.normals.is_empty() {
            source.normals = convert_vec3d(&mesh.normals);
        }

        // TODO: TANGENTS
        // if !mesh.tangents.is_empty() {
        //     source. = convert_vec3d(&mesh.normals);
        // }

        // only the first for now
        if !mesh.texture_coords.is_empty() {
            if let Some(list) = &mesh.texture_coords[0] {
                source.textures = convert_tex(list);
            }
        }

        // again only the first
        if !mesh.colors.is_empty() {
            if let Some(list) = &mesh.colors[0] {
                source.colors = convert_color(list);
            }
        }

        for face in &mesh.faces {
            let mut nf: [u32; 3] = [0, 0, 0];

            fill_array(&face.0, &mut nf);

            source.triangles.push(nf);
        }

        // find the material
        let mat = self.materials[mesh.material_index as usize].clone();

        let packed_mesh_info = server_bufferbuilder::create_mesh(state, source);

        let patch = GeometryPatch {
            attributes: packed_mesh_info.attributes,
            vertex_count: packed_mesh_info.vertex_count,
            indices: packed_mesh_info.indices,
            patch_type: packed_mesh_info.patch_type,
            material: mat,
        };

        log::debug!("Made patch: {patch:?}");

        self.meshes
            .push(state.geometries.new_component(GeometryState {
                name: None,
                patches: vec![patch],
            }));
    }

    fn build_materials(&mut self, scene: &Scene, state: &mut ServerState) {
        for scene_mat in &scene.materials {
            self.build_material(scene_mat, state);
        }
    }

    fn build_meshes(&mut self, scene: &Scene, state: &mut ServerState) {
        for scene_mesh in &scene.meshes {
            self.build_mesh(scene_mesh, state);
        }
    }

    pub fn build(&mut self, scene: &Scene, state: &mut ServerState) {
        // we need to do materials first, as they will be referenced by meshes
        self.build_materials(scene, state);
        self.build_meshes(scene, state);

        self.nodes = Some(self.recurse_node(None, scene.root.as_ref().unwrap(), state));
    }
}

struct MatProps {
    props: HashMap<String, PropertyTypeInfo>,
}

impl MatProps {
    fn new(mat: &russimp::material::Material) -> Self {
        let mut ret = MatProps {
            props: Default::default(),
        };

        for prop in &mat.properties {
            println!("Adding property {}", prop.key);
            ret.props.insert(prop.key.clone(), prop.data.clone());
        }

        ret
    }

    fn find_string(&self, key: &str) -> Option<String> {
        let v = self.props.get(key)?;
        match v {
            PropertyTypeInfo::String(x) => Some(x.clone()),
            _ => None,
        }
    }

    fn _find_int(&self, key: &str) -> Option<i32> {
        let v = self.props.get(key)?;
        match v {
            PropertyTypeInfo::IntegerArray(x) => Some(x[0]),
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

fn convert_vec3d(list: &[russimp::Vector3D]) -> Vec<[f32; 3]> {
    list.iter().map(|v| [v.x, v.y, v.z]).collect()
}

fn convert_tex(list: &[russimp::Vector3D]) -> Vec<[u16; 2]> {
    list.iter()
        .map(|v| [normalize_to_u16(v.x), normalize_to_u16(v.y)])
        .collect()
}

fn convert_color(list: &[russimp::Color4D]) -> Vec<[u8; 4]> {
    list.iter()
        .map(|v| {
            [
                normalize_to_u8(v.r),
                normalize_to_u8(v.g),
                normalize_to_u8(v.b),
                normalize_to_u8(v.a),
            ]
        })
        .collect()
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
