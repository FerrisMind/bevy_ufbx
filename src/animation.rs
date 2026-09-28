//! Bake FBX animation stacks into Bevy [`AnimationClip`]s.

use crate::error::FbxError;
use crate::label::FbxAssetLabel;
use crate::names::{animation_name_path, node_typed_id};
use bevy::animation::animation_curves::WeightsCurve;
use bevy::animation::gltf_curves::WideLinearKeyframeCurve;
use bevy::animation::prelude::AnimatableCurve;
use bevy::animation::{AnimationClip, AnimationTargetId, animated_field};
use bevy::asset::{Handle, LoadContext};
use bevy::math::curve::{ConstantCurve, Interval, UnevenSampleAutoCurve};
use bevy::mesh::morph::MAX_MORPH_WEIGHTS;
use bevy::prelude::*;
use std::collections::HashMap;

/// Result of animation baking.
pub struct ProcessedAnimations {
    pub animations: Vec<Handle<AnimationClip>>,
    pub named_animations: HashMap<Box<str>, Handle<AnimationClip>>,
}

/// Bake each FBX anim stack into a labeled [`AnimationClip`].
///
/// Clips are labeled `Animation{N}` only. Bevy's [`LoadContext`] does not support
/// registering a second label for the same asset, so stack names are exposed via
/// [`ProcessedAnimations::named_animations`] (and [`crate::Fbx::named_animations`])
/// rather than as `AssetPath` labels.
pub fn process_animations(
    scene: &ufbx::Scene,
    load_context: &mut LoadContext,
) -> Result<ProcessedAnimations, FbxError> {
    let mut animations = Vec::new();
    let mut named_animations = HashMap::new();

    for (stack_index, stack) in scene.anim_stacks.as_ref().iter().enumerate() {
        // Trim so keys match DCC take names like "Take 001" without stray whitespace.
        let take_name = {
            let raw = stack.element.name.trim();
            if raw.is_empty() {
                format!("take{stack_index}")
            } else {
                raw.to_string()
            }
        };

        let baked = ufbx::bake_anim(
            scene,
            &stack.anim,
            ufbx::BakeOpts {
                trim_start_time: true,
                ..Default::default()
            },
        )
        .map_err(|e| {
            FbxError::ConversionError(format!(
                "Failed to bake animation stack '{take_name}': {e:?}"
            ))
        })?;

        let mut clip = AnimationClip::default();
        clip.set_duration(baked.playback_duration as f32);

        for baked_node in baked.nodes.as_ref().iter() {
            let path = animation_name_path(scene, baked_node.typed_id);
            if path.is_empty() {
                continue;
            }
            let target_id = AnimationTargetId::from_names(path.iter());
            add_trs_curves(&mut clip, target_id, baked_node);
        }

        add_morph_weight_curves(scene, &stack.anim, &baked, &mut clip);

        let handle =
            load_context.add_labeled_asset(FbxAssetLabel::Animation(stack_index).to_string(), clip);
        named_animations.insert(Box::from(take_name.as_str()), handle.clone());
        animations.push(handle);
    }

    Ok(ProcessedAnimations {
        animations,
        named_animations,
    })
}

