//! Material and texture processing for FBX files.

use crate::error::FbxError;
use crate::label::FbxAssetLabel;
use crate::loader::FbxLoaderSettings;
use crate::types::FbxMaterial;
use crate::utils::{convert_texture_uv_transform, props_to_extras};
use bevy::asset::{Handle, LoadContext, RenderAssetUsages};
use bevy::image::{
    CompressedImageFormats, ImageAddressMode, ImageLoaderSettings, ImageSampler,
    ImageSamplerDescriptor, ImageType,
};
use bevy::material::AlphaMode;
use bevy::pbr::StandardMaterial;
use bevy::prelude::*;
use std::collections::HashMap;

/// Result of [`process_materials`].
///
/// `standard_materials` / `named_standard_materials` are parallel convenience
/// handles for scene spawning (not stored on [`crate::Fbx`]).
/// `inverted_materials` is parallel to `materials`: cull-inverted
/// [`StandardMaterial`] copies for negative-scale nodes.
pub struct ProcessedMaterials {
    pub materials: Vec<Handle<FbxMaterial>>,
    pub named_materials: HashMap<Box<str>, Handle<FbxMaterial>>,
    pub standard_materials: Vec<Handle<StandardMaterial>>,
    pub named_standard_materials: HashMap<Box<str>, Handle<StandardMaterial>>,
    pub inverted_materials: Vec<Handle<StandardMaterial>>,
}

/// Process all materials from the FBX scene.
pub fn process_materials(
    scene: &ufbx::Scene,
    settings: &FbxLoaderSettings,
    load_context: &mut LoadContext,
) -> Result<ProcessedMaterials, FbxError> {
    let mut materials = Vec::new();
    let mut named_materials = HashMap::new();
    let mut standard_materials = Vec::new();
    let mut named_standard_materials = HashMap::new();
    let mut inverted_materials = Vec::new();
    let texture_handles = process_textures(scene, settings, load_context)?;

    // Dense index over kept materials (skip element_id == 0) so Material{N}
    // matches Fbx.materials order (0..n-1).
    for (index, ufbx_material) in scene
        .materials
        .as_ref()
        .iter()
        .filter(|m| m.element.element_id != 0)
        .enumerate()
    {
        let standard_material =
            create_standard_material(ufbx_material, &texture_handles, settings.load_materials)?;
        let inverted = invert_cull_material(&standard_material);

        // Labeled StandardMaterial; FbxMaterial.material points here.
        let standard_handle = load_context.add_labeled_asset(
            FbxAssetLabel::MaterialStandard(index).to_string(),
            standard_material,
        );
        let inverted_handle = load_context
            .add_labeled_asset(FbxAssetLabel::MaterialInverted(index).to_string(), inverted);

        let name = if ufbx_material.element.name.is_empty() {
            format!("FbxMaterial{index}")
        } else {
            ufbx_material.element.name.to_string()
        };
        let extras = props_to_extras(&ufbx_material.element.props);
        let fbx_material = FbxMaterial {
            index,
            name: name.clone(),
            material: standard_handle.clone(),
            extras,
        };
        let fbx_handle = load_context
            .add_labeled_asset(FbxAssetLabel::Material(index).to_string(), fbx_material);

        if !ufbx_material.element.name.is_empty() {
            let key: Box<str> = Box::from(ufbx_material.element.name.as_ref());
            named_materials.insert(key.clone(), fbx_handle.clone());
            named_standard_materials.insert(key, standard_handle.clone());
        }

        materials.push(fbx_handle);
        standard_materials.push(standard_handle);
        inverted_materials.push(inverted_handle);
    }

    Ok(ProcessedMaterials {
        materials,
        named_materials,
        standard_materials,
        named_standard_materials,
        inverted_materials,
    })
}

fn invert_cull_material(material: &StandardMaterial) -> StandardMaterial {
    let mut inverted = material.clone();
    inverted.cull_mode = match material.cull_mode {
        Some(bevy::render::render_resource::Face::Back) => {
            Some(bevy::render::render_resource::Face::Front)
        }
        Some(bevy::render::render_resource::Face::Front) => {
            Some(bevy::render::render_resource::Face::Back)
        }
        other => other,
    };
    inverted
}

fn wrap_to_address_mode(mode: ufbx::WrapMode) -> ImageAddressMode {
    match mode {
        ufbx::WrapMode::Clamp => ImageAddressMode::ClampToEdge,
        ufbx::WrapMode::Repeat => ImageAddressMode::Repeat,
    }
}

