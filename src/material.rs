//! Material and texture processing for FBX files.
//!
//! # Texture color space (parity with `bevy_gltf`)
//!
//! Color slots (base color, emissive) are decoded as sRGB; data slots (normal,
//! metallic/roughness, occlusion) are decoded linear, mirroring `bevy_gltf`'s
//! `is_srgb = !linear_textures.contains(..)`. A texture that feeds both kinds of
//! slots is decoded once per color space and published under two labels:
//! `Texture{N}` for the sRGB variant and `Texture{N}/Linear` for the linear one
//! (`N` is the dense texture index, like every other `FbxAssetLabel`).
//!
//! # Load-time packing
//!
//! Bevy's `StandardMaterial` samples albedo and opacity from one texture, and
//! metallic/roughness from one texture's G/B channels. FBX authors those as
//! separate maps, so the loader packs them while the pixels are still in
//! memory (embedded content only):
//!
//! - **Opacity**: an FBX opacity/transparency map is a *luminance mask*, not an
//!   alpha channel (the ufbx corpus checkerboards carry `alpha = 255`
//!   everywhere). The mask is written into the alpha channel of a new base
//!   color image (`Material{i}/BaseColorOpacity`), so `AlphaMode::Mask` works
//!   without turning the albedo into the mask's grayscale.
//! - **Metallic/roughness**: separate maps are packed into the glTF layout Bevy
//!   samples (`Material{i}/MetallicRoughness`, G = roughness, B = metallic).
//!
//! External images are loaded through the asset server (`LoadContext` has no
//! synchronous file API in Bevy 0.19) and cannot be read at load time; those
//! maps warn and are skipped rather than being bound into the wrong channel.

use crate::error::FbxError;
use crate::label::FbxAssetLabel;
use crate::loader::FbxLoaderSettings;
use crate::types::FbxMaterial;
use crate::utils::props_to_extras;
use bevy::asset::{Handle, LoadContext, RenderAssetUsages};
use bevy::material::AlphaMode;
use bevy::math::Affine2;
use bevy::mesh::UvChannel;
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
///
/// Synchronous compatibility entry point: embedded images are decoded, external
/// references stay asset-server handles (no channel packing for them). The
/// loader uses [`process_materials_with_external_bytes`], which reads external
/// files so opacity and metallic/roughness packing works for those as well.
pub fn process_materials(
    scene: &ufbx::Scene,
    settings: &FbxLoaderSettings,
    load_context: &mut LoadContext,
) -> Result<ProcessedMaterials, FbxError> {
    let textures = process_textures_internal(scene, settings, load_context)?;
    build_materials(scene, &textures, load_context)
}

/// Process all materials with external image bytes that the caller read first.
///
/// The loader reads the files (see the texture pass'
/// `external_texture_requests`) between two synchronous parse scopes, because
/// holding the ufbx scene across an await would make the asset loader future
/// non-`Send`.
pub fn process_materials_with_external_bytes(
    scene: &ufbx::Scene,
    settings: &FbxLoaderSettings,
    load_context: &mut LoadContext,
    external_bytes: &HashMap<u32, Vec<u8>>,
) -> Result<ProcessedMaterials, FbxError> {
    let textures =
        process_textures_with_external_bytes(scene, settings, load_context, external_bytes)?;
    build_materials(scene, &textures, load_context)
}

