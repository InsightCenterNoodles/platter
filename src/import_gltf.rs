use std::{collections::HashMap, path::Path};

use anyhow::Result;

use crate::object::{Object, ObjectRoot};
use colabrodo_common::{components::*, types::Format};
use colabrodo_server::{server_http::*, server_messages::*, server_state::*};
use gltf;

/// Trait to convert GLTF enums and values to corresponding NOODLES values
trait ToNoodles {
    /// NOODLES result type of this conversion
    type Value;
    fn into_noodles(self) -> Self::Value;
}

impl ToNoodles for gltf::texture::MinFilter {
    type Value = MinFilter;
    fn into_noodles(self) -> Self::Value {
        match self {
            gltf::texture::MinFilter::Nearest => MinFilter::Nearest,
            gltf::texture::MinFilter::Linear => MinFilter::Linear,
            _ => MinFilter::LinearMipmapLinear,
        }
    }
}

impl ToNoodles for gltf::texture::MagFilter {
    type Value = MagFilter;
    fn into_noodles(self) -> Self::Value {
        match self {
            gltf::texture::MagFilter::Nearest => MagFilter::Nearest,
            gltf::texture::MagFilter::Linear => MagFilter::Linear,
        }
    }
}

impl ToNoodles for gltf::texture::WrappingMode {
    type Value = SamplerMode;
    fn into_noodles(self) -> Self::Value {
        match self {
            gltf::texture::WrappingMode::ClampToEdge => SamplerMode::Clamp,
            gltf::texture::WrappingMode::MirroredRepeat => SamplerMode::MirrorRepeat,
            gltf::texture::WrappingMode::Repeat => SamplerMode::Repeat,
        }
    }
}

impl ToNoodles for gltf::mesh::Semantic {
    type Value = Option<(AttributeSemantic, Option<u32>)>;
    fn into_noodles(self) -> Self::Value {
        match self {
            gltf::Semantic::Positions => Some((AttributeSemantic::Position, None)),
            gltf::Semantic::Normals => Some((AttributeSemantic::Normal, None)),
            gltf::Semantic::Tangents => Some((AttributeSemantic::Tangent, None)),
            gltf::Semantic::Colors(x) => Some((AttributeSemantic::Color, Some(x))),
            gltf::Semantic::TexCoords(x) => Some((AttributeSemantic::Texture, Some(x))),
            _ => None,
        }
    }
}

impl ToNoodles for gltf::mesh::Mode {
    type Value = Option<PrimitiveType>;

    fn into_noodles(self) -> Self::Value {
        match self {
            gltf::mesh::Mode::Points => Some(PrimitiveType::Points),
            gltf::mesh::Mode::Lines => Some(PrimitiveType::Lines),
            gltf::mesh::Mode::LineLoop => None,
            gltf::mesh::Mode::LineStrip => Some(PrimitiveType::LineStrip),
            gltf::mesh::Mode::Triangles => Some(PrimitiveType::Triangles),
            gltf::mesh::Mode::TriangleStrip => Some(PrimitiveType::TriangleStrip),
            gltf::mesh::Mode::TriangleFan => None,
        }
    }
}

impl<'a> ToNoodles for gltf::accessor::Accessor<'a> {
    type Value = Option<Format>;

    fn into_noodles(self) -> Self::Value {
        match (self.data_type(), self.dimensions()) {
            (gltf::accessor::DataType::U8, gltf::accessor::Dimensions::Scalar) => Some(Format::U8),
            (gltf::accessor::DataType::U8, gltf::accessor::Dimensions::Vec4) => {
                Some(Format::U8VEC4)
            }
            (gltf::accessor::DataType::U16, gltf::accessor::Dimensions::Scalar) => {
                Some(Format::U16)
            }
            (gltf::accessor::DataType::U16, gltf::accessor::Dimensions::Vec2) => {
                Some(Format::U16VEC2)
            }
            (gltf::accessor::DataType::U32, gltf::accessor::Dimensions::Scalar) => {
                Some(Format::U32)
            }
            (gltf::accessor::DataType::F32, gltf::accessor::Dimensions::Vec2) => Some(Format::VEC2),
            (gltf::accessor::DataType::F32, gltf::accessor::Dimensions::Vec3) => Some(Format::VEC3),
            (gltf::accessor::DataType::F32, gltf::accessor::Dimensions::Vec4) => Some(Format::VEC4),
            (gltf::accessor::DataType::F32, gltf::accessor::Dimensions::Mat3) => Some(Format::MAT3),
            (gltf::accessor::DataType::F32, gltf::accessor::Dimensions::Mat4) => Some(Format::MAT4),
            (_, _) => None,
        }
    }
}

