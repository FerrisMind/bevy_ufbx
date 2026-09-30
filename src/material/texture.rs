//! Texture decoding, color-space variants and load-time material packing.
//!
//! Split out of `material.rs` to keep the material module focused on
//! `StandardMaterial` construction. The texture pass decodes embedded bytes,
//! decides the color space per slot and packs FBX's separate
//! opacity/metallic/roughness maps into Bevy's channel layouts while the pixels
//! are still in memory.

use super::pack::{compose_base_color_alpha, pack_metallic_roughness};
use super::resolve_opacity_alpha;
use crate::error::FbxError;
use crate::label::FbxAssetLabel;
use crate::loader::FbxLoaderSettings;
use crate::utils::convert_texture_uv_transform;
use bevy::asset::{Handle, LoadContext, RenderAssetUsages};
use bevy::image::{
    CompressedImageFormats, ImageAddressMode, ImageFormat, ImageLoaderSettings, ImageSampler,
    ImageSamplerDescriptor, ImageType,
};
use bevy::math::Affine2;
use bevy::prelude::*;
use std::collections::{HashMap, HashSet};

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

/// Which color space an image variant is decoded in.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(super) enum ColorSpace {
    /// sRGB ("color") data: base color, emissive, opacity masks.
    Srgb,
    /// Linear ("data") data: normal, metallic/roughness, occlusion.
    Linear,
}

impl ColorSpace {
    fn is_srgb(self) -> bool {
        matches!(self, ColorSpace::Srgb)
    }
}

/// `StandardMaterial` texture slots fed from FBX material maps.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum TextureSlot {
    BaseColor,
    Emissive,
    Opacity,
    NormalMap,
    Metalness,
    Roughness,
    MetallicRoughness,
    Occlusion,
    /// Clearcoat strength (`KHR_materials_clearcoat` `clearcoatTexture`).
    Clearcoat,
    /// Clearcoat roughness (`clearcoatRoughnessTexture`).
    ClearcoatRoughness,
    /// Clearcoat layer normal (`clearcoatNormalTexture`).
    ClearcoatNormal,
}

impl TextureSlot {
    /// Color space of the slot, mirroring `bevy_gltf`'s linear-texture set
    /// (all clearcoat maps are non-color data there too).
    pub(super) fn color_space(self) -> ColorSpace {
        match self {
            TextureSlot::BaseColor | TextureSlot::Emissive | TextureSlot::Opacity => {
                ColorSpace::Srgb
            }
            TextureSlot::NormalMap
            | TextureSlot::Metalness
            | TextureSlot::Roughness
            | TextureSlot::MetallicRoughness
            | TextureSlot::Occlusion
            | TextureSlot::Clearcoat
            | TextureSlot::ClearcoatRoughness
            | TextureSlot::ClearcoatNormal => ColorSpace::Linear,
        }
    }

    /// Compatibility mapping from raw FBX `material_prop` names, used when ufbx
    /// does not resolve a map for the material's shader.
    fn from_prop(prop: &str) -> Option<Self> {
        match prop {
            "DiffuseColor" | "BaseColor" => Some(TextureSlot::BaseColor),
            "EmissiveColor" => Some(TextureSlot::Emissive),
            "TransparencyFactor" | "TransparentColor" | "Opacity" => Some(TextureSlot::Opacity),
            "NormalMap" => Some(TextureSlot::NormalMap),
            "Metallic" => Some(TextureSlot::Metalness),
            "Roughness" => Some(TextureSlot::Roughness),
            "MetallicRoughness" => Some(TextureSlot::MetallicRoughness),
            "AmbientOcclusion" => Some(TextureSlot::Occlusion),
            _ => None,
        }
    }
}

/// One texture binding of a material slot.
#[derive(Clone, Copy)]
pub(super) struct SlotBinding<'a> {
    pub(super) slot: TextureSlot,
    pub(super) texture: &'a ufbx::Texture,
}

impl SlotBinding<'_> {
    pub(super) fn id(&self) -> u32 {
        self.texture.element.element_id
    }
}

fn push_binding<'a>(
    out: &mut Vec<SlotBinding<'a>>,
    slot: TextureSlot,
    texture: Option<&'a ufbx::Texture>,
) {
    let Some(texture) = texture else {
        return;
    };
    // First binding for a slot wins: one Bevy texture per slot.
    if out.iter().any(|binding| binding.slot == slot) {
        return;
    }
    out.push(SlotBinding { slot, texture });
}

/// Enabled texture of an ufbx-resolved PBR map.
fn map_texture(map: &ufbx::MaterialMap) -> Option<&ufbx::Texture> {
    if !map.texture_enabled {
        return None;
    }
    map.texture.as_deref()
}

/// `true` when ufbx considers the material's opacity authored.
///
/// ufbx folds Maya's `TransparencyFactor * TransparentColor` into `pbr.opacity`
/// and only enables the feature when transparency is real; a texture bound to an
/// opacity property enables it too (`UFBXI_SHADER_FEATURE_IF_TEXTURE`). Files
/// that merely *contain* `TransparencyFactor = 1` with a black
/// `TransparentColor` (opaque Maya Lambert) stay opaque.
fn opacity_authored(material: &ufbx::Material) -> bool {
    material.features.opacity.enabled
}

