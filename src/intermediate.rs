use colabrodo_common::components::{MagFilter, MinFilter, SamplerMode};
use colabrodo_server::server_bufferbuilder;


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