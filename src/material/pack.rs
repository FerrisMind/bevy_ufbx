//! Pixel-level packing for material maps whose layout differs from Bevy's.
//!
//! Bevy samples opacity from the base color image's alpha channel and
//! metallic/roughness from the B/G channels of one texture; FBX stores those as
//! separate grayscale maps. These helpers rewrite the pixels into the layout the
//! shader actually samples (load-time warnings live in `texture.rs`).

use bevy::asset::RenderAssetUsages;
use bevy::color::{Color, Srgba};
use bevy::image::Image;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

/// Read the raw (stored) components of a pixel without color-space conversion.
///
/// For `Rgba8UnormSrgb`/`Rgba8Unorm` sources these are the authored bytes
/// scaled to `0..1` in the image's own space, which is what a data map must be
/// sampled as.
fn raw_components(color: Color) -> [f32; 4] {
    match color {
        Color::Srgba(color) => [color.red, color.green, color.blue, color.alpha],
        Color::LinearRgba(color) => [color.red, color.green, color.blue, color.alpha],
        other => {
            let color = Srgba::from(other);
            [color.red, color.green, color.blue, color.alpha]
        }
    }
}

fn raw_texel(image: &Image, x: u32, y: u32) -> Result<[f32; 4], String> {
    image
        .get_color_at(x, y)
        .map(raw_components)
        .map_err(|error| error.to_string())
}

/// Stored luminance of a texel (grayscale masks are exact: `r == g == b`).
fn texel_luminance(texel: [f32; 4]) -> f32 {
    0.2126 * texel[0] + 0.7152 * texel[1] + 0.0722 * texel[2]
}

/// Does the image carry a real alpha channel (any pixel with alpha < 1)?
///
/// FBX opacity masks are stored as RGBA images whose alpha is 255 everywhere
/// (verified on the ufbx corpus checkerboards), so those must be read as
/// luminance instead of as an alpha channel.
fn image_has_meaningful_alpha(image: &Image) -> bool {
    let format = image.texture_descriptor.format;
    if matches!(
        format,
        TextureFormat::Rgba8Unorm | TextureFormat::Rgba8UnormSrgb
    ) && let Some(data) = image.data.as_ref()
    {
        let pixels = (image.width() as usize) * (image.height() as usize);
        if data.len() >= pixels * 4 {
            return data[..pixels * 4].chunks_exact(4).any(|p| p[3] < 255);
        }
    }

    // Fallback for other formats: sample through the color API, strided for
    // large images to keep load time bounded.
    let total = image.width() * image.height();
    if total == 0 {
        return false;
    }
    let step = (total / 4096).max(1);
    let mut index = 0u32;
    while index < total {
        let (x, y) = (index % image.width(), index / image.width());
        if let Ok(color) = image.get_color_at(x, y)
            && raw_components(color)[3] < 1.0
        {
            return true;
        }
        index += step;
    }
    false
}