/// Resolve the texture bound to each material slot.
///
/// ufbx-resolved [`ufbx::MaterialMap::texture`] wins (ufbx understands the
/// Maya/3dsMax/Blender shader variants); the historical `material_prop` name
/// matching is the compatibility fallback for files where ufbx resolves no map.
pub(super) fn slot_bindings(material: &ufbx::Material) -> Vec<SlotBinding<'_>> {
    let mut out = Vec::new();
    let pbr = &material.pbr;
    push_binding(
        &mut out,
        TextureSlot::BaseColor,
        map_texture(&pbr.base_color),
    );
    push_binding(
        &mut out,
        TextureSlot::Emissive,
        map_texture(&pbr.emission_color),
    );
    if opacity_authored(material) {
        push_binding(&mut out, TextureSlot::Opacity, map_texture(&pbr.opacity));
    }
    push_binding(
        &mut out,
        TextureSlot::NormalMap,
        map_texture(&pbr.normal_map),
    );
    push_binding(
        &mut out,
        TextureSlot::Metalness,
        map_texture(&pbr.metalness),
    );
    push_binding(
        &mut out,
        TextureSlot::Roughness,
        map_texture(&pbr.roughness),
    );
    push_binding(
        &mut out,
        TextureSlot::Occlusion,
        map_texture(&pbr.ambient_occlusion),
    );

    // Clearcoat maps compose like `bevy_gltf`'s KHR_materials_clearcoat, where
    // factor and texture arrive with the same extension: ufbx's `features.coat`
    // gate keeps the textures consistent with the `apply_coat` scalars. Only
    // `coat_roughness` binds (not `coat_glossiness`): ufbx moves a
    // glossiness-authored map and its texture to `coat_glossiness`, and Bevy
    // has no gloss-inverting sampler — same precedent as the main roughness map.
    if material.features.coat.enabled {
        push_binding(
            &mut out,
            TextureSlot::Clearcoat,
            map_texture(&pbr.coat_factor),
        );
        push_binding(
            &mut out,
            TextureSlot::ClearcoatRoughness,
            map_texture(&pbr.coat_roughness),
        );
        push_binding(
            &mut out,
            TextureSlot::ClearcoatNormal,
            map_texture(&pbr.coat_normal),
        );
    }

    for texture_ref in material.textures.iter() {
        let Some(slot) = TextureSlot::from_prop(texture_ref.material_prop.as_ref()) else {
            continue;
        };
        if slot == TextureSlot::Opacity && !opacity_authored(material) {
            continue;
        }
        push_binding(&mut out, slot, Some(&texture_ref.texture));
    }
    out
}

/// Which color-space variants each FBX texture must be decoded in.
#[derive(Default)]
struct VariantPlan {
    srgb: HashSet<u32>,
    linear: HashSet<u32>,
}

impl VariantPlan {
    /// Whether any slot uses this texture.
    fn mentions(&self, id: u32) -> bool {
        self.srgb.contains(&id) || self.linear.contains(&id)
    }

    fn variants_for(&self, id: u32) -> Vec<ColorSpace> {
        let mut variants = Vec::new();
        if self.srgb.contains(&id) {
            variants.push(ColorSpace::Srgb);
        }
        if self.linear.contains(&id) {
            variants.push(ColorSpace::Linear);
        }
        if variants.is_empty() {
            // Unclassified textures keep the historical sRGB decode.
            variants.push(ColorSpace::Srgb);
        }
        variants
    }
}

fn texture_variant_plan(scene: &ufbx::Scene) -> VariantPlan {
    let mut plan = VariantPlan::default();
    for material in scene
        .materials
        .as_ref()
        .iter()
        .filter(|m| m.element.element_id != 0)
    {
        for binding in slot_bindings(material) {
            match binding.slot.color_space() {
                ColorSpace::Srgb => plan.srgb.insert(binding.id()),
                ColorSpace::Linear => plan.linear.insert(binding.id()),
            };
        }
    }
    plan
}

/// Label for one color-space variant of a texture.
///
/// `Texture{N}` is the sRGB variant and `Texture{N}/Linear` the linear one; a
/// texture used only by linear slots gets the `/Linear` label only.
fn texture_label(dense_index: usize, space: ColorSpace) -> String {
    match space {
        ColorSpace::Srgb => FbxAssetLabel::Texture(dense_index).to_string(),
        ColorSpace::Linear => format!("{}/Linear", FbxAssetLabel::Texture(dense_index)),
    }
}

/// Decoded (or externally referenced) image for one texture variant.
enum DecodedTexture {
    /// Decoded in-process; registered after material packing.
    Embedded(Image),
    /// Loaded through the asset server (external file).
    External(Handle<Image>),
}

/// Handles for one FBX texture, per color space.
#[derive(Default, Clone)]
pub(super) struct TextureVariants {
    srgb: Option<Handle<Image>>,
    linear: Option<Handle<Image>>,
}

impl TextureVariants {
    /// One handle used for both color spaces (flat compatibility path).
    pub(super) fn uniform(handle: Handle<Image>) -> Self {
        Self {
            srgb: Some(handle.clone()),
            linear: Some(handle),
        }
    }

    fn set(&mut self, space: ColorSpace, handle: Handle<Image>) {
        match space {
            ColorSpace::Srgb => self.srgb = Some(handle),
            ColorSpace::Linear => self.linear = Some(handle),
        }
    }

    pub(super) fn get(&self, space: ColorSpace) -> Option<&Handle<Image>> {
        match space {
            ColorSpace::Srgb => self.srgb.as_ref(),
            ColorSpace::Linear => self.linear.as_ref(),
        }
    }

    fn any(&self) -> Option<&Handle<Image>> {
        self.srgb.as_ref().or(self.linear.as_ref())
    }
}

