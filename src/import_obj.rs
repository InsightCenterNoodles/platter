use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader},
    mem::take,
    path::Path,
    str::SplitWhitespace,
};

use anyhow::{Context, Result};

use nalgebra_glm;
use nalgebra_glm::Vec3;

use crate::scene::{Scene, SceneObject};

use colabrodo_common::components::*;
use colabrodo_server::{
    server_bufferbuilder::*, server_http::*, server_messages::*, server_state::*,
};

/// Import a wavefront OBJ file
pub fn import_file(
    path: &Path,
    state: ServerStatePtr,
    asset_store: AssetStorePtr,
) -> Result<Scene> {
    let file = File::open(path)?;
    let mut buf_reader = BufReader::new(file);

    let mut line = String::new();

    let mut wfobj = WFObjectState::new();

    loop {
        line.clear();
        let count = buf_reader.read_line(&mut line).unwrap_or_default();
        if count == 0 {
            break;
        }
        if line.starts_with('#') {
            continue;
        }

        wfobj.handle(&line);
    }

    let all_objs = pack_wf_state(wfobj);

    let mut lock = state.lock().unwrap();

    let published = Vec::<uuid::Uuid>::new();

    let mut root = SceneObject {
        parts: vec![],
        children: vec![],
    };

    for sub_obj in all_objs {
        let source = VertexSource {
            name: None,
            vertex: &sub_obj.verts,
            index: IndexType::Triangles(&sub_obj.faces),
        };

        let bytes = source.pack_bytes().context("Packing bytes")?;

        let asset_id = create_asset_id();

        let url = add_asset(
            asset_store.clone(),
            asset_id,
            Asset::new_from_slice(&bytes.bytes),
        );

        let material = lock.materials.new_component(ServerMaterialState {
            name: None,
            mutable: ServerMaterialStateUpdatable {
                pbr_info: Some(PBRInfo {
                    base_color: [1.0, 1.0, 1.0, 1.0],
                    metallic: Some(0.0),
                    roughness: Some(1.0),
                    ..Default::default()
                }),
                ..Default::default()
            },
        });

        let geom_ref = source
            .build_geometry(&mut lock, BufferRepresentation::Url(url), material)
            .context("Building geometry")?;

        let entity = lock.entities.new_component(ServerEntityState {
            name: Some(sub_obj.name),
            mutable: ServerEntityStateUpdatable {
                representation: Some(ServerEntityRepresentation::new_render(
                    RenderRepresentation {
                        mesh: geom_ref,
                        instances: None,
                    },
                )),
                ..Default::default()
            },
        });

        root.parts.push(entity);
    }

    Ok(Scene::new(root, published, asset_store))
}

type WFFunc = fn(obj: &mut WFObjectState, line: SplitWhitespace) -> Option<()>;

fn handle_v(obj: &mut WFObjectState, line: SplitWhitespace) -> Option<()> {
    let mut v = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];

    let mut c = 0;

    for (i, f) in line.take(6).enumerate() {
        v[i] = f.parse().unwrap_or_default();
        c = i;
    }

    match c {
        4 => {
            // has a W coord case.
            v[0] /= v[3];
            v[1] /= v[3];
            v[2] /= v[3];
            v[3] = 1.0;
        }
        6 => {
            // has color. Clamp?
        }
        _ => (),
    }

    obj.vert_list.push([v[0], v[1], v[2]]);

    Some(())
}

fn handle_vn(obj: &mut WFObjectState, mut line: SplitWhitespace) -> Option<()> {
    let n: [f32; 3] = [
        line.next().unwrap_or_default().parse().unwrap_or_default(),
        line.next().unwrap_or_default().parse().unwrap_or_default(),
        line.next().unwrap_or_default().parse().unwrap_or_default(),
    ];

    obj.normal_list.push(n);

    Some(())
}