// =============================================================================

/// Build a NOODLES texture reference from a list of NOODLES textures from a GLTF 'texture reference'.
fn fetch_texture_by_info(
    tex_list: &[TextureReference],
    gltf_tex: &gltf::texture::Info,
) -> ServerTextureRef {
    ServerTextureRef {
        texture: tex_list[gltf_tex.texture().index()].clone(),
        transform: None,
        texture_coord_slot: Some(gltf_tex.tex_coord()),
    }
}

/// Build a NOODLES texture reference from the GLTF normal texture reference.
fn fetch_normal_texture(
    tex_list: &[TextureReference],
    gltf_tex: &gltf::material::NormalTexture,
) -> ServerTextureRef {
    ServerTextureRef {
        texture: tex_list[gltf_tex.texture().index()].clone(),
        transform: None,
        texture_coord_slot: None,
    }
}

/// Build a NOODLES texture reference from a GLTF occlusion texture reference.
fn fetch_occ_texture(
    tex_list: &[TextureReference],
    gltf_tex: &gltf::material::OcclusionTexture,
) -> ServerTextureRef {
    ServerTextureRef {
        texture: tex_list[gltf_tex.texture().index()].clone(),
        transform: None,
        texture_coord_slot: None,
    }
}

/// Create a default material if a GLTF material is missing
fn make_default_material(state: &mut ServerState) -> MaterialReference {
    state.materials.new_component(ServerMaterialState {
        name: Some("Default".into()),
        mutable: ServerMaterialStateUpdatable {
            pbr_info: Some(PBRInfo {
                base_color: [1.0; 4],
                metallic: Some(1.0),
                roughness: Some(1.0),
                ..Default::default()
            }),
            ..Default::default()
        },
    })
}

/// Convert a GLTF Primitive to a NOODLES geometry patch
///
/// Takes a list of buffer views to refer to, the GLTF primitive, and the material to use when building the patch.
fn convert_geometry_patch(
    buffer_views: &[BufferViewReference],
    prim: &gltf::Primitive,
    mat: MaterialReference,
) -> Option<ServerGeometryPatch> {
    let mut attrib = Vec::<ServerGeometryAttribute>::new();

    // We need to send the vertex count. We'll try to extract this count
    // from the position attribute later on.
    let mut pos_count: Option<u64> = None;

    for (attr_sem, attr_accessor) in prim.attributes() {
        // If this is a position, steal the vertex count.
        if attr_sem == gltf::Semantic::Positions {
            log::debug!(
                "Found position attribute. Vertex count {}",
                attr_accessor.count()
            );
            pos_count = Some(attr_accessor.count() as u64)
        }

        // Get the attribute semantic and corresponding slot
        let (n_sem, n_slot) = match attr_sem.into_noodles() {
            Some(x) => x,
            None => continue,
        };

        // What is the attribute format?
        let format = match attr_accessor.clone().into_noodles() {
            Some(x) => x,
            None => {
                log::warn!("No way to convert GLTF accessor to NOODLES");
                continue;
            }
        };

        // Get the GLTF buffer view
        let g_view = match attr_accessor.view() {
            Some(x) => x,
            None => {
                log::warn!("Unable to handle sparse views at this time.");
                continue;
            }
        };

        log::debug!(
            "Attribute semantic {:?}, format: {:?}, stride {}",
            n_sem,
            format,
            g_view.stride().unwrap_or_default()
        );

        let buffer_view = buffer_views[g_view.index()].clone();

        let n_attr = ServerGeometryAttribute {
            view: buffer_view,
            semantic: n_sem,
            channel: n_slot,
            offset: Some(attr_accessor.offset() as u32),
            stride: g_view.stride().map(|f| f as u32),
            format,
            normalized: Some(attr_accessor.normalized()),
            minimum_value: None,
            maximum_value: None,
        };

        attrib.push(n_attr);
    }

    // Optional indexed geometry processing
    let n_index = prim.indices().and_then(|f| {
        // Get the GLTF buffer view of the indicies
        let g_view = match f.view() {
            Some(x) => x,
            None => {
                log::warn!("Unable to handle sparse views at this time.");
                return None;
            }
        };

        // Format of the index data
        let format = match f.clone().into_noodles() {
            Some(x) => x,
            None => {
                log::warn!("No way to convert GLTF accessor to NOODLES");
                return None;
            }
        };

        log::debug!(
            "Index buffer found: Format {:?}, Count: {}",
            format,
            f.count()
        );

        Some(ServerGeometryIndex {
            view: buffer_views[g_view.index()].clone(),
            count: f.count() as u32,
            offset: Some(f.offset() as u32),
            stride: g_view.stride().map(|f| f as u32),
            format,
        })
    });

    // Assemble the patch
    Some(ServerGeometryPatch {
        attributes: attrib,
        vertex_count: pos_count.unwrap_or_default(),
        indices: n_index,
        patch_type: prim.mode().into_noodles()?,
        material: mat,
    })
}