/// What kind of image a pending packed asset is.
#[derive(Clone, Copy)]
enum PackedKind {
    BaseColorOpacity,
    MetallicRoughness,
}

/// A packed image waiting to be registered as a labeled asset.
struct PendingPackedImage {
    label: String,
    image: Image,
    material_index: usize,
    kind: PackedKind,
}

/// Load-time packing result for one material.
#[derive(Default, Clone)]
pub(super) struct MaterialTexturePlan {
    /// Base color image with the opacity mask packed into its alpha channel.
    pub(super) base_color: Option<Handle<Image>>,
    /// Separate metallic/roughness maps packed into Bevy's G/B layout.
    pub(super) metallic_roughness: Option<Handle<Image>>,
    /// The metallic/roughness texture (if any) already uses Bevy's G/B layout.
    pub(super) direct_metallic_roughness: bool,
}

/// Everything the material pass needs from the texture pass.
pub(super) struct ProcessedTextures {
    pub(super) handles: HashMap<u32, TextureVariants>,
    pub(super) material_plans: HashMap<usize, MaterialTexturePlan>,
    /// Per-material UV set ordering of the meshes that use the material.
    pub(super) uv_orderings: HashMap<usize, MaterialUvOrdering>,
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
    let textures = process_textures_internal(scene, settings, load_context)?;
    Ok(textures
        .handles
        .into_iter()
        .filter_map(|(id, variants)| variants.any().map(|handle| (id, handle.clone())))
        .collect())
}

/// Texture pass used by the synchronous compatibility path: embedded images are
/// decoded, external references stay asset-server handles (no packing for them).
pub(super) fn process_textures_internal(
    scene: &ufbx::Scene,
    settings: &FbxLoaderSettings,
    load_context: &mut LoadContext,
) -> Result<ProcessedTextures, FbxError> {
    process_textures_internal_impl(scene, settings, load_context, None)
}

/// External texture read requests: `(texture id, candidate asset paths)`.
///
/// Built from a *synchronous* parse of the scene; the returned values own their
/// strings, so a caller can drop the ufbx scene, read the files, and only then
/// parse/build for real. This keeps the FBX loader future `Send` (`ufbx::Scene`
/// is not `Send`) while still getting external pixels at load time.
pub(crate) fn external_texture_requests(
    scene: &ufbx::Scene,
    fbx_dir: &str,
) -> Vec<(u32, Vec<String>)> {
    let variant_plan = texture_variant_plan(scene);
    let mut requests = Vec::new();
    for texture in scene.textures.as_ref().iter() {
        let id = texture.element.element_id;
        if id == 0 || !texture.content.is_empty() || !variant_plan.mentions(id) {
            continue;
        }
        let candidates = external_texture_candidates(fbx_dir, texture);
        if !candidates.is_empty() {
            requests.push((id, candidates));
        }
    }
    requests
}