/// Pack the FBX opacity mask into a base color image's alpha channel.
///
/// The result is an sRGB image whose RGB is the base color albedo (white when
/// the material has no base color texture, so the scalar `base_color` still
/// shows through) and whose alpha is the opacity mask times the scalar opacity
/// times the base color image's own alpha when that carries real transparency.
pub(super) fn compose_base_color_alpha(
    base: Option<&Image>,
    opacity: &Image,
    scalar_opacity: f32,
    usage: RenderAssetUsages,
) -> Result<Image, String> {
    let width = opacity.width();
    let height = opacity.height();
    if width == 0 || height == 0 {
        return Err("empty opacity image".to_string());
    }
    if let Some(base) = base
        && (base.width() != width || base.height() != height)
    {
        return Err(format!(
            "size mismatch (base color {}x{}, opacity {}x{})",
            base.width(),
            base.height(),
            width,
            height
        ));
    }

    let use_opacity_alpha = image_has_meaningful_alpha(opacity);
    let base_has_alpha = base.map(image_has_meaningful_alpha).unwrap_or(false);
    let scalar = scalar_opacity.clamp(0.0, 1.0);
    let size = Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let mut composed = Image::new(
        size,
        TextureDimension::D2,
        vec![255u8; (width as usize) * (height as usize) * 4],
        TextureFormat::Rgba8UnormSrgb,
        usage,
    );
    composed.sampler = match base {
        Some(base) => base.sampler.clone(),
        None => opacity.sampler.clone(),
    };

    for y in 0..height {
        for x in 0..width {
            let texel = raw_texel(opacity, x, y)?;
            let mask = if use_opacity_alpha {
                texel[3]
            } else {
                texel_luminance(texel)
            };
            let color = match base {
                Some(base) => {
                    let base = Srgba::from(base.get_color_at(x, y).map_err(|e| e.to_string())?);
                    // An authored cutout in the base color image must survive the
                    // pack: mask * scalar * base alpha.
                    let base_alpha = if base_has_alpha { base.alpha } else { 1.0 };
                    let alpha = (mask * scalar * base_alpha).clamp(0.0, 1.0);
                    Srgba::new(base.red, base.green, base.blue, alpha)
                }
                None => {
                    let alpha = (mask * scalar).clamp(0.0, 1.0);
                    Srgba::new(1.0, 1.0, 1.0, alpha)
                }
            };
            composed
                .set_color_at(x, y, Color::Srgba(color))
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(composed)
}

/// Pack separate metallic/roughness maps into the layout `StandardMaterial`
/// samples: G = roughness, B = metallic, R/A unused.
///
/// A missing channel is written as 1.0 so the material's scalar factor still
/// applies. The result is linear data (`Rgba8Unorm`).
pub(super) fn pack_metallic_roughness(
    metallic: Option<&Image>,
    roughness: Option<&Image>,
    usage: RenderAssetUsages,
) -> Result<Image, String> {
    let Some(source) = metallic.or(roughness) else {
        return Err("no metallic/roughness source image".to_string());
    };
    let width = source.width();
    let height = source.height();
    if width == 0 || height == 0 {
        return Err("empty metallic/roughness image".to_string());
    }
    for image in [metallic, roughness].into_iter().flatten() {
        if image.width() != width || image.height() != height {
            return Err(format!(
                "size mismatch ({}x{} vs {}x{})",
                image.width(),
                image.height(),
                width,
                height
            ));
        }
    }

    let size = Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let mut data = vec![255u8; (width as usize) * (height as usize) * 4];
    for y in 0..height {
        for x in 0..width {
            let index = ((y as usize) * (width as usize) + (x as usize)) * 4;
            let metallic_value = match metallic {
                Some(image) => raw_texel(image, x, y)?[0],
                None => 1.0,
            };
            let roughness_value = match roughness {
                Some(image) => raw_texel(image, x, y)?[0],
                None => 1.0,
            };
            data[index] = 255;
            data[index + 1] = encode_unorm(roughness_value);
            data[index + 2] = encode_unorm(metallic_value);
            data[index + 3] = 255;
        }
    }

    let mut packed = Image::new(
        size,
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8Unorm,
        usage,
    );
    packed.sampler = source.sampler.clone();
    Ok(packed)
}

fn encode_unorm(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: u32, height: u32, pixels: &[[u8; 4]]) -> Image {
        assert_eq!(pixels.len() as u32, width * height);
        let mut data = Vec::with_capacity(pixels.len() * 4);
        for pixel in pixels {
            data.extend_from_slice(pixel);
        }
        Image::new(
            Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            data,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        )
    }

    fn texel(image: &Image, x: u32, y: u32) -> [f32; 4] {
        raw_texel(image, x, y).expect("texel")
    }

    fn close(actual: f32, expected: f32, what: &str) {
        assert!(
            (actual - expected).abs() <= 1.5 / 255.0,
            "{what}: {actual} vs {expected}"
        );
    }

    /// Opacity-only materials pack to white RGB so the scalar `base_color` shows
    /// through, with alpha = mask * scalar.
    #[test]
    fn opacity_only_packing_uses_white_rgb_and_scaled_mask_alpha() {
        // Luminance mask: one opaque black texel, one opaque white texel.
        let opacity = image(2, 1, &[[0, 0, 0, 255], [255, 255, 255, 255]]);
        let packed = compose_base_color_alpha(None, &opacity, 0.5, RenderAssetUsages::default())
            .expect("pack");

        let dark = texel(&packed, 0, 0);
        close(dark[0], 1.0, "white R");
        close(dark[1], 1.0, "white G");
        close(dark[2], 1.0, "white B");
        close(dark[3], 0.0, "black mask texel alpha");

        let bright = texel(&packed, 1, 0);
        close(bright[3], 0.5, "white mask texel * scalar");
    }

    /// A base color image with real alpha keeps it: mask * scalar * base alpha.
    #[test]
    fn packing_multiplies_authored_base_alpha_with_the_mask_and_scalar() {
        // Base: opaque red texel and a half-transparent red texel.
        let base = image(2, 1, &[[255, 0, 0, 255], [255, 0, 0, 128]]);
        // Opacity: opaque white mask everywhere (so the base alpha is the only cutout).
        let opacity = image(2, 1, &[[255, 255, 255, 255], [255, 255, 255, 255]]);
        let packed =
            compose_base_color_alpha(Some(&base), &opacity, 0.5, RenderAssetUsages::default())
                .expect("pack");

        let opaque = texel(&packed, 0, 0);
        close(opaque[0], 1.0, "base R");
        close(opaque[3], 0.5, "opaque base alpha * scalar");

        let cutout = texel(&packed, 1, 0);
        close(cutout[3], 0.5 * (128.0 / 255.0), "base cutout * scalar");
    }

    /// Separate metallic/roughness maps land in G (roughness) and B (metallic).
    #[test]
    fn metallic_roughness_packing_writes_g_and_b() {
        let metallic = image(1, 1, &[[0, 0, 0, 255]]);
        let roughness = image(1, 1, &[[255, 255, 255, 255]]);
        let packed = pack_metallic_roughness(
            Some(&metallic),
            Some(&roughness),
            RenderAssetUsages::default(),
        )
        .expect("pack");
        assert_eq!(packed.texture_descriptor.format, TextureFormat::Rgba8Unorm);

        let pixel = texel(&packed, 0, 0);
        close(pixel[1], 1.0, "roughness in G");
        close(pixel[2], 0.0, "metallic in B");
    }

    /// A single source fills the other channel with 1.0 so the scalar factor
    /// still applies.
    #[test]
    fn single_channel_packing_fills_the_missing_channel_with_one() {
        let roughness = image(1, 1, &[[128, 128, 128, 255]]);
        let packed = pack_metallic_roughness(None, Some(&roughness), RenderAssetUsages::default())
            .expect("pack");

        let pixel = texel(&packed, 0, 0);
        close(pixel[1], 128.0 / 255.0, "roughness in G");
        close(pixel[2], 1.0, "missing metallic filled with 1.0");
    }
}