/// Recursively convert each GLTF node.
///
/// Takes the NOODLES state to add entities, corresponding GLTF node, an optional NOODLES parent to use, a list of meshes to refer to, and a mapping of GLTF node id to NOODLES entity reference (updated during this call)
fn recursive_convert_node(
    state: &mut ServerState,
    node: &gltf::Node,
    parent: Option<EntityReference>,
    n_meshes: &[GeometryReference],
    n_nodes: &mut HashMap<usize, EntityReference>,
) -> EntityReference {
    // If the node already exists, return it
    if let Some(e) = n_nodes.get(&node.index()) {
        return e.clone();
    }

    // does not exist, build

    let tf = {
        // there's got to be a better way
        // but we need to take a nested 4x4 array to a 16x1 array. There's a nightly call, but we don't want to require it.
        let tf = node.transform().matrix();
        let mut ret = [0.0; 16];
        let mut count: usize = 0;

        for i in tf {
            ret[count] = i[0];
            count += 1;
            ret[count] = i[1];
            count += 1;
            ret[count] = i[2];
            count += 1;
            ret[count] = i[3];
            count += 1;
        }

        ret
    };

    // Determine the representation
    let rep: Option<ServerEntityRepresentation> = node.mesh().map(|f| {
        let mesh = n_meshes[f.index()].clone();
        ServerEntityRepresentation::new_render(RenderRepresentation {
            mesh,
            instances: None,
        })
    });

    // Create a new entity for this node
    let new_ent = state.entities.new_component(ServerEntityState {
        name: node.name().map(|f| f.to_string()),
        mutable: ServerEntityStateUpdatable {
            parent,
            transform: Some(tf),
            representation: rep,
            ..Default::default()
        },
    });

    // Update the node mapping
    n_nodes.insert(node.index(), new_ent.clone());

    // Build all children
    for child in node.children() {
        recursive_convert_node(state, &child, Some(new_ent.clone()), n_meshes, n_nodes);
    }

    new_ent
}