/// Asset-base-relative folder of the FBX file being loaded.
pub(crate) fn asset_base_dir(load_context: &LoadContext<'_>) -> String {
    load_context
        .path()
        .path()
        .parent()
        .map(|dir| dir.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Read the files named by [`external_texture_requests`], first candidate first.
///
/// Holds no ufbx data, so this is safe to await inside the asset load task.
pub(crate) async fn read_external_texture_bytes(
    requests: &[(u32, Vec<String>)],
    load_context: &mut LoadContext<'_>,
) -> HashMap<u32, Vec<u8>> {
    let mut bytes_by_id = HashMap::new();
    for (id, candidates) in requests {
        for candidate in candidates {
            if let Ok(bytes) = load_context.read_asset_bytes(candidate.clone()).await {
                bytes_by_id.insert(*id, bytes);
                break;
            }
        }
        if !bytes_by_id.contains_key(id) {
            warn!(
                "External texture '{}' could not be read; keeping the asset-server reference. Opacity and \
                 metallic/roughness packing need the pixels at load time and stay unavailable for this map. \
                 The consequence is a material that never renders: while the file is missing the image handle \
                 never resolves, the StandardMaterial never prepares (bevy_pbr RetryNextUpdate), and its mesh \
                 is not drawn — identical to bevy_gltf with a missing texture URI. The mesh appears once the \
                 file exists at that path, or when the slot is cleared (see `settle_texture` in \
                 examples/materials_pbr_fbx.rs).",
                candidates.first().map(String::as_str).unwrap_or("")
            );
        }
    }
    bytes_by_id
}

/// Texture pass with external image bytes already read by the caller, so
/// opacity / metallic-roughness packing works for external maps too.
pub(super) fn process_textures_with_external_bytes(
    scene: &ufbx::Scene,
    settings: &FbxLoaderSettings,
    load_context: &mut LoadContext,
    external_bytes: &HashMap<u32, Vec<u8>>,
) -> Result<ProcessedTextures, FbxError> {
    process_textures_internal_impl(scene, settings, load_context, Some(external_bytes))
}

fn process_textures_internal_impl(
    scene: &ufbx::Scene,
    settings: &FbxLoaderSettings,
    load_context: &mut LoadContext,
    external_bytes: Option<&HashMap<u32, Vec<u8>>>,
) -> Result<ProcessedTextures, FbxError> {
    let variant_plan = texture_variant_plan(scene);

    // Dense labels: `Texture{N}` follows the same dense-index contract as
    // `Mesh{N}`/`Material{N}`; ufbx element ids are global and sparse, so they
    // must not leak into the label.
    let mut dense_index: HashMap<u32, usize> = HashMap::new();
    for (dense, texture) in scene
        .textures
        .as_ref()
        .iter()
        .filter(|t| t.element.element_id != 0)
        .enumerate()
    {
        dense_index.insert(texture.element.element_id, dense);
    }

    let mut decoded: HashMap<(u32, ColorSpace), DecodedTexture> = HashMap::new();
    for texture in scene.textures.as_ref().iter() {
        let id = texture.element.element_id;
        if id == 0 {
            continue;
        }
        let sampler_descriptor = texture_sampler(texture, settings);
        for space in variant_plan.variants_for(id) {
            if !texture.content.is_empty() {
                match decode_embedded(texture, space, &sampler_descriptor, settings.load_materials)
                {
                    Ok(image) => {
                        decoded.insert((id, space), DecodedTexture::Embedded(image));
                        continue;
                    }
                    Err(reason) => {
                        warn!(
                            "Failed to decode embedded texture '{}': {reason}",
                            texture.filename
                        );
                    }
                }
            }

            // External file read by the async pass: decode it here so the
            // packing step below sees real pixels.
            if let Some(bytes) = external_bytes.and_then(|bytes_by_id| bytes_by_id.get(&id)) {
                match decode_image_bytes(
                    bytes,
                    extension_hint(texture),
                    space,
                    &sampler_descriptor,
                    settings.load_materials,
                ) {
                    Ok(image) => {
                        decoded.insert((id, space), DecodedTexture::Embedded(image));
                        continue;
                    }
                    Err(reason) => {
                        warn!(
                            "Failed to decode external texture '{}': {reason}",
                            texture.filename
                        );
                    }
                }
            }

            // No embedded bytes (or decode failed): fall back to the external
            // file reference, preserving the existing asset-server path.
            if let Some(handle) =
                load_external_texture(texture, space, &sampler_descriptor, settings, load_context)
            {
                decoded.insert((id, space), DecodedTexture::External(handle));
            }
        }
    }

    // Load-time packing (needs the pixels while they are still in memory).
    let uv_orderings = material_uv_orderings(scene);
    let mut pending: Vec<PendingPackedImage> = Vec::new();
    let mut material_plans: HashMap<usize, MaterialTexturePlan> = HashMap::new();
    for (index, material) in scene
        .materials
        .as_ref()
        .iter()
        .filter(|m| m.element.element_id != 0)
        .enumerate()
    {
        let bindings = slot_bindings(material);
        let plan = plan_material_packing(
            index,
            material,
            &bindings,
            uv_orderings.get(&index),
            &decoded,
            settings.load_materials,
            &mut pending,
        );
        material_plans.insert(index, plan);
    }

    // Publish every decoded image under its (dense) texture label.
    let mut handles: HashMap<u32, TextureVariants> = HashMap::new();
    for ((id, space), texture) in decoded {
        let Some(dense) = dense_index.get(&id).copied() else {
            continue;
        };
        let handle = match texture {
            DecodedTexture::Embedded(image) => {
                load_context.add_labeled_asset(texture_label(dense, space), image)
            }
            DecodedTexture::External(handle) => handle,
        };
        handles.entry(id).or_default().set(space, handle);
    }

    // Publish packed images under material-scoped labels.
    for packed in pending {
        let handle = load_context.add_labeled_asset(packed.label, packed.image);
        let plan = material_plans.entry(packed.material_index).or_default();
        match packed.kind {
            PackedKind::BaseColorOpacity => plan.base_color = Some(handle),
            PackedKind::MetallicRoughness => plan.metallic_roughness = Some(handle),
        }
    }

    Ok(ProcessedTextures {
        handles,
        material_plans,
        uv_orderings,
    })
}

/// Compressed texture formats the current render device supports.
///
/// `bevy_gltf` receives `CompressedImageFormatSupport` (the GPU feature truth)
/// at plugin `finish()` time and passes it into `GltfLoader`; a `LoadContext`
/// has no ECS access, so texture processing reads the value `FbxPlugin::finish`
/// recorded in [`crate::cached_supported_compressed_formats`]. Until a plugin
/// has finished (headless tests, `App::update`-only harnesses) the cache
/// reports [`CompressedImageFormats::NONE`], so compressed containers are
/// reported and skipped exactly as `bevy_gltf` skips formats the device does
/// not support — never decoded into GPU formats the device may not support.
/// Decoders that are not compiled into `bevy_image` are still rejected earlier
/// by [`embedded_image_format`]; failure to decode stays warn + fallback
/// instead of failing the load (locked superset over `bevy_gltf`).
fn compressed_image_formats() -> CompressedImageFormats {
    crate::cached_supported_compressed_formats()
}

/// Image container detected from magic bytes, when recognized.
pub(super) fn sniff_image_extension(content: &[u8]) -> Option<&'static str> {
    let starts = |needle: &[u8]| content.starts_with(needle);
    if starts(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("png")
    } else if starts(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if starts(b"DDS ") {
        Some("dds")
    } else if starts(&[0xAB, b'K', b'T', b'X', b' ', b'2', b'0', 0xBB]) {
        Some("ktx2")
    } else if content.len() >= 12 && &content[0..4] == b"RIFF" && &content[8..12] == b"WEBP" {
        Some("webp")
    } else if starts(b"BM") {
        Some("bmp")
    } else if starts(b"GIF8") {
        Some("gif")
    } else if starts(&[0x76, 0x2F, 0x31, 0x01]) {
        Some("exr")
    } else if starts(b"#?") {
        Some("hdr")
    } else if starts(&[0x73, 0x42]) {
        Some("basis")
    } else {
        None
    }
}

/// Pick the decode format for embedded bytes: magic bytes first (FBX filenames
/// routinely lie about the container), then the filename extension.
fn embedded_image_format(content: &[u8], extension_hint: &str) -> Result<ImageFormat, String> {
    if let Some(extension) = sniff_image_extension(content) {
        return ImageFormat::from_extension(extension).ok_or_else(|| {
            format!(
                "detected {extension} content but this build has no {extension} decoder; \
                 enable the matching bevy image feature (bevy_image/{extension})"
            )
        });
    }
    ImageFormat::from_extension(extension_hint).ok_or_else(|| {
        format!(
            "no image container detected from magic bytes and the '{extension_hint}' extension \
             has no decoder in this build"
        )
    })
}

/// Decode embedded texture bytes in the requested color space.
fn decode_embedded(
    texture: &ufbx::Texture,
    space: ColorSpace,
    sampler: &ImageSamplerDescriptor,
    usage: RenderAssetUsages,
) -> Result<Image, String> {
    let content = texture.content.as_ref();
    if content.is_empty() {
        return Err("no embedded content".to_string());
    }
    decode_image_bytes(content, extension_hint(texture), space, sampler, usage)
}

/// Decode image bytes in the requested color space (embedded content or an
/// external file read by the async pass).
fn decode_image_bytes(
    bytes: &[u8],
    extension_hint: &str,
    space: ColorSpace,
    sampler: &ImageSamplerDescriptor,
    usage: RenderAssetUsages,
) -> Result<Image, String> {
    let format = embedded_image_format(bytes, extension_hint)?;
    Image::from_buffer(
        bytes,
        ImageType::Format(format),
        compressed_image_formats(),
        space.is_srgb(),
        ImageSampler::Descriptor(sampler.clone()),
        usage,
    )
    .map_err(|error| error.to_string())
}

/// Load an external texture through the asset server with the slot's color
/// space and the FBX sampler.
fn load_external_texture(
    texture: &ufbx::Texture,
    space: ColorSpace,
    sampler: &ImageSamplerDescriptor,
    settings: &FbxLoaderSettings,
    load_context: &mut LoadContext,
) -> Option<Handle<Image>> {
    let asset_path = load_context.path();
    let fbx_dir = asset_path
        .path()
        .parent()
        .map(|dir| dir.to_string_lossy().to_string())
        .unwrap_or_default();
    let texture_path = external_texture_candidates(&fbx_dir, texture)
        .into_iter()
        .next()?;

    let sampler = sampler.clone();
    let usage = settings.load_materials;
    let is_srgb = space.is_srgb();
    Some(
        load_context
            .load_builder()
            .with_settings(move |image_settings: &mut ImageLoaderSettings| {
                image_settings.is_srgb = is_srgb;
                image_settings.sampler = ImageSampler::Descriptor(sampler.clone());
                image_settings.asset_usage = usage;
            })
            .load(texture_path),
    )
}

/// Join an FBX asset directory with a texture reference and clamp the result to
/// the asset root.
///
/// The join happens *before* the clamp so a reference may climb out of the FBX
/// folder (`../textures/x.png` from `models/` resolves to
/// `textures/x.png`), while `..` can still never climb above the root. Joining
/// the already-clamped reference instead would lose those legitimate parents.
pub(super) fn resolve_reference_in_root(fbx_dir: &str, reference: &str) -> String {
    let dir = normalize_relative_asset_path(fbx_dir);
    let joined = if dir.is_empty() {
        reference.to_string()
    } else {
        format!("{dir}/{reference}")
    };
    normalize_relative_asset_path(&joined)
}

/// Normalize a path fragment into a safe, `/`-separated asset path relative to
/// the asset root: separators are unified and `.`/`..` components are removed so
/// a texture reference cannot escape the root.
pub(super) fn normalize_relative_asset_path(path: &str) -> String {
    let unified = path.replace('\\', "/");
    let stripped = if unified.len() >= 2 && unified.as_bytes()[1] == b':' {
        &unified[2..]
    } else {
        unified.as_str()
    };
    let mut parts: Vec<&str> = Vec::new();
    for part in stripped.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
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

/// Texture reference reduced to a path usable from the FBX file's folder:
/// separators are unified and absolute DCC paths are reduced to their `.fbm`
/// sidecar folder or file name.
fn texture_reference_path(texture: &ufbx::Texture) -> String {
    let filename = if !texture.filename.is_empty() {
        texture.filename.as_ref()
    } else if !texture.absolute_filename.is_empty() {
        texture.absolute_filename.as_ref()
    } else {
        return String::new();
    };

    let unified = filename.replace('\\', "/");
    let is_absolute =
        unified.starts_with('/') || (unified.len() >= 3 && unified.as_bytes()[1] == b':');

    if !is_absolute {
        unified
    } else if let Some(fbm_pos) = unified.rfind(".fbm/") {
        // `.../name.fbx.fbm/tex.png` -> `name.fbx.fbm/tex.png`
        let before_fbm = &unified[..fbm_pos];
        let folder_start = before_fbm.rfind('/').map(|p| p + 1).unwrap_or(0);
        unified[folder_start..].to_string()
    } else {
        match unified.rfind('/') {
            Some(pos) => unified[pos + 1..].to_string(),
            None => unified,
        }
    }
}

/// Asset paths to try for an external texture.
///
/// Exactly one candidate is produced: the reference resolved from the FBX
/// folder with `.`/`..` removed and clamped to the asset root, so a file can
/// never be read outside the asset source while legitimate parents inside it
/// still resolve (see [`resolve_reference_in_root`]). Absolute DCC paths were
/// already reduced to their `.fbm` folder or file name (see
/// [`texture_reference_path`]); references that only resolve by escaping the
/// root intentionally fail here.
fn external_texture_candidates(fbx_dir: &str, texture: &ufbx::Texture) -> Vec<String> {
    let reference = texture_reference_path(texture);
    if reference.is_empty() {
        return Vec::new();
    }
    let candidate = resolve_reference_in_root(fbx_dir, &reference);
    if candidate.is_empty() {
        Vec::new()
    } else {
        vec![candidate]
    }
}

/// `Texture{id}`-style display name for warnings.
pub(super) fn texture_display(texture: &ufbx::Texture) -> String {
    if !texture.filename.is_empty() {
        texture.filename.to_string()
    } else if !texture.element.name.is_empty() {
        texture.element.name.to_string()
    } else {
        format!("Texture{}", texture.element.element_id)
    }
}

/// Embedded pixels for a decoded texture variant, when available in memory.
fn embedded_image<'a>(
    decoded: &'a HashMap<(u32, ColorSpace), DecodedTexture>,
    id: u32,
    space: ColorSpace,
) -> Option<&'a Image> {
    match decoded.get(&(id, space))? {
        DecodedTexture::Embedded(image) => Some(image),
        DecodedTexture::External(_) => None,
    }
}

fn embedded_any<'a>(
    decoded: &'a HashMap<(u32, ColorSpace), DecodedTexture>,
    id: u32,
) -> Option<&'a Image> {
    embedded_image(decoded, id, ColorSpace::Linear)
        .or_else(|| embedded_image(decoded, id, ColorSpace::Srgb))
}

