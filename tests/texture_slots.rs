//! Texture slot fidelity tests over real corpus FBX files.
//!
//! Covers the two load paths: embedded textures (decoded in-process) and
//! external texture files (read through the load context before the scene is
//! processed), plus the per-slot color space, dense `Texture{N}` labels and the
//! channel packing that Bevy's `StandardMaterial` requires.
//!
//! Fixtures (a ufbx corpus, found through `UFBX_TEST_DATA` or the local
//! `../../libs/ufbx/data` pack; the corpus tests skip with a printed reason when
//! neither exists):
//! - `max_physical_material_textures_6100_binary.fbx`: embedded base color,
//!   transparency, roughness, metallic, normal and emissive maps on UV1.
//! - `blender_293_textures_7400_binary.fbx`: the same material with external
//!   files instead of embedded content.

use std::path::{Path, PathBuf};
use std::time::Duration;

use bevy::animation::AnimationClip;
use bevy::asset::{AssetPath, AssetPlugin, AssetServer, LoadState, RenderAssetUsages};
use bevy::image::{CompressedImageFormats, ImageFormat, ImageSampler, ImageType};
use bevy::material::AlphaMode;
use bevy::mesh::UvChannel;
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy::world_serialization::WorldAsset;
use bevy_ufbx::{Fbx, FbxMaterial, FbxPlugin};

/// Local corpus pack, expected next to this repository.
const LOCAL_CORPUS: &str = "../../libs/ufbx/data";

/// Corpus root for the fixture tests.
///
/// `UFBX_TEST_DATA` wins when set (CI points it at its own ufbx data checkout);
/// an explicitly set but unusable variable is a hard error so CI cannot quietly
/// test nothing. Without the variable the manifest-relative local pack is used,
/// and `None` means neither exists and the corpus tests skip.
fn corpus_root() -> Option<PathBuf> {
    if let Some(value) = std::env::var_os("UFBX_TEST_DATA") {
        let path = PathBuf::from(value);
        assert!(
            path.is_dir(),
            "UFBX_TEST_DATA is set but is not an existing directory: {}",
            path.display()
        );
        return Some(path);
    }
    let local = Path::new(env!("CARGO_MANIFEST_DIR")).join(LOCAL_CORPUS);
    local.is_dir().then_some(local)
}

/// [`corpus_root`] for one test, printing why it is skipped when absent.
fn corpus_or_skip(test: &str) -> Option<PathBuf> {
    match corpus_root() {
        Some(root) => Some(root),
        None => {
            eprintln!(
                "skipping {test}: no FBX corpus found. Set UFBX_TEST_DATA to a ufbx test data \
                 directory or check out the local pack at {LOCAL_CORPUS}."
            );
            None
        }
    }
}

fn headless_app(root: &Path) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin {
            file_path: root.to_string_lossy().into_owned(),
            ..default()
        })
        .add_plugins(FbxPlugin)
        .init_asset::<Mesh>()
        .init_asset::<StandardMaterial>()
        .init_asset::<FbxMaterial>()
        .init_asset::<Image>()
        .init_asset::<AnimationClip>()
        .init_asset::<WorldAsset>();
    app
}