/// Build sampler: `override_sampler` wins; else FBX wrap on top of `default_sampler`
/// filter/mipmap settings.
fn texture_sampler(
    texture: &ufbx::Texture,
    settings: &FbxLoaderSettings,
) -> ImageSamplerDescriptor {
    if let Some(override_sampler) = &settings.override_sampler {
        return override_sampler.clone();
    }
    let mut sampler = settings.default_sampler.clone();
    sampler.address_mode_u = wrap_to_address_mode(texture.wrap_u);
    sampler.address_mode_v = wrap_to_address_mode(texture.wrap_v);
    sampler
}

/// Process textures: embedded content first, then external / `.fbm` paths.
///
/// Sampler mapping: ufbx 0.9 [`ufbx::Texture`] exposes `wrap_u` / `wrap_v` only
/// (no mag/min/mip filter fields), so only address modes come from FBX; filter /
/// mipmap come from [`FbxLoaderSettings::default_sampler`].
pub fn process_textures(
    scene: &ufbx::Scene,
    settings: &FbxLoaderSettings,
    load_context: &mut LoadContext,
) -> Result<HashMap<u32, Handle<Image>>, FbxError> {
    let mut texture_handles = HashMap::new();

    for texture in scene.textures.as_ref().iter() {
        let sampler_descriptor = texture_sampler(texture, settings);
        let sampler = ImageSampler::Descriptor(sampler_descriptor.clone());

        // Embedded binary content.
        if !texture.content.is_empty() {
            let ext = extension_hint(texture);
            let image_type = ImageType::Extension(ext);
            match Image::from_buffer(
                texture.content.as_ref(),
                image_type,
                CompressedImageFormats::NONE,
                true,
                sampler.clone(),
                settings.load_materials,
            ) {
                Ok(image) => {
                    let handle = load_context.add_labeled_asset(
                        FbxAssetLabel::Texture(texture.element.element_id as usize).to_string(),
                        image,
                    );
                    texture_handles.insert(texture.element.element_id, handle);
                    continue;
                }
                Err(e) => {
                    warn!(
                        "Failed to decode embedded texture '{}': {e}",
                        texture.filename
                    );
                }
            }
        }

        let relative_path = resolve_texture_relative_path(texture);
        if relative_path.is_empty() {
            continue;
        }

        let asset_path = load_context.path();
        let fbx_dir = asset_path
            .path()
            .parent()
            .unwrap_or_else(|| std::path::Path::new(""));
        let texture_path = fbx_dir.join(relative_path).to_string_lossy().to_string();

        // Mirror bevy_gltf: apply sampler via ImageLoaderSettings on nested loads.
        let asset_usage = settings.load_materials;
        let image_handle: Handle<Image> = load_context
            .load_builder()
            .with_settings(move |image_settings: &mut ImageLoaderSettings| {
                image_settings.sampler = ImageSampler::Descriptor(sampler_descriptor.clone());
                image_settings.asset_usage = asset_usage;
            })
            .load(texture_path);
        texture_handles.insert(texture.element.element_id, image_handle);
    }

    Ok(texture_handles)
}

fn extension_hint(texture: &ufbx::Texture) -> &str {
    let name = if !texture.filename.is_empty() {
        texture.filename.as_ref()
    } else {
        texture.absolute_filename.as_ref()
    };
    std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
}

fn resolve_texture_relative_path(texture: &ufbx::Texture) -> &str {
    let filename = if !texture.filename.is_empty() {
        texture.filename.as_ref()
    } else if !texture.absolute_filename.is_empty() {
        texture.absolute_filename.as_ref()
    } else {
        return "";
    };

    let is_absolute =
        filename.starts_with('/') || (filename.len() >= 3 && filename.chars().nth(1) == Some(':'));

    if !is_absolute {
        return filename;
    }

    if let Some(fbm_pos) = filename.rfind(".fbm/").or_else(|| filename.rfind(".fbm\\")) {
        let before_fbm = &filename[..fbm_pos];
        let folder_start = before_fbm.rfind(['/', '\\']).map(|p| p + 1).unwrap_or(0);
        &filename[folder_start..]
    } else {
        let last_slash = filename.rfind(['/', '\\']);
        if let Some(pos) = last_slash {
            &filename[pos + 1..]
        } else {
            filename
        }
    }
}