/// Re-express an authored texture UV transform in the loader's flipped-V UV
/// space: the conjugation `F ∘ T ∘ F`.
///
/// FBX UVs are V-up (bottom-origin) — ufbx's own reference renderer maps
/// `uv.y = 0` to the BOTTOM image row (`px = uv * (width, -height)` in
/// `sample_image`, `libs/ufbx/examples/picort/picort.h`, over row-top-down
/// storage in `libs/ufbx/examples/picort/picort_png.cpp`). Bevy images are
/// V-down (top-origin, PNG row 0 first), so `crate::mesh` stores mesh UVs as
/// `F(u, v) = (u, 1 - v)`: that mesh flip alone is what maps FBX UVs into
/// Bevy's image space. An authored transform `T` must therefore be
/// conjugated by the flip, not composed with it — the shader matrix `M` has
/// to satisfy `M(F(uv)) = F(T(uv))`, i.e. `M = F ∘ T ∘ F`. Bevy's `Affine2`
/// multiplies column vectors (`(a * b).transform_point2(p) ==
/// a.transform_point2(b.transform_point2(p))`), so this is literally
/// `flip * transform * flip`.
///
/// Consequences: an identity authored transform yields `Affine2::IDENTITY`
/// (the mesh flip alone does the FBX→Bevy mapping), and an authored scale
/// `(sx, sy)` plus translation `(tx, ty)` (no rotation) yields
/// `x' = sx*x + tx`, `y' = sy*y + (1 - sy - ty)`.
pub(super) fn compensate_v_flip(transform: Affine2) -> Affine2 {
    let flip =
        Affine2::from_scale_angle_translation(Vec2::new(1.0, -1.0), 0.0, Vec2::new(0.0, 1.0));
    flip * transform * flip
}

