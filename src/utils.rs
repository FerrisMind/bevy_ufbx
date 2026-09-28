//! Utility functions for converting between ufbx and Bevy types.

use crate::types::FbxExtras;
use bevy::math::{Affine2, Mat4};
use bevy::prelude::*;

/// Convert ufbx texture UV transform to Bevy Affine2.
pub fn convert_texture_uv_transform(texture: &ufbx::Texture) -> Affine2 {
    let translation = Vec2::new(
        texture.uv_transform.translation.x as f32,
        texture.uv_transform.translation.y as f32,
    );
    let scale = Vec2::new(
        texture.uv_transform.scale.x as f32,
        texture.uv_transform.scale.y as f32,
    );
    let rotation_z = texture.uv_transform.rotation.z as f32;
    Affine2::from_scale_angle_translation(scale, rotation_z, translation)
}

/// Convert ufbx matrix to Bevy Mat4.
pub fn convert_matrix(m: &ufbx::Matrix) -> Mat4 {
    Mat4::from_cols_array(&[
        m.m00 as f32,
        m.m10 as f32,
        m.m20 as f32,
        0.0,
        m.m01 as f32,
        m.m11 as f32,
        m.m21 as f32,
        0.0,
        m.m02 as f32,
        m.m12 as f32,
        m.m22 as f32,
        0.0,
        m.m03 as f32,
        m.m13 as f32,
        m.m23 as f32,
        1.0,
    ])
}

/// Convert ufbx transform to Bevy Transform.
pub fn convert_transform(t: &ufbx::Transform) -> Transform {
    Transform {
        translation: Vec3::new(
            t.translation.x as f32,
            t.translation.y as f32,
            t.translation.z as f32,
        ),
        rotation: Quat::from_xyzw(
            t.rotation.x as f32,
            t.rotation.y as f32,
            t.rotation.z as f32,
            t.rotation.w as f32,
        ),
        scale: Vec3::new(t.scale.x as f32, t.scale.y as f32, t.scale.z as f32),
    }
}

/// Flatten ufbx **user-defined** custom properties into an [`FbxExtras`] blob
/// (glTF-style). Built-in / synthetic props are skipped.
pub(crate) fn props_to_extras(props: &ufbx::Props) -> Option<FbxExtras> {
    if props.props.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    for prop in props.props.as_ref().iter() {
        if prop.name.is_empty() {
            continue;
        }
        if !prop.flags.has_any(ufbx::PropFlags::USER_DEFINED) {
            continue;
        }
        let value = if !prop.value_str.is_empty() {
            prop.value_str.to_string()
        } else {
            format!(
                "({},{},{},{})",
                prop.value_vec4.x, prop.value_vec4.y, prop.value_vec4.z, prop.value_vec4.w
            )
        };
        parts.push(format!("{}={}", prop.name, value));
    }
    if parts.is_empty() {
        None
    } else {
        Some(FbxExtras {
            value: parts.join(";"),
        })
    }
}