/// Resolve scalar opacity when ufbx marked opacity/transparency as present.
///
/// Prefers `pbr.opacity`; falls back to `1 - fbx.transparency_factor` (Godot-like).
fn resolve_opacity_alpha(ufbx_material: &ufbx::Material) -> Option<f32> {
    if ufbx_material.pbr.opacity.has_value {
        return Some(ufbx_material.pbr.opacity.value_vec4.x as f32);
    }
    if ufbx_material.fbx.transparency_factor.has_value {
        return Some(1.0 - ufbx_material.fbx.transparency_factor.value_vec4.x as f32);
    }
    None
}

/// Create a StandardMaterial from ufbx material.
pub fn create_standard_material(
    ufbx_material: &ufbx::Material,
    texture_handles: &HashMap<u32, Handle<Image>>,
    _usages: RenderAssetUsages,
) -> Result<StandardMaterial, FbxError> {
    let mut material = StandardMaterial::default();

    // Only apply maps that ufbx marked as present. Unset PBR scalars are often
    // 0.0 — writing them over Bevy defaults (e.g. roughness 0.5) makes Lambert
    // materials into black mirrors under dark HDR environments.
    if ufbx_material.fbx.diffuse_color.has_value {
        let diffuse = ufbx_material.fbx.diffuse_color.value_vec4;
        material.base_color = Color::srgb(diffuse.x as f32, diffuse.y as f32, diffuse.z as f32);
    } else if ufbx_material.pbr.base_color.has_value {
        let pbr_base = ufbx_material.pbr.base_color.value_vec4;
        material.base_color = Color::srgb(pbr_base.x as f32, pbr_base.y as f32, pbr_base.z as f32);
    }

    if ufbx_material.pbr.metalness.has_value {
        material.metallic = ufbx_material.pbr.metalness.value_vec4.x as f32;
    }
    if ufbx_material.pbr.roughness.has_value {
        material.perceptual_roughness = ufbx_material.pbr.roughness.value_vec4.x as f32;
    }

    if ufbx_material.fbx.emission_color.has_value {
        let emission = ufbx_material.fbx.emission_color.value_vec4;
        material.emissive =
            LinearRgba::rgb(emission.x as f32, emission.y as f32, emission.z as f32);
    }

    // Clearcoat / transmission / IOR when present on the ufbx PBR maps.
    if ufbx_material.features.coat.enabled {
        material.clearcoat = ufbx_material.pbr.coat_factor.value_vec4.x as f32;
        material.clearcoat_perceptual_roughness =
            ufbx_material.pbr.coat_roughness.value_vec4.x as f32;
    }
    if ufbx_material.features.transmission.enabled {
        material.specular_transmission = ufbx_material.pbr.transmission_factor.value_vec4.x as f32;
    }
    if ufbx_material.features.ior.enabled {
        let ior = ufbx_material.pbr.specular_ior.value_vec4.x as f32;
        if ior > 0.0 {
            material.ior = ior;
        }
    }
    if ufbx_material.features.double_sided.enabled {
        material.double_sided = true;
        material.cull_mode = None;
    }

    // Only force alpha when opacity/transparency was actually authored.
    if let Some(alpha) = resolve_opacity_alpha(ufbx_material) {
        // Godot-like: composite opacity into base_color alpha.
        material.base_color = material.base_color.with_alpha(alpha);
        if alpha < 1.0 {
            // Opacity textures tend to be cutouts; scalar opacity alone blends.
            let has_opacity_tex = ufbx_material.textures.iter().any(|t| {
                matches!(
                    t.material_prop.as_ref(),
                    "TransparencyFactor" | "TransparentColor" | "Opacity"
                ) && texture_handles.contains_key(&t.texture.element.element_id)
            });
            material.alpha_mode = if has_opacity_tex {
                AlphaMode::Mask(0.5)
            } else if alpha < 0.98 {
                AlphaMode::Blend
            } else {
                AlphaMode::Opaque
            };
        }
    }

    for texture_ref in &ufbx_material.textures {
        if let Some(image_handle) = texture_handles.get(&texture_ref.texture.element.element_id) {
            match texture_ref.material_prop.as_ref() {
                "DiffuseColor" | "BaseColor" => {
                    material.base_color_texture = Some(image_handle.clone());
                    material.uv_transform = convert_texture_uv_transform(&texture_ref.texture);
                }
                "NormalMap" => material.normal_map_texture = Some(image_handle.clone()),
                "Metallic" | "Roughness" | "MetallicRoughness" => {
                    material.metallic_roughness_texture = Some(image_handle.clone());
                }
                "EmissiveColor" => material.emissive_texture = Some(image_handle.clone()),
                "AmbientOcclusion" => material.occlusion_texture = Some(image_handle.clone()),
                _ => {}
            }
        }
    }

    Ok(material)
}