/// The transform the shader applies to one texture: the authored UV
/// transform conjugated into the loader's flipped-V UV space.
pub(super) fn unit_uv_transform(texture: &ufbx::Texture) -> Affine2 {
    compensate_v_flip(convert_texture_uv_transform(texture))
}

/// Ordinal from a numbered DCC UV set name (`UVChannel_1`, `map1`, ...).
///
/// Every convention that numbers UV sets starts at 1, so `UVChannel_1` is the
/// first set (Bevel channel 0). Names without digits are the first set as well.
pub(super) fn numbered_uv_set_ordinal(uv_set: &str) -> u32 {
    let digits: String = uv_set
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if digits.is_empty() {
        0
    } else {
        digits
            .parse::<u32>()
            .map(|number| number.saturating_sub(1))
            .unwrap_or(0)
    }
}

/// UV set ordinals observed for one material across the meshes that use it.
///
/// ufbx UV set names are DCC labels, not ordinals: a mesh can order
/// `["UVChannel_2", "UVChannel_1"]`, so only the mesh's own `uv_sets` order
/// tells which Bevy channel a name maps to.
#[derive(Debug, Default)]
pub(super) struct MaterialUvOrdering {
    /// uv_set name -> (ordinal in the first user mesh, users disagree).
    names: HashMap<String, (u32, bool)>,
}

impl MaterialUvOrdering {
    pub(super) fn insert(&mut self, name: &str, ordinal: u32) {
        if name.is_empty() {
            return;
        }
        match self.names.get_mut(name) {
            Some((existing, conflicted)) => {
                if *existing != ordinal {
                    *conflicted = true;
                }
            }
            None => {
                self.names.insert(name.to_string(), (ordinal, false));
            }
        }
    }

    /// 0-based channel ordinal for `uv_set`, plus whether the material's users
    /// order that name differently (one Bevy channel cannot represent both).
    pub(super) fn channel_for(&self, uv_set: &str) -> (u32, bool) {
        match self.names.get(uv_set) {
            Some((ordinal, conflicted)) => (*ordinal, *conflicted),
            // Never seen on a user mesh (custom name or the material is unused):
            // numbered DCC names still carry their own ordinal.
            None => (numbered_uv_set_ordinal(uv_set), false),
        }
    }
}