fn handle_vt(obj: &mut WFObjectState, mut line: SplitWhitespace) -> Option<()> {
    let t: [f32; 3] = [
        line.next().unwrap_or_default().parse().unwrap_or_default(),
        line.next().unwrap_or_default().parse().unwrap_or_default(),
        line.next().unwrap_or_default().parse().unwrap_or_default(),
    ];

    obj.tex_list.push(t);

    Some(())
}

#[derive(Debug, Clone, Eq, Hash, PartialEq)]
struct FaceDef {
    v: Option<i32>,
    t: Option<i32>,
    n: Option<i32>,
}

impl FaceDef {
    fn new(definition: &str) -> Self {
        let mut iter = definition.split('/').take(3).map(|f| f.parse::<i32>().ok());

        let a = iter.next().flatten();
        let b = iter.next().flatten();
        let c = iter.next().flatten();

        match (a.is_some(), b.is_some(), c.is_some()) {
            (true, true, true) => Self { v: a, t: b, n: c },
            (true, false, true) => Self {
                v: a,
                t: None,
                n: c,
            },
            (true, true, false) => Self {
                v: a,
                t: b,
                n: None,
            },
            (true, false, false) => Self {
                v: a,
                t: None,
                n: None,
            },
            (_, _, _) => Self {
                v: None,
                t: None,
                n: None,
            },
        }
    }

    fn sanitize(
        self,
        vert_list: &[[f32; 3]],
        normal_list: &[[f32; 3]],
        tex_list: &[[f32; 3]],
    ) -> Self {
        Self {
            v: self.v.map(|x| {
                if x < 0 {
                    vert_list.len() as i32 + x
                } else {
                    x - 1
                }
            }),

            n: self.n.map(|x| {
                if x < 0 {
                    normal_list.len() as i32 + x
                } else {
                    x - 1
                }
            }),

            t: self.t.map(|x| {
                if x < 0 {
                    tex_list.len() as i32 + x
                } else {
                    x - 1
                }
            }),
        }
    }
}

#[derive(Debug, Clone)]
enum FaceMarker {
    Def(FaceDef),
    End,
}

fn handle_f(obj: &mut WFObjectState, line: SplitWhitespace) -> Option<()> {
    // slightly awkward here to avoid double borrow of obj
    obj.last_face_list.extend(line.map(|f| {
        FaceMarker::Def(FaceDef::new(f).sanitize(&obj.vert_list, &obj.normal_list, &obj.tex_list))
    }));
    obj.last_face_list.push(FaceMarker::End);

    Some(())
}

fn handle_o(obj: &mut WFObjectState, mut line: SplitWhitespace) -> Option<()> {
    obj.push_object();
    obj.last_name = line.next().unwrap_or("Unknown").to_string();
    Some(())
}

struct WFObjectState {
    fn_map: HashMap<String, WFFunc>,

    vert_list: Vec<[f32; 3]>,
    normal_list: Vec<[f32; 3]>,
    tex_list: Vec<[f32; 3]>,

    obj_face_list: HashMap<String, Vec<FaceMarker>>,
    last_name: String,
    last_face_list: Vec<FaceMarker>,
}

impl WFObjectState {
    fn new() -> Self {
        let mut fn_map = HashMap::<String, WFFunc>::new();

        fn_map.insert("v".to_string(), handle_v);
        fn_map.insert("vn".to_string(), handle_vn);
        fn_map.insert("vt".to_string(), handle_vt);
        fn_map.insert("f".to_string(), handle_f);
        fn_map.insert("o".to_string(), handle_o);

        Self {
            fn_map,
            vert_list: Default::default(),
            normal_list: Default::default(),
            tex_list: Default::default(),
            obj_face_list: Default::default(),
            last_name: Default::default(),
            last_face_list: Default::default(),
        }
    }

    fn handle(&mut self, line: &str) -> Option<()> {
        let mut iter = line.split_whitespace();
        let directive = iter.next()?;

        let ptr = self.fn_map.get(directive)?;

        (ptr)(self, iter)
    }

