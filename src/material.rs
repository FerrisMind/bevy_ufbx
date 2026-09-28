//! Material and texture processing for FBX files.

use crate::error::FbxError;
use crate::label::FbxAssetLabel;
use crate::loader::FbxLoaderSettings;
use crate::utils::convert_texture_uv_transform;
use bevy::asset::{Handle, LoadContext, RenderAssetUsages};
use bevy::image::{
    CompressedImageFormats, ImageAddressMode, ImageSampler, ImageSamplerDescriptor, ImageType,
};
use bevy::material::AlphaMode;
use bevy::pbr::StandardMaterial;
use bevy::prelude::*;
use std::collections::HashMap;

type MaterialHandles = (
    Vec<Handle<StandardMaterial>>,
    HashMap<Box<str>, Handle<StandardMaterial>>,
);

/// Process all materials from the FBX scene.
pub fn process_materials(
    scene: &ufbx::Scene,
    settings: &FbxLoaderSettings,
    load_context: &mut LoadContext,
) -> Result<MaterialHandles, FbxError> {
    let mut materials = Vec::new();
    let mut named_materials = HashMap::new();
    let texture_handles = process_textures(scene, settings, load_context)?;

    for (index, ufbx_material) in scene.materials.as_ref().iter().enumerate() {
        if ufbx_material.element.element_id == 0 {
            continue;
        }

        let standard_material =
            create_standard_material(ufbx_material, &texture_handles, settings.load_materials)?;
        let handle = load_context.add_labeled_asset(
            FbxAssetLabel::Material(index).to_string(),
            standard_material,
        );

        if !ufbx_material.element.name.is_empty() {
            named_materials.insert(
                Box::from(ufbx_material.element.name.as_ref()),
                handle.clone(),
            );
        }

        materials.push(handle);
    }

    Ok((materials, named_materials))
}

fn wrap_to_address_mode(mode: ufbx::WrapMode) -> ImageAddressMode {
    match mode {
        ufbx::WrapMode::Clamp => ImageAddressMode::ClampToEdge,
        ufbx::WrapMode::Repeat => ImageAddressMode::Repeat,
    }
}

/// Process textures: embedded content first, then external / `.fbm` paths.
pub fn process_textures(
    scene: &ufbx::Scene,
    settings: &FbxLoaderSettings,
    load_context: &mut LoadContext,
) -> Result<HashMap<u32, Handle<Image>>, FbxError> {
    let mut texture_handles = HashMap::new();

    for texture in scene.textures.as_ref().iter() {
        let sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
            address_mode_u: wrap_to_address_mode(texture.wrap_u),
            address_mode_v: wrap_to_address_mode(texture.wrap_v),
            ..ImageSamplerDescriptor::default()
        });

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
        let texture_path = fbx_dir
            .join(relative_path)
            .to_string_lossy()
            .to_string();

        let image_handle: Handle<Image> = load_context.load(texture_path);
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

    let is_absolute = filename.starts_with('/')
        || (filename.len() >= 3 && filename.chars().nth(1) == Some(':'));

    if !is_absolute {
        return filename;
    }

    if let Some(fbm_pos) = filename.rfind(".fbm/").or_else(|| filename.rfind(".fbm\\")) {
        let before_fbm = &filename[..fbm_pos];
        let folder_start = before_fbm
            .rfind(['/', '\\'])
            .map(|p| p + 1)
            .unwrap_or(0);
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

    if ufbx_material.pbr.opacity.value_vec4.x < 1.0 {
        let alpha = ufbx_material.pbr.opacity.value_vec4.x as f32;
        material.alpha_mode = if alpha < 0.98 {
            AlphaMode::Blend
        } else {
            AlphaMode::Opaque
        };
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