/// Scene-derived UV ordering: material dense index -> its users' UV set order.
///
/// Dense indices match the `Material{N}` labels (`scene.materials` without the
/// reserved element 0).
pub(super) fn material_uv_orderings(scene: &ufbx::Scene) -> HashMap<usize, MaterialUvOrdering> {
    let mut dense: HashMap<u32, usize> = HashMap::new();
    for (index, material) in scene
        .materials
        .as_ref()
        .iter()
        .filter(|material| material.element.element_id != 0)
        .enumerate()
    {
        dense.insert(material.element.element_id, index);
    }

    let mut orderings: HashMap<usize, MaterialUvOrdering> = HashMap::new();
    for node in scene.nodes.as_ref().iter() {
        let Some(mesh) = node.mesh.as_deref() else {
            continue;
        };
        for material in &node.materials {
            let Some(index) = dense.get(&material.element.element_id) else {
                continue;
            };
            let entry = orderings.entry(*index).or_default();
            for (ordinal, uv_set) in mesh.uv_sets.as_ref().iter().enumerate() {
                entry.insert(&uv_set.name, ordinal as u32);
            }
        }
    }
    orderings
}

#[cfg(test)]
mod tests {
    use super::*;

    /// H5: the decode path consumes the plugin's cached device support, so a
    /// run without a finished `FbxPlugin` (headless tests) filters compressed
    /// containers down to whatever the cache reports — `NONE` until
    /// [`crate::FbxPlugin::finish`] records the render device's formats.
    #[test]
    fn compressed_image_formats_follow_the_plugin_cache() {
        assert_eq!(
            compressed_image_formats(),
            crate::cached_supported_compressed_formats()
        );
    }

    /// Clearcoat maps are non-color data: `bevy_gltf` loads all three
    /// `KHR_materials_clearcoat` textures linear (`gltf_ext/mod.rs:75-83`) and
    /// bevy_pbr documents them as "must not be loaded as sRGB".
    #[test]
    fn clearcoat_slots_decode_linear() {
        for slot in [
            TextureSlot::Clearcoat,
            TextureSlot::ClearcoatRoughness,
            TextureSlot::ClearcoatNormal,
        ] {
            assert_eq!(slot.color_space(), ColorSpace::Linear, "{slot:?}");
        }
    }
}

/// UV sampling identity of one texture reference: the channel ordinal a user
/// mesh assigns to its UV set, the transform the shader applies and the sampler
/// wrap modes.
///
/// A packed image is sampled through a single channel, transform and sampler, so
/// two maps may only be packed together when these match.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct UvSampling {
    pub(super) channel: u32,
    pub(super) transform: Affine2,
    pub(super) address_u: ImageAddressMode,
    pub(super) address_v: ImageAddressMode,
}

/// Sampling identity used for `texture` in `material`'s context.
pub(super) fn uv_sampling(
    ordering: Option<&MaterialUvOrdering>,
    texture: &ufbx::Texture,
) -> UvSampling {
    let uv_set = texture.uv_set.as_ref();
    let channel = match ordering {
        Some(ordering) => ordering.channel_for(uv_set).0,
        None => numbered_uv_set_ordinal(uv_set),
    };
    UvSampling {
        channel,
        transform: unit_uv_transform(texture),
        address_u: wrap_to_address_mode(texture.wrap_u),
        address_v: wrap_to_address_mode(texture.wrap_v),
    }
}