fn build_materials(
    scene: &ufbx::Scene,
    textures: &ProcessedTextures,
    load_context: &mut LoadContext,
) -> Result<ProcessedMaterials, FbxError> {
    let mut materials = Vec::new();
    let mut named_materials = HashMap::new();
    let mut standard_materials = Vec::new();
    let mut named_standard_materials = HashMap::new();
    let mut inverted_materials = Vec::new();

    // Dense index over kept materials (skip element_id == 0) so Material{N}
    // matches Fbx.materials order (0..n-1).
    for (index, ufbx_material) in scene
        .materials
        .as_ref()
        .iter()
        .filter(|m| m.element.element_id != 0)
        .enumerate()
    {
        let plan = textures
            .material_plans
            .get(&index)
            .cloned()
            .unwrap_or_default();
        let ordering = textures.uv_orderings.get(&index);
        let slots =
            resolve_material_slots(index, ufbx_material, &textures.handles, &plan, ordering);
        let standard_material = build_standard_material(ufbx_material, &slots);
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

mod pack;
pub(crate) mod texture;

pub use self::texture::process_textures;
use self::texture::{
    ColorSpace, MaterialTexturePlan, MaterialUvOrdering, ProcessedTextures, TextureSlot,
    TextureVariants, numbered_uv_set_ordinal, process_textures_internal,
    process_textures_with_external_bytes, slot_bindings, unit_uv_transform,
};

/// Resolve scalar opacity only when ufbx marked `pbr.opacity` as present.
///
/// Do **not** fall back to raw `fbx.transparency_factor`: Maya Lambert often
/// exports `TransparencyFactor=1` with black `TransparentColor`, which is
/// opaque. Effective opacity is `factor * color`; ufbx already folds that into
/// `pbr.opacity` and only sets `has_value` when transparency is real. Using
/// `1 - transparency_factor` alone made opaque cubes fully invisible (alpha 0).
fn resolve_opacity_alpha(ufbx_material: &ufbx::Material) -> Option<f32> {
    if ufbx_material.pbr.opacity.has_value {
        Some(ufbx_material.pbr.opacity.value_vec4.x as f32)
    } else {
        None
    }
}

/// Bevy channel for a UV set ordinal, warning when Bevy cannot represent it.
///
/// `conflicted` reports that the material's users order the same UV set name
/// differently; one channel per texture slot cannot serve both, so the first
/// user's ordinal is used.
fn uv_channel_for(ordinal: u32, conflicted: bool, uv_set: &str, material_name: &str) -> UvChannel {
    if conflicted {
        warn!(
            "Material '{material_name}' samples UV set '{uv_set}' through meshes that order their UV sets \
             differently; a texture slot binds one channel, so UV{ordinal} (the first user's ordering) is used."
        );
    }
    match ordinal {
        0 => UvChannel::Uv0,
        1 => UvChannel::Uv1,
        other => {
            warn!(
                "Material '{material_name}' samples UV set '{uv_set}' (channel {other}), but Bevy supports two \
                 UV channels (UV0/UV1); falling back to UV0."
            );
            UvChannel::Uv0
        }
    }
}

/// Resolve one texture reference's channel: the ordinal of its UV set name in
/// the meshes that use the material, falling back to numbered DCC conventions.
fn binding_uv_channel(
    ordering: Option<&MaterialUvOrdering>,
    texture: &ufbx::Texture,
    material_name: &str,
) -> UvChannel {
    let uv_set = texture.uv_set.as_ref();
    let (ordinal, conflicted) = match ordering {
        Some(ordering) => ordering.channel_for(uv_set),
        None => (numbered_uv_set_ordinal(uv_set), false),
    };
    uv_channel_for(ordinal, conflicted, uv_set, material_name)
}

/// Handles and channel/transform choices for one material's texture slots.
#[derive(Default)]
struct MaterialSlots {
    base_color: Option<Handle<Image>>,
    base_color_channel: UvChannel,
    base_color_uv_transform: Option<Affine2>,
    /// The base color handle's alpha already carries the opacity mask.
    base_color_alpha_packed: bool,
    emissive: Option<(Handle<Image>, UvChannel)>,
    normal_map: Option<(Handle<Image>, UvChannel)>,
    metallic_roughness: Option<(Handle<Image>, UvChannel)>,
    occlusion: Option<(Handle<Image>, UvChannel)>,
    /// An opacity texture is bound to the material (alpha_mode heuristic).
    opacity_texture_present: bool,
}

/// Pick the handles a material binds, honoring load-time packing results.
fn resolve_material_slots(
    material_index: usize,
    material: &ufbx::Material,
    textures: &HashMap<u32, TextureVariants>,
    plan: &MaterialTexturePlan,
    ordering: Option<&MaterialUvOrdering>,
) -> MaterialSlots {
    let mut slots = MaterialSlots::default();
    let bindings = slot_bindings(material);
    let material_name = if material.element.name.is_empty() {
        format!("Material{material_index}")
    } else {
        material.element.name.to_string()
    };
    let mut base_channel = None;
    let mut base_transform = None;
    let mut opacity_channel = None;
    let mut opacity_transform = None;

    for binding in &bindings {
        let channel = binding_uv_channel(ordering, binding.texture, &material_name);
        let handle = textures
            .get(&binding.id())
            .and_then(|variants| variants.get(binding.slot.color_space()))
            .cloned();
        match binding.slot {
            TextureSlot::BaseColor => {
                slots.base_color = plan.base_color.clone().or(handle);
                base_channel = Some(channel);
                base_transform = Some(unit_uv_transform(binding.texture));
            }
            TextureSlot::Emissive => slots.emissive = handle.map(|handle| (handle, channel)),
            TextureSlot::NormalMap => slots.normal_map = handle.map(|handle| (handle, channel)),
            TextureSlot::Occlusion => slots.occlusion = handle.map(|handle| (handle, channel)),
            TextureSlot::Opacity => {
                slots.opacity_texture_present = true;
                opacity_channel = Some(channel);
                opacity_transform = Some(unit_uv_transform(binding.texture));
            }
            TextureSlot::Metalness | TextureSlot::Roughness | TextureSlot::MetallicRoughness => {}
        }
    }

    // The packed base color image also covers opacity-only materials, which have
    // no base color binding at all; the mask's own UV slot and transform then
    // describe how the packed image is sampled. Packing already multiplied the
    // scalar opacity into the alpha, so it must not be applied a second time.
    // When nothing was packed the base binding (or its absence) alone describes
    // the slot, so an opacity transform must not leak into it.
    let packed_base = plan.base_color.is_some();
    if let Some(packed) = &plan.base_color {
        slots.base_color = Some(packed.clone());
        slots.base_color_alpha_packed = true;
    }
    slots.base_color_channel = if packed_base {
        base_channel.or(opacity_channel)
    } else {
        base_channel
    }
    .unwrap_or(UvChannel::Uv0);
    slots.base_color_uv_transform = if packed_base {
        base_transform.or(opacity_transform)
    } else {
        base_transform
    };

    // Metallic/roughness selection follows the packing plan.
    if let Some(binding) = bindings
        .iter()
        .find(|b| b.slot == TextureSlot::MetallicRoughness)
    {
        slots.metallic_roughness = textures
            .get(&binding.id())
            .and_then(|variants| variants.get(ColorSpace::Linear))
            .cloned()
            .map(|handle| {
                (
                    handle,
                    binding_uv_channel(ordering, binding.texture, &material_name),
                )
            });
    } else if let Some(packed) = &plan.metallic_roughness {
        let channel = bindings
            .iter()
            .find(|b| matches!(b.slot, TextureSlot::Metalness | TextureSlot::Roughness))
            .map(|b| binding_uv_channel(ordering, b.texture, &material_name))
            .unwrap_or(UvChannel::Uv0);
        slots.metallic_roughness = Some((packed.clone(), channel));
    } else if plan.direct_metallic_roughness
        && let Some(binding) = bindings
            .iter()
            .find(|b| matches!(b.slot, TextureSlot::Metalness | TextureSlot::Roughness))
    {
        slots.metallic_roughness = textures
            .get(&binding.id())
            .and_then(|variants| variants.get(ColorSpace::Linear))
            .cloned()
            .map(|handle| {
                (
                    handle,
                    binding_uv_channel(ordering, binding.texture, &material_name),
                )
            });
    }
    // Maps the plan left unset stay unbound on purpose; the load-time warnings
    // explain why (unreadable external file, or UV sampling that one packed
    // image cannot represent).

    slots
}

/// Create a StandardMaterial from ufbx material.
///
/// Flat compatibility entry point: one handle per FBX texture is used for every
/// slot (the loader uses the per-slot variant path in `resolve_material_slots`).
pub fn create_standard_material(
    ufbx_material: &ufbx::Material,
    texture_handles: &HashMap<u32, Handle<Image>>,
    _usages: RenderAssetUsages,
) -> Result<StandardMaterial, FbxError> {
    let mutations = texture_handles
        .iter()
        .map(|(id, handle)| (*id, TextureVariants::uniform(handle.clone())))
        .collect();
    let plan = MaterialTexturePlan {
        direct_metallic_roughness: true,
        ..Default::default()
    };
    let slots = resolve_material_slots(0, ufbx_material, &mutations, &plan, None);
    Ok(build_standard_material(ufbx_material, &slots))
}

fn build_standard_material(
    material_source: &ufbx::Material,
    slots: &MaterialSlots,
) -> StandardMaterial {
    let mut material = StandardMaterial::default();

    // Only apply maps that ufbx marked as present. Unset PBR scalars are often
    // 0.0 — writing them over Bevy defaults (e.g. roughness 0.5) makes Lambert
    // materials into black mirrors under dark HDR environments.
    if material_source.fbx.diffuse_color.has_value {
        let diffuse = material_source.fbx.diffuse_color.value_vec4;
        material.base_color = Color::srgb(diffuse.x as f32, diffuse.y as f32, diffuse.z as f32);
    } else if material_source.pbr.base_color.has_value {
        let pbr_base = material_source.pbr.base_color.value_vec4;
        material.base_color = Color::srgb(pbr_base.x as f32, pbr_base.y as f32, pbr_base.z as f32);
    }

    if material_source.pbr.metalness.has_value {
        material.metallic = material_source.pbr.metalness.value_vec4.x as f32;
    }
    if material_source.pbr.roughness.has_value {
        material.perceptual_roughness = material_source.pbr.roughness.value_vec4.x as f32;
    }

    if material_source.fbx.emission_color.has_value {
        let emission = material_source.fbx.emission_color.value_vec4;
        material.emissive =
            LinearRgba::rgb(emission.x as f32, emission.y as f32, emission.z as f32);
    }

    // Clearcoat / transmission / IOR when present on the ufbx PBR maps.
    if material_source.features.coat.enabled {
        material.clearcoat = material_source.pbr.coat_factor.value_vec4.x as f32;
        material.clearcoat_perceptual_roughness =
            material_source.pbr.coat_roughness.value_vec4.x as f32;
    }
    if material_source.features.transmission.enabled {
        material.specular_transmission =
            material_source.pbr.transmission_factor.value_vec4.x as f32;
    }
    if material_source.features.ior.enabled {
        let ior = material_source.pbr.specular_ior.value_vec4.x as f32;
        if ior > 0.0 {
            material.ior = ior;
        }
    }
    if material_source.features.double_sided.enabled {
        material.double_sided = true;
        material.cull_mode = None;
    }

    // Texture slots, in the color space each one needs.
    if let Some(handle) = &slots.base_color {
        material.base_color_texture = Some(handle.clone());
    }
    material.base_color_channel = slots.base_color_channel.clone();
    if let Some(transform) = slots.base_color_uv_transform {
        material.uv_transform = transform;
    }
    if let Some((handle, channel)) = &slots.emissive {
        material.emissive_texture = Some(handle.clone());
        material.emissive_channel = channel.clone();
    }
    if let Some((handle, channel)) = &slots.normal_map {
        material.normal_map_texture = Some(handle.clone());
        material.normal_map_channel = channel.clone();
    }
    if let Some((handle, channel)) = &slots.metallic_roughness {
        material.metallic_roughness_texture = Some(handle.clone());
        material.metallic_roughness_channel = channel.clone();
    }
    if let Some((handle, channel)) = &slots.occlusion {
        material.occlusion_texture = Some(handle.clone());
        material.occlusion_channel = channel.clone();
    }

    // Only force alpha when opacity/transparency was actually authored.
    let scalar_alpha = resolve_opacity_alpha(material_source);
    if let Some(alpha) = scalar_alpha {
        if !slots.base_color_alpha_packed {
            // Godot-like: composite scalar opacity into base_color alpha.
            material.base_color = material.base_color.with_alpha(alpha);
        }
        if alpha < 1.0 || slots.opacity_texture_present {
            material.alpha_mode = alpha_mode_for(alpha, slots.opacity_texture_present);
        }
    } else if slots.opacity_texture_present {
        // Texture-only opacity: FBX transparency maps are usually hard cutouts.
        material.alpha_mode = AlphaMode::Mask(0.5);
    }

    material
}

/// Alpha mode heuristic: an authored opacity texture with full scalar opacity is
/// treated as a cutout (FBX transparency maps are usually binary). A scalar
/// opacity factor below one is bucketed like factor-only opacity, so `Blend`
/// wins over `Mask`: under `Mask` the scalar would scale the texture alpha and
/// fragments below the 0.5 cutoff would disappear entirely.
fn alpha_mode_for(alpha: f32, opacity_texture_present: bool) -> AlphaMode {
    if alpha < 0.98 {
        AlphaMode::Blend
    } else if opacity_texture_present {
        AlphaMode::Mask(0.5)
    } else {
        AlphaMode::Opaque
    }
}

#[cfg(test)]
mod tests {
    use super::texture::{
        MaterialUvOrdering, UvSampling, compensate_v_flip, normalize_relative_asset_path,
        numbered_uv_set_ordinal, resolve_reference_in_root, sniff_image_extension,
    };
    use super::*;
    use bevy::image::ImageAddressMode;

    #[test]
    fn alpha_mode_blends_when_the_scalar_scales_a_cutout() {
        assert!(matches!(
            alpha_mode_for(1.0, true),
            AlphaMode::Mask(cutoff) if cutoff == 0.5
        ));
        assert!(matches!(alpha_mode_for(0.4, true), AlphaMode::Blend));
        assert!(matches!(alpha_mode_for(0.4, false), AlphaMode::Blend));
        assert!(matches!(alpha_mode_for(1.0, false), AlphaMode::Opaque));
    }

    #[test]
    fn packing_sampling_identity_includes_wrap_modes() {
        let repeat = UvSampling {
            channel: 0,
            transform: Affine2::IDENTITY,
            address_u: ImageAddressMode::Repeat,
            address_v: ImageAddressMode::Repeat,
        };
        assert_ne!(
            repeat,
            UvSampling {
                address_u: ImageAddressMode::ClampToEdge,
                ..repeat
            },
            "differing wrap_u must split the sampling identity"
        );
        assert_ne!(
            repeat,
            UvSampling {
                address_v: ImageAddressMode::ClampToEdge,
                ..repeat
            },
            "differing wrap_v must split the sampling identity"
        );
    }

    #[test]
    fn v_flip_compensation_maps_flipped_uvs_to_the_authored_sample() {
        let transform =
            Affine2::from_scale_angle_translation(Vec2::new(2.0, 3.0), 0.4, Vec2::new(0.2, 0.7));
        let compensated = compensate_v_flip(transform);
        let flip =
            Affine2::from_scale_angle_translation(Vec2::new(1.0, -1.0), 0.0, Vec2::new(0.0, 1.0));
        for uv in [
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(0.25, 0.75),
            Vec2::new(0.5, 1.0),
            Vec2::new(0.9, 0.1),
        ] {
            let flipped = flip.transform_point2(uv);
            let expected = transform.transform_point2(uv);
            let actual = compensated.transform_point2(flipped);
            assert!(
                (actual - expected).length() < 1e-5,
                "uv {uv:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// Numbered DCC names are 1-based, so `UVChannel_1`/`map1` are the first set.
    #[test]
    fn numbered_uv_set_names_are_one_based() {
        assert_eq!(numbered_uv_set_ordinal("UVChannel_1"), 0);
        assert_eq!(numbered_uv_set_ordinal("UVChannel_2"), 1);
        assert_eq!(numbered_uv_set_ordinal("map1"), 0);
        assert_eq!(numbered_uv_set_ordinal("map3"), 2);
        assert_eq!(numbered_uv_set_ordinal("UVSet0"), 0);
        assert_eq!(numbered_uv_set_ordinal(""), 0);
        assert_eq!(numbered_uv_set_ordinal("UVSet"), 0);
    }

    /// The mesh's own `uv_sets` order wins over the name: a mesh may list
    /// `UVChannel_2` first, and a material using both must follow that order.
    #[test]
    fn uv_set_channels_follow_the_user_mesh_order() {
        let mut ordering = MaterialUvOrdering::default();
        ordering.insert("UVChannel_2", 0);
        ordering.insert("UVChannel_1", 1);
        assert_eq!(ordering.channel_for("UVChannel_2"), (0, false));
        assert_eq!(ordering.channel_for("UVChannel_1"), (1, false));
        // Unknown custom names fall back to the numbered convention.
        assert_eq!(ordering.channel_for("map2"), (1, false));
        assert_eq!(ordering.channel_for(""), (0, false));

        // Two users with different orders for the same name cannot both be
        // represented: the first user's ordinal wins and the conflict is visible.
        let mut conflicting = MaterialUvOrdering::default();
        conflicting.insert("UVChannel_1", 0);
        conflicting.insert("UVChannel_1", 1);
        assert_eq!(conflicting.channel_for("UVChannel_1"), (0, true));
    }

    /// Channel selection reports orders Bevy cannot represent.
    #[test]
    fn uv_channel_selection_warns_and_clamps_beyond_two_channels() {
        assert!(matches!(
            uv_channel_for(0, false, "UVChannel_1", "M"),
            UvChannel::Uv0
        ));
        assert!(matches!(
            uv_channel_for(1, true, "UVSet", "M"),
            UvChannel::Uv1
        ));
        assert!(matches!(
            uv_channel_for(2, false, "map3", "M"),
            UvChannel::Uv0
        ));
    }

    /// `..` is resolved against the FBX folder before clamping to the root.
    #[test]
    fn relative_parents_resolve_inside_the_root() {
        assert_eq!(
            resolve_reference_in_root("models", "../textures/x.png"),
            "textures/x.png"
        );
        assert_eq!(
            resolve_reference_in_root("a/b", "../../textures/x.png"),
            "textures/x.png"
        );
        assert_eq!(
            resolve_reference_in_root("", "textures/x.png"),
            "textures/x.png"
        );
        // Escaping the root is clamped, not followed.
        assert_eq!(
            resolve_reference_in_root("", "../../etc/passwd"),
            "etc/passwd"
        );
        assert_eq!(
            resolve_reference_in_root("sub", "../../../etc/passwd"),
            "etc/passwd"
        );
    }

    #[test]
    fn magic_bytes_win_over_the_filename_extension() {
        assert_eq!(sniff_image_extension(b"\x89PNG\r\n\x1a\n...."), Some("png"));
        assert_eq!(sniff_image_extension(b"\xff\xd8\xff\xe0"), Some("jpg"));
        assert_eq!(sniff_image_extension(b"DDS "), Some("dds"));
        assert_eq!(sniff_image_extension(b"RIFF____WEBPVP8 "), Some("webp"));
        assert_eq!(sniff_image_extension(b"not an image"), None);
    }

    #[test]
    fn relative_paths_are_normalized_and_stay_inside_the_root() {
        assert_eq!(
            normalize_relative_asset_path("textures\\sub\\tex.png"),
            "textures/sub/tex.png"
        );
        assert_eq!(
            normalize_relative_asset_path("D:\\art\\tex.png"),
            "art/tex.png"
        );
        assert_eq!(
            normalize_relative_asset_path("../../outside/tex.png"),
            "outside/tex.png"
        );
        assert_eq!(
            normalize_relative_asset_path("textures/../shared/x.png"),
            "shared/x.png"
        );
        assert_eq!(normalize_relative_asset_path(""), "");
    }
}