/// Import a GLTF file
pub fn import_file(
    path: &Path,
    state: ServerStatePtr,
    asset_store: AssetStorePtr,
) -> Result<ObjectRoot> {
    let mut lock = state.lock().unwrap();

    let mut published = Vec::<uuid::Uuid>::new();

    // Import and fetch whatever buffers we can. Note that this will NOT fetch
    // remote data hosted on external URIs. We will pass those along.
    let (gltf, buffers, _images) = gltf::import(path)?;

    log::debug!("Starting NOODLES conversion:");
    let n_buffers: Vec<_> = buffers
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let id = create_asset_id();

            published.push(id);

            let res = add_asset(
                asset_store.clone(),
                id,
                Asset::new_from_slice(f.0.as_slice()),
            );

            log::debug!("Adding {i}");

            lock.buffers
                .new_component(BufferState::new_from_url(&res, f.len() as u64))
        })
        .collect();

    log::debug!("Added {} buffers", n_buffers.len());

    let n_buffer_views: Vec<_> = gltf
        .views()
        .map(|f| {
            let buffer = n_buffers[f.buffer().index()].clone();

            let src_size = lock
                .buffers
                .inspect(buffer.id(), |t| t.size)
                .expect("Missing buffer?");

            let fixed_size = src_size - (f.offset() as u64);

            lock.buffer_views.new_component(ServerBufferViewState {
                name: None,
                source_buffer: n_buffers[f.buffer().index()].clone(),
                view_type: BufferViewType::Geometry,
                offset: f.offset() as u64,
                length: fixed_size,
            })
        })
        .collect();

    log::debug!("Added {} buffer views", n_buffer_views.len());

    let n_images: Vec<_> = gltf
        .images()
        .enumerate()
        .map(|(_i, img)| {
            let new_state = ServerImageState {
                name: img.name().map(|f| f.to_string()),
                source: match img.source() {
                    gltf::image::Source::View { view, .. } => {
                        ImageSource::new_buffer(n_buffer_views[view.index()].clone())
                    }
                    gltf::image::Source::Uri { uri, .. } => {
                        ImageSource::new_uri(uri.parse().unwrap())
                    }
                },
            };

            lock.images.new_component(new_state)
        })
        .collect();

    log::debug!("Added {} images", n_images.len());

    let n_samplers: Vec<_> = gltf
        .samplers()
        .map(|f| {
            lock.samplers.new_component(SamplerState {
                name: f.name().map(|f| f.to_string()),
                mag_filter: f.mag_filter().map(|f| f.into_noodles()),
                min_filter: f.min_filter().map(|f| f.into_noodles()),
                wrap_s: Some(f.wrap_s().into_noodles()),
                wrap_t: Some(f.wrap_t().into_noodles()),
            })
        })
        .collect();

    log::debug!("Added {} samplers", n_samplers.len());

    let n_texture: Vec<_> = gltf
        .textures()
        .map(|f| {
            log::debug!("Adding texture: {:?}", f.index());
            lock.textures.new_component(ServerTextureState {
                name: f.name().map(|f| f.to_string()),
                image: n_images[f.source().index()].clone(),
                sampler: f
                    .sampler()
                    .index()
                    .and_then(|id| n_samplers.get(id).cloned()),
            })
        })
        .collect();

    log::debug!("Added {} textures", n_texture.len());

    let n_material: Vec<_> = gltf
        .materials()
        .map(|f| {
            lock.materials.new_component(ServerMaterialState {
                name: f.name().map(|f| f.to_string()),
                mutable: ServerMaterialStateUpdatable {
                    pbr_info: Some(PBRInfo {
                        base_color: f.pbr_metallic_roughness().base_color_factor(),
                        base_color_texture: f
                            .pbr_metallic_roughness()
                            .base_color_texture()
                            .map(|tex| fetch_texture_by_info(&n_texture, &tex)),
                        metallic: Some(f.pbr_metallic_roughness().metallic_factor()),
                        roughness: Some(f.pbr_metallic_roughness().roughness_factor()),
                        metal_rough_texture: f
                            .pbr_metallic_roughness()
                            .metallic_roughness_texture()
                            .map(|tex| fetch_texture_by_info(&n_texture, &tex)),
                    }),
                    normal_texture: f
                        .normal_texture()
                        .map(|tex| fetch_normal_texture(&n_texture, &tex)),
                    occlusion_texture: f
                        .occlusion_texture()
                        .map(|tex| fetch_occ_texture(&n_texture, &tex)),
                    emissive_texture: f
                        .emissive_texture()
                        .map(|tex| fetch_texture_by_info(&n_texture, &tex)),
                    emissive_factor: Some(f.emissive_factor()),
                    use_alpha: match f.alpha_mode() {
                        gltf::material::AlphaMode::Opaque => None,
                        gltf::material::AlphaMode::Mask => Some(true),
                        gltf::material::AlphaMode::Blend => Some(true),
                    },
                    alpha_cutoff: match (f.alpha_cutoff(), f.alpha_mode()) {
                        (None, _) => None,
                        (Some(_), gltf::material::AlphaMode::Opaque) => None,
                        (Some(x), gltf::material::AlphaMode::Mask) => Some(x),
                        (Some(_), gltf::material::AlphaMode::Blend) => None,
                    },
                    double_sided: Some(f.double_sided()),
                    ..Default::default()
                },
            })
        })
        .collect();

    log::debug!("Added {} materials", n_material.len());

    let mut n_default_mat: Option<MaterialReference> = None;

    let n_geoms: Vec<_> = gltf
        .meshes()
        .map(|f| {
            let new_c = ServerGeometryState {
                name: f.name().map(|f| f.to_string()),
                patches: f
                    .primitives()
                    .filter_map(|f| {
                        let mat = f
                            .material()
                            .index()
                            .map(|f| n_material[f].clone())
                            .unwrap_or_else(|| {
                                if n_default_mat.is_none() {
                                    n_default_mat = Some(make_default_material(&mut lock))
                                }
                                n_default_mat.clone().unwrap()
                            });

                        convert_geometry_patch(&n_buffer_views, &f, mat)
                    })
                    .collect(),
            };

            lock.geometries.new_component(new_c)
        })
        .collect();

    log::debug!("Added {} meshes", n_geoms.len());

    let mut n_nodes = HashMap::<usize, EntityReference>::new();

    for node in gltf.nodes() {
        recursive_convert_node(&mut lock, &node, None, &n_geoms, &mut n_nodes);
    }

    log::debug!("Added {} nodes", n_nodes.len());

    let root = Object {
        parts: gltf
            .nodes()
            .enumerate()
            .map(|(i, _n)| n_nodes.get(&i).unwrap().clone())
            .collect(),
        children: vec![],
    };

    Ok(ObjectRoot::new(root, published, asset_store))
}