/// Decide load-time packing for one material and queue the packed images.
fn plan_material_packing(
    material_index: usize,
    material: &ufbx::Material,
    bindings: &[SlotBinding<'_>],
    ordering: Option<&MaterialUvOrdering>,
    decoded: &HashMap<(u32, ColorSpace), DecodedTexture>,
    usage: RenderAssetUsages,
    pending: &mut Vec<PendingPackedImage>,
) -> MaterialTexturePlan {
    let mut plan = MaterialTexturePlan::default();
    let material_name = if material.element.name.is_empty() {
        format!("Material{material_index}")
    } else {
        material.element.name.to_string()
    };
    let scalar_opacity = resolve_opacity_alpha(material).unwrap_or(1.0);

    // ---- opacity mask -> base color alpha ----
    if let Some(binding) = bindings.iter().find(|b| b.slot == TextureSlot::Opacity) {
        let opacity_source = embedded_any(decoded, binding.id());
        let base_binding = bindings.iter().find(|b| b.slot == TextureSlot::BaseColor);
        let base_source = base_binding.and_then(|b| embedded_any(decoded, b.id()));
        // A single packed image is sampled through one channel, transform and
        // sampler, so the mask may only be packed when both maps match.
        let sampling_mismatch = base_binding.is_some_and(|base| {
            uv_sampling(ordering, base.texture) != uv_sampling(ordering, binding.texture)
        });
        let base_name = base_binding
            .map(|base| format!("'{}' (Texture{})", texture_display(base.texture), base.id()))
            .unwrap_or_else(|| "the base color map".to_string());
        if sampling_mismatch {
            warn!(
                "The base color map {base_name} and the opacity map '{}' (Texture{}) of \
                 '{material_name}' are sampled through different UV channels, transforms, or wrap modes. \
                 Bevy samples opacity from the base color texture's alpha, so packing them would sample one \
                 of the maps wrongly. The opacity map is not packed and mesh alpha comes from the scalar \
                 opacity only.",
                texture_display(binding.texture),
                binding.id()
            );
        } else if let Some(opacity) = opacity_source {
            if base_binding.is_some() && base_source.is_none() {
                warn!(
                    "Opacity texture '{}' (Texture{}) of '{material_name}' cannot be packed into the base \
                     color alpha: the base color texture is not decoded in memory (unreadable or \
                     unsupported external file, or decode failure). Mesh alpha will come from the scalar \
                     opacity only.",
                    texture_display(binding.texture),
                    binding.id()
                );
            } else {
                match compose_base_color_alpha(base_source, opacity, scalar_opacity, usage) {
                    Ok(image) => pending.push(PendingPackedImage {
                        label: format!("Material{material_index}/BaseColorOpacity"),
                        image,
                        material_index,
                        kind: PackedKind::BaseColorOpacity,
                    }),
                    Err(reason) => {
                        warn!(
                            "Opacity texture '{}' (Texture{}) of '{material_name}' cannot be packed into the \
                             base color alpha: {reason}. Mesh alpha will come from the scalar opacity only.",
                            texture_display(binding.texture),
                            binding.id()
                        );
                    }
                }
            }
        } else {
            warn!(
                "Opacity texture '{}' (Texture{}) of '{material_name}' was not decoded in memory (unreadable \
                 or unsupported external file, or decode failure): Bevy samples opacity from the base color \
                 texture's alpha channel, so the opacity map is not packed. Mesh alpha will come from the \
                 scalar opacity only.",
                texture_display(binding.texture),
                binding.id()
            );
        }
    }

    // ---- separate metallic/roughness maps -> Bevy's G/B layout ----
    let metalness = bindings.iter().find(|b| b.slot == TextureSlot::Metalness);
    let roughness = bindings.iter().find(|b| b.slot == TextureSlot::Roughness);
    let gltf_packed = bindings
        .iter()
        .any(|b| b.slot == TextureSlot::MetallicRoughness);
    if !gltf_packed {
        match (metalness, roughness) {
            (Some(metalness), Some(roughness)) if metalness.id() != roughness.id() => {
                let metal = embedded_image(decoded, metalness.id(), ColorSpace::Linear);
                let rough = embedded_image(decoded, roughness.id(), ColorSpace::Linear);
                if uv_sampling(ordering, metalness.texture)
                    != uv_sampling(ordering, roughness.texture)
                {
                    warn!(
                        "Separate metallic '{}' (Texture{}) and roughness '{}' (Texture{}) maps of \
                         '{material_name}' are sampled through different UV channels, transforms, or wrap \
                         modes. Bevy samples both from one texture, so packing them would sample one of the \
                         maps wrongly; the metallic/roughness slot is left unbound.",
                        texture_display(metalness.texture),
                        metalness.id(),
                        texture_display(roughness.texture),
                        roughness.id()
                    );
                } else {
                    match (metal, rough) {
                        (Some(metal), Some(rough)) => {
                            match pack_metallic_roughness(Some(metal), Some(rough), usage) {
                                Ok(image) => pending.push(PendingPackedImage {
                                    label: format!("Material{material_index}/MetallicRoughness"),
                                    image,
                                    material_index,
                                    kind: PackedKind::MetallicRoughness,
                                }),
                                Err(reason) => {
                                    warn!(
                                        "Metallic/roughness maps of '{material_name}' could not be packed: \
                                         {reason}. The metallic/roughness slot is left unbound."
                                    );
                                }
                            }
                        }
                        _ => {
                            warn!(
                                "Separate metallic '{}' (Texture{}) and roughness '{}' (Texture{}) maps of \
                                 '{material_name}' are not both decoded in memory (unreadable or unsupported \
                                 external file, or decode failure). Bevy samples metallic from B and roughness \
                                 from G of one texture, so the maps are not bound instead of shading from the \
                                 wrong channel.",
                                texture_display(metalness.texture),
                                metalness.id(),
                                texture_display(roughness.texture),
                                roughness.id()
                            );
                        }
                    }
                }
            }
            (Some(_), Some(_)) => {
                // One texture bound as both maps: exporters that write the glTF
                // layout store roughness in G and metallic in B; pass it through.
                plan.direct_metallic_roughness = true;
            }
            (Some(single), None) | (None, Some(single)) => {
                let channel = if metalness.is_some() {
                    "metallic"
                } else {
                    "roughness"
                };
                match embedded_image(decoded, single.id(), ColorSpace::Linear) {
                    Some(image) => {
                        let (metal, rough) = if metalness.is_some() {
                            (Some(image), None)
                        } else {
                            (None, Some(image))
                        };
                        match pack_metallic_roughness(metal, rough, usage) {
                            Ok(image) => pending.push(PendingPackedImage {
                                label: format!("Material{material_index}/MetallicRoughness"),
                                image,
                                material_index,
                                kind: PackedKind::MetallicRoughness,
                            }),
                            Err(reason) => {
                                warn!(
                                    "The {channel} map '{}' (Texture{}) of '{material_name}' could not be packed: \
                                     {reason}. The metallic/roughness slot is left unbound.",
                                    texture_display(single.texture),
                                    single.id()
                                );
                            }
                        }
                    }
                    None => {
                        warn!(
                            "The {channel} map '{}' (Texture{}) of '{material_name}' was not decoded in memory \
                             (unreadable or unsupported external file, or decode failure): Bevy samples metallic \
                             from B and roughness from G of one texture, so the map is not bound instead of \
                             shading from the wrong channel.",
                            texture_display(single.texture),
                            single.id()
                        );
                    }
                }
            }
            (None, None) => {}
        }
    }

    plan
}