fn add_trs_curves(
    clip: &mut AnimationClip,
    target_id: AnimationTargetId,
    baked_node: &ufbx::BakedNode,
) {
    if baked_node.translation_keys.len() == 1 {
        let v = baked_node.translation_keys[0].value;
        let translation = Vec3::new(v.x as f32, v.y as f32, v.z as f32);
        clip.add_curve_to_target(
            target_id,
            AnimatableCurve::new(
                animated_field!(Transform::translation),
                ConstantCurve::new(Interval::EVERYWHERE, translation),
            ),
        );
    } else if baked_node.translation_keys.len() >= 2 {
        let samples = baked_node.translation_keys.iter().map(|k| {
            let v = k.value;
            (k.time as f32, Vec3::new(v.x as f32, v.y as f32, v.z as f32))
        });
        if let Ok(curve) = UnevenSampleAutoCurve::new(samples) {
            clip.add_curve_to_target(
                target_id,
                AnimatableCurve::new(animated_field!(Transform::translation), curve),
            );
        }
    }

    if baked_node.rotation_keys.len() == 1 {
        let v = baked_node.rotation_keys[0].value;
        let rotation = Quat::from_xyzw(v.x as f32, v.y as f32, v.z as f32, v.w as f32).normalize();
        clip.add_curve_to_target(
            target_id,
            AnimatableCurve::new(
                animated_field!(Transform::rotation),
                ConstantCurve::new(Interval::EVERYWHERE, rotation),
            ),
        );
    } else if baked_node.rotation_keys.len() >= 2 {
        let samples = baked_node.rotation_keys.iter().map(|k| {
            let v = k.value;
            (
                k.time as f32,
                Quat::from_xyzw(v.x as f32, v.y as f32, v.z as f32, v.w as f32).normalize(),
            )
        });
        if let Ok(curve) = UnevenSampleAutoCurve::new(samples) {
            clip.add_curve_to_target(
                target_id,
                AnimatableCurve::new(animated_field!(Transform::rotation), curve),
            );
        }
    }

    if baked_node.scale_keys.len() == 1 {
        let v = baked_node.scale_keys[0].value;
        let scale = Vec3::new(v.x as f32, v.y as f32, v.z as f32);
        clip.add_curve_to_target(
            target_id,
            AnimatableCurve::new(
                animated_field!(Transform::scale),
                ConstantCurve::new(Interval::EVERYWHERE, scale),
            ),
        );
    } else if baked_node.scale_keys.len() >= 2 {
        let samples = baked_node.scale_keys.iter().map(|k| {
            let v = k.value;
            (k.time as f32, Vec3::new(v.x as f32, v.y as f32, v.z as f32))
        });
        if let Ok(curve) = UnevenSampleAutoCurve::new(samples) {
            clip.add_curve_to_target(
                target_id,
                AnimatableCurve::new(animated_field!(Transform::scale), curve),
            );
        }
    }
}

/// Sample blend-channel weights into a [`WeightsCurve`] on each mesh node path.
fn add_morph_weight_curves(
    scene: &ufbx::Scene,
    anim: &ufbx::Anim,
    baked: &ufbx::BakedAnim,
    clip: &mut AnimationClip,
) {
    let duration = baked.playback_duration as f32;
    if duration < 0.0 {
        return;
    }

    for node in scene.nodes.as_ref().iter() {
        if node.is_root {
            continue;
        }
        let Some(mesh_ref) = node.mesh.as_ref() else {
            continue;
        };
        let mesh = mesh_ref.as_ref();
        if mesh.blend_deformers.is_empty() {
            continue;
        }

        let mut channels: Vec<&ufbx::BlendChannel> = Vec::new();
        for deformer in mesh.blend_deformers.as_ref().iter() {
            for channel in deformer.channels.as_ref().iter() {
                if channel.target_shape.is_none() {
                    continue;
                }
                if channels.len() >= MAX_MORPH_WEIGHTS {
                    break;
                }
                channels.push(channel);
            }
        }
        if channels.is_empty() {
            continue;
        }

        let path = animation_name_path(scene, node_typed_id(node));
        if path.is_empty() {
            continue;
        }
        let target_id = AnimationTargetId::from_names(path.iter());

        let sample_count = ((duration * 30.0).ceil() as usize).clamp(1, 256);
        let mut times = Vec::with_capacity(sample_count + 1);
        let mut values = Vec::with_capacity((sample_count + 1) * channels.len());

        for i in 0..=sample_count {
            let t = if sample_count == 0 {
                0.0
            } else {
                duration * (i as f32) / sample_count as f32
            };
            let ufbx_t = baked.playback_time_begin + f64::from(t);
            times.push(t);
            for channel in &channels {
                values.push(ufbx::evaluate_blend_weight(anim, channel, ufbx_t) as f32);
            }
        }

        if times.len() == 1 {
            clip.add_curve_to_target(
                target_id,
                WeightsCurve(ConstantCurve::new(Interval::EVERYWHERE, values)),
            );
        } else if let Ok(curve) = WideLinearKeyframeCurve::new(times, values) {
            clip.add_curve_to_target(target_id, WeightsCurve(curve));
        }
    }
}