    fn push_object(&mut self) {
        if self.last_face_list.is_empty() {
            return;
        }

        let mut name = self.last_name.as_str();
        if name.is_empty() {
            name = "Unknown";
        }

        let local_vec = take(&mut self.last_face_list);

        self.obj_face_list.insert(name.to_string(), local_vec);
    }
}

fn assemble_vertex(obj: &WFObjectState, f: FaceDef) -> VertexTexture {
    VertexTexture {
        position: f
            .v
            .map(|x| obj.vert_list[x as usize])
            .unwrap_or([0.0, 0.0, 0.0]),
        normal: f
            .n
            .map(|x| obj.normal_list[x as usize])
            .unwrap_or([0.0, 0.0, 0.0]),
        texture: f
            .t
            .map(|x| {
                let source = obj.tex_list[x as usize];
                [
                    (source[0] * (65536.0 - 1.0)) as u16,
                    (source[1] * (65536.0 - 1.0)) as u16,
                ]
            })
            .unwrap_or([0, 0]),
    }
}

fn get_concave_vertex(indicies: &[u32], vs: &[VertexTexture]) -> [u32; 4] {
    for window in indicies.windows(4) {
        let v = Vec3::from(vs[window[0] as usize].position);
        let v2 = Vec3::from(vs[window[1] as usize].position);
        let v1 = Vec3::from(vs[window[2] as usize].position);
        let v0 = Vec3::from(vs[window[3] as usize].position);

        let left = (v0 - v).normalize();
        let diag = (v1 - v).normalize();
        let right = (v2 - v).normalize();

        let angle = left.dot(&diag).acos() + right.dot(&diag).acos();

        if angle > std::f32::consts::PI {
            return [window[0], window[1], window[2], window[3]];
        }
    }
    [indicies[0], indicies[1], indicies[2], indicies[3]]
}

// Following the assimp code for quads
fn compute_quad(indicies: &[u32], vs: &[VertexTexture]) -> ([u32; 3], [u32; 3]) {
    assert_eq!(indicies.len(), 4);

    let start_vertex = get_concave_vertex(indicies, vs);

    //let temp = [indicies[0], indicies[1], indicies[2], indicies[3]];

    let f1 = [start_vertex[0], start_vertex[1], start_vertex[2]];
    let f2 = [start_vertex[0], start_vertex[2], start_vertex[3]];

    (f1, f2)
}

struct PackedObj {
    name: String,
    verts: Vec<VertexTexture>,
    faces: Vec<[u32; 3]>,
}

fn pack_wf_state(mut obj: WFObjectState) -> Vec<PackedObj> {
    let mut vert_list = Vec::<VertexTexture>::new();
    let mut faces = Vec::<[u32; 3]>::new();

    let mut face_remapper = HashMap::<FaceDef, u32>::new();

    let mut counter;

    let mut this_face_cache = Vec::<u32>::new();

    obj.push_object();

    let mut ret = Vec::<PackedObj>::new();

    for (name, this_obj_faces) in take(&mut obj.obj_face_list) {
        this_face_cache.clear();
        counter = 0;
        vert_list.clear();
        faces.clear();

        for face in this_obj_faces {
            match face {
                FaceMarker::Def(face) => {
                    this_face_cache.push(*face_remapper.entry(face.clone()).or_insert_with(|| {
                        vert_list.push(assemble_vertex(&obj, face.clone()));

                        let place = counter;
                        counter += 1;
                        place
                    }));
                }
                FaceMarker::End => {
                    if this_face_cache.len() == 3 {
                        // tri
                        faces.push([this_face_cache[0], this_face_cache[1], this_face_cache[2]]);
                    } else if this_face_cache.len() == 4 {
                        let (f1, f2) = compute_quad(&this_face_cache, &vert_list);

                        faces.push(f1);
                        faces.push(f2);
                    }

                    this_face_cache.clear();
                }
            }
        }

        ret.push(PackedObj {
            name,
            verts: take(&mut vert_list),
            faces: take(&mut faces),
        })
    }

    ret
}