fn wait_for_load(app: &mut App, handle: &Handle<Fbx>, max_frames: usize) -> bool {
    for _ in 0..max_frames {
        app.update();
        let server = app.world().resource::<AssetServer>();
        match server.load_state(handle) {
            LoadState::Loaded => return true,
            LoadState::Failed(_) => return false,
            _ => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    false
}

/// Load a fixture and settle its labeled dependency assets.
fn load_fixture(root: &Path, file: &'static str) -> (App, Handle<Fbx>) {
    let mut app = headless_app(root);
    let fbx_handle: Handle<Fbx> = app.world().resource::<AssetServer>().load(file);
    assert!(
        wait_for_load(&mut app, &fbx_handle, 500),
        "{file} failed to load"
    );
    for _ in 0..10 {
        app.update();
        std::thread::sleep(Duration::from_millis(10));
    }
    (app, fbx_handle)
}

/// The `StandardMaterial` of the material named `name`, and its asset labels.
fn material_by_name(
    app: &mut App,
    fbx_handle: &Handle<Fbx>,
    name: &str,
) -> (StandardMaterial, Vec<String>) {
    let (material, labels) = {
        let fbxs = app.world().resource::<Assets<Fbx>>();
        let fbx = fbxs.get(fbx_handle).expect("Fbx");
        let fbx_materials = app.world().resource::<Assets<FbxMaterial>>();
        let standard_materials = app.world().resource::<Assets<StandardMaterial>>();
        let server = app.world().resource::<AssetServer>();
        let fbx_material = fbx
            .materials
            .iter()
            .map(|handle| fbx_materials.get(handle).expect("FbxMaterial"))
            .find(|material| material.name == name)
            .unwrap_or_else(|| panic!("material {name}"));
        let material = standard_materials
            .get(&fbx_material.material)
            .expect("StandardMaterial")
            .clone();
        let labels = [
            material.base_color_texture.as_ref(),
            material.emissive_texture.as_ref(),
            material.normal_map_texture.as_ref(),
            material.metallic_roughness_texture.as_ref(),
            material.occlusion_texture.as_ref(),
        ]
        .into_iter()
        .flatten()
        .map(|handle| {
            server
                .get_path(handle.id())
                .map(|path| path.to_string())
                .unwrap_or_default()
        })
        .collect();
        (material, labels)
    };
    (material, labels)
}

fn decode_png(bytes: &[u8], is_srgb: bool) -> Image {
    Image::from_buffer(
        bytes,
        ImageType::Format(ImageFormat::Png),
        CompressedImageFormats::NONE,
        is_srgb,
        ImageSampler::Default,
        RenderAssetUsages::default(),
    )
    .expect("png decode")
}

fn raw(image: &Image, x: u32, y: u32) -> [f32; 4] {
    let color = image.get_color_at(x, y).expect("pixel");
    match color {
        Color::Srgba(color) => [color.red, color.green, color.blue, color.alpha],
        Color::LinearRgba(color) => [color.red, color.green, color.blue, color.alpha],
        other => {
            let color = Srgba::from(other);
            [color.red, color.green, color.blue, color.alpha]
        }
    }
}

fn assert_close(actual: f32, expected: f32, what: &str, x: u32, y: u32) {
    assert!(
        (actual - expected).abs() <= 1.5 / 255.0,
        "{what} at ({x},{y}): {actual} vs {expected}"
    );
}

fn label_of(labels: &[String], needle: &str) -> bool {
    labels.iter().any(|label| label.contains(needle))
}

/// Embedded fixture: opacity packed into base color alpha, separate
/// metallic/roughness maps packed into G/B, per-slot color spaces.
#[test]
fn embedded_fixture_packs_opacity_and_metallic_roughness() {
    let Some(corpus) = corpus_or_skip("embedded_fixture_packs_opacity_and_metallic_roughness")
    else {
        return;
    };
    let file = "max_physical_material_textures_6100_binary.fbx";
    let (mut app, fbx_handle) = load_fixture(&corpus, file);
    let (material, labels) = material_by_name(&mut app, &fbx_handle, "PhysicalMaterial");

    // Slots.
    let base = material
        .base_color_texture
        .clone()
        .expect("base color texture");
    let mr = material
        .metallic_roughness_texture
        .clone()
        .expect("metallic/roughness texture");
    let normal = material
        .normal_map_texture
        .clone()
        .expect("normal map texture");
    let emissive = material.emissive_texture.clone().expect("emissive texture");
    assert!(material.occlusion_texture.is_none());

    // Opacity is a cutout here (transparency texture, no scalar factor).
    assert!(matches!(material.alpha_mode, AlphaMode::Mask(0.5)));
    assert_close(
        material.base_color.alpha(),
        1.0,
        "scalar alpha stays on the mask",
        0,
        0,
    );

    // UV slot selection and the 1-v mesh flip compensation. The fixture's mesh
    // has a single UV set (named `UVChannel_1`), so its ordinal is UV0: the name
    // is a DCC label, not the mesh's channel order.
    assert!(matches!(material.base_color_channel, UvChannel::Uv0));
    assert!(matches!(
        material.metallic_roughness_channel,
        UvChannel::Uv0
    ));
    assert!(matches!(material.normal_map_channel, UvChannel::Uv0));
    assert_close(
        material.uv_transform.matrix2.y_axis.y,
        -1.0,
        "uv flip",
        0,
        0,
    );
    assert_close(material.uv_transform.translation.y, 1.0, "uv flip", 0, 0);
    assert_close(material.uv_transform.matrix2.x_axis.x, 1.0, "uv flip", 0, 0);

    // Dense labels: the emissive map is the sRGB variant of texture 6, the
    // normal map the linear-only variant of texture 8; packed images carry
    // material-scoped labels.
    assert!(label_of(&labels, "#Texture6"), "emissive label: {labels:?}");
    assert!(
        label_of(&labels, "#Texture8/Linear"),
        "normal label: {labels:?}"
    );
    assert!(
        label_of(&labels, "#Material0/BaseColorOpacity"),
        "base label: {labels:?}"
    );
    assert!(
        label_of(&labels, "#Material0/MetallicRoughness"),
        "mr label: {labels:?}"
    );

    // Per-slot color spaces: color slots sRGB, data slots linear.
    let images = app.world().resource::<Assets<Image>>();
    let base_image = images.get(&base).expect("packed base color");
    let mr_image = images.get(&mr).expect("packed metallic/roughness");
    let normal_image = images.get(&normal).expect("normal map");
    let emissive_image = images.get(&emissive).expect("emissive");
    assert_eq!(
        base_image.texture_descriptor.format,
        TextureFormat::Rgba8UnormSrgb
    );
    assert_eq!(
        mr_image.texture_descriptor.format,
        TextureFormat::Rgba8Unorm
    );
    assert_eq!(
        normal_image.texture_descriptor.format,
        TextureFormat::Rgba8Unorm
    );
    assert_eq!(
        emissive_image.texture_descriptor.format,
        TextureFormat::Rgba8UnormSrgb
    );

    // Pixel fidelity against the authored source maps.
    let path = corpus.join(file).to_string_lossy().into_owned();
    let scene = ufbx::load_file(&path, ufbx::LoadOpts::default()).expect("ufbx");
    let source = |suffix: &str| -> Vec<u8> {
        scene
            .textures
            .as_ref()
            .iter()
            .find(|texture| texture.filename.ends_with(suffix))
            .unwrap_or_else(|| panic!("embedded source {suffix}"))
            .content
            .as_ref()
            .to_vec()
    };
    let diffuse = decode_png(&source("checkerboard_diffuse.png"), true);
    let transparency = decode_png(&source("checkerboard_transparency.png"), true);
    let roughness = decode_png(&source("checkerboard_roughness.png"), false);
    let metallic = decode_png(&source("checkerboard_metallic.png"), false);

    assert_eq!(
        (base_image.width(), base_image.height()),
        (diffuse.width(), diffuse.height())
    );
    assert_eq!(
        (mr_image.width(), mr_image.height()),
        (roughness.width(), roughness.height())
    );

    let (mut alpha_low, mut alpha_high) = (1.0f32, 0.0f32);
    let mut rgb_is_not_grayscale = false;
    for y in 0..base_image.height() {
        for x in 0..base_image.width() {
            let packed = raw(base_image, x, y);
            let color = raw(&diffuse, x, y);
            let mask = raw(&transparency, x, y);
            assert_close(packed[0], color[0], "base color R", x, y);
            assert_close(packed[1], color[1], "base color G", x, y);
            assert_close(packed[2], color[2], "base color B", x, y);
            let expected_alpha = 0.2126 * mask[0] + 0.7152 * mask[1] + 0.0722 * mask[2];
            assert_close(packed[3], expected_alpha, "packed opacity alpha", x, y);
            alpha_low = alpha_low.min(packed[3]);
            alpha_high = alpha_high.max(packed[3]);
            if (packed[0] - packed[1]).abs() > 2.0 / 255.0 {
                rgb_is_not_grayscale = true;
            }
        }
    }
    // The mask is a luminance mask: the source alpha channel is 255 everywhere
    // while the packed alpha varies with the mask.
    assert!(
        alpha_high - alpha_low > 0.5,
        "opacity mask not packed: {alpha_low}..{alpha_high}"
    );
    assert!(rgb_is_not_grayscale, "base color replaced by the mask");
    assert_close(
        raw(&transparency, 0, 0)[3],
        1.0,
        "source mask alpha stays 255",
        0,
        0,
    );

    // G = roughness, B = metallic, from two different maps.
    let mut g_b_differ = false;
    for y in 0..mr_image.height() {
        for x in 0..mr_image.width() {
            let packed = raw(mr_image, x, y);
            assert_close(
                packed[1],
                raw(&roughness, x, y)[0],
                "packed roughness (G)",
                x,
                y,
            );
            assert_close(
                packed[2],
                raw(&metallic, x, y)[0],
                "packed metallic (B)",
                x,
                y,
            );
            assert_close(packed[0], 1.0, "packed R unused", x, y);
            assert_close(packed[3], 1.0, "packed A unused", x, y);
            if (packed[1] - packed[2]).abs() > 2.0 / 255.0 {
                g_b_differ = true;
            }
        }
    }
    assert!(
        g_b_differ,
        "roughness and metallic packed from the same source"
    );
}

/// External fixture: the Fbx references texture files on disk; the loader reads
/// them through the load context so the same packing applies.
#[test]
fn external_fixture_packs_metallic_roughness_from_disk() {
    let Some(corpus) = corpus_or_skip("external_fixture_packs_metallic_roughness_from_disk") else {
        return;
    };
    let file = "blender_293_textures_7400_binary.fbx";
    let (mut app, fbx_handle) = load_fixture(&corpus, file);
    let (material, labels) = material_by_name(&mut app, &fbx_handle, "Material.001");

    let base = material
        .base_color_texture
        .clone()
        .expect("base color texture");
    let mr = material
        .metallic_roughness_texture
        .clone()
        .expect("metallic/roughness packed from external files");
    assert!(material.normal_map_texture.is_none());

    // External pixels are read during the load, so they keep their FBX labels
    // and per-slot color space.
    assert!(label_of(&labels, "#Texture0"), "base label: {labels:?}");
    assert!(
        label_of(&labels, "#Material0/MetallicRoughness"),
        "mr label: {labels:?}"
    );

    let images = app.world().resource::<Assets<Image>>();
    let base_image = images.get(&base).expect("external base color");
    let mr_image = images.get(&mr).expect("packed metallic/roughness");
    assert_eq!(
        base_image.texture_descriptor.format,
        TextureFormat::Rgba8UnormSrgb
    );
    assert_eq!(
        mr_image.texture_descriptor.format,
        TextureFormat::Rgba8Unorm
    );

    // Pixel fidelity against the referenced files on disk.
    let read = |name: &str| {
        std::fs::read(corpus.join("textures").join(name))
            .unwrap_or_else(|error| panic!("{name}: {error}"))
    };
    let diffuse = decode_png(&read("checkerboard_diffuse.png"), true);
    let roughness = decode_png(&read("checkerboard_roughness.png"), false);
    let metallic = decode_png(&read("checkerboard_metallic.png"), false);

    let mut g_b_differ = false;
    for y in 0..mr_image.height() {
        for x in 0..mr_image.width() {
            let packed = raw(mr_image, x, y);
            assert_close(
                packed[1],
                raw(&roughness, x, y)[0],
                "packed roughness (G)",
                x,
                y,
            );
            assert_close(
                packed[2],
                raw(&metallic, x, y)[0],
                "packed metallic (B)",
                x,
                y,
            );
            if (packed[1] - packed[2]).abs() > 2.0 / 255.0 {
                g_b_differ = true;
            }
        }
    }
    assert!(
        g_b_differ,
        "roughness and metallic packed from the same source"
    );
    for y in 0..base_image.height() {
        for x in 0..base_image.width() {
            assert_close(
                raw(base_image, x, y)[0],
                raw(&diffuse, x, y)[0],
                "external base R",
                x,
                y,
            );
        }
    }
}

/// Labeled texture variants stay loadable by label from the asset server.
#[test]
fn texture_variants_are_loadable_by_label() {
    let Some(corpus) = corpus_or_skip("texture_variants_are_loadable_by_label") else {
        return;
    };
    let file = "max_physical_material_textures_6100_binary.fbx";
    let (mut app, fbx_handle) = load_fixture(&corpus, file);
    let _ = material_by_name(&mut app, &fbx_handle, "PhysicalMaterial");

    let handle: Handle<Image> = app
        .world()
        .resource::<AssetServer>()
        .load(AssetPath::from(file).with_label("Texture6"));
    for _ in 0..50 {
        app.update();
        if app
            .world()
            .resource::<Assets<Image>>()
            .get(&handle)
            .is_some()
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let images = app.world().resource::<Assets<Image>>();
    let image = images.get(&handle).expect("Texture6 labeled image");
    assert_eq!(
        image.texture_descriptor.format,
        TextureFormat::Rgba8UnormSrgb
    );
}

/// A 16x16 JPEG (4:4:4; four 8x8 quadrants: red, green, blue, white).
///
/// Generated locally from a PPM with `ffmpeg -pix_fmt yuvj444p -q:v 3`, 218
/// bytes. Decoded quadrants measure 254/0/0, 0/255/0, 0/0/254 and 255/255/255.
const JPEG_FIXTURE: &[u8] = &[
    0xFF, 0xD8, 0xFF, 0xFE, 0x00, 0x10, 0x4C, 0x61, 0x76, 0x63, 0x36, 0x32, 0x2E, 0x32, 0x38, 0x2E,
    0x31, 0x30, 0x32, 0x00, 0xFF, 0xDB, 0x00, 0x43, 0x00, 0x08, 0x06, 0x06, 0x07, 0x06, 0x07, 0x08,
    0x08, 0x08, 0x08, 0x08, 0x08, 0x09, 0x09, 0x09, 0x0A, 0x0A, 0x0A, 0x09, 0x09, 0x09, 0x09, 0x0A,
    0x0A, 0x0A, 0x0A, 0x0A, 0x0A, 0x0C, 0x0C, 0x0C, 0x0A, 0x0A, 0x0A, 0x0A, 0x0A, 0x0A, 0x0A, 0x0C,
    0x0C, 0x0C, 0x0C, 0x0D, 0x0E, 0x0D, 0x0D, 0x0D, 0x0C, 0x0D, 0x0E, 0x0E, 0x0F, 0x0F, 0x0F, 0x12,
    0x12, 0x11, 0x11, 0x15, 0x15, 0x15, 0x19, 0x19, 0x1F, 0xFF, 0xC4, 0x00, 0x4D, 0x00, 0x01, 0x01,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x06, 0x07,
    0x01, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x07, 0x08, 0x06, 0x10, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x11, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0x10, 0x00,
    0x10, 0x03, 0x01, 0x12, 0x00, 0x02, 0x12, 0x00, 0x03, 0x12, 0x00, 0xFF, 0xDA, 0x00, 0x0C, 0x03,
    0x01, 0x00, 0x02, 0x11, 0x03, 0x11, 0x00, 0x3F, 0x00, 0x16, 0x20, 0xC9, 0xAA, 0xB3, 0xFA, 0x6B,
    0x5E, 0x56, 0x94, 0x56, 0x55, 0x14, 0x9A, 0xDF, 0xFF, 0xD9,
];

/// JPEG decoding must work for consumers of this crate.
///
/// `bevy_ufbx` depends on `bevy` with `default-features = false`, and Bevy
/// enables image decoders one at a time: `png` arrives through its `common_api`
/// feature set, `jpeg` through nothing, so this crate has to declare it (see
/// `Cargo.toml`). The dev-dependency's default features do not enable `jpeg`
/// either, so this test fails if that declaration is dropped.
///
/// Mirrors the texture pass: container from `ImageFormat::from_extension`, then
/// `Image::from_buffer` with `is_srgb`.
#[test]
fn jpeg_texture_decoder_is_available() {
    let Some(format) = ImageFormat::from_extension("jpg") else {
        panic!("bevy 'jpeg' feature missing: declare it on the bevy dependency in Cargo.toml");
    };
    let image = Image::from_buffer(
        JPEG_FIXTURE,
        ImageType::Format(format),
        CompressedImageFormats::NONE,
        true,
        ImageSampler::Default,
        RenderAssetUsages::default(),
    )
    .expect("JPEG decode");

    assert_eq!((image.width(), image.height()), (16, 16));
    assert_eq!(
        image.texture_descriptor.format,
        TextureFormat::Rgba8UnormSrgb
    );

    let quadrants = [
        (2u32, 2u32, [1.0f32, 0.0, 0.0], "top-left red"),
        (13, 2, [0.0, 1.0, 0.0], "top-right green"),
        (2, 13, [0.0, 0.0, 1.0], "bottom-left blue"),
        (13, 13, [1.0, 1.0, 1.0], "bottom-right white"),
    ];
    for (x, y, expected, what) in quadrants {
        let texel = raw(&image, x, y);
        for channel in 0..3 {
            assert_close(texel[channel], expected[channel], what, x, y);
        }
    }
}
