//! Bake FBX animation stacks into Bevy [`AnimationClip`]s.

use crate::error::FbxError;
use crate::label::FbxAssetLabel;
use crate::names::{animation_name_path, node_typed_id};
use crate::utils::convert_transform;
use bevy::animation::animation_curves::WeightsCurve;
use bevy::animation::gltf_curves::WideLinearKeyframeCurve;
use bevy::animation::prelude::AnimatableCurve;
use bevy::animation::{AnimationClip, AnimationTargetId, animated_field};
use bevy::asset::{Handle, LoadContext};
use bevy::math::curve::{ConstantCurve, Interval, UnevenSampleAutoCurve};
use bevy::mesh::morph::MAX_MORPH_WEIGHTS;
use bevy::prelude::*;
use std::collections::{HashMap, HashSet};

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
///
/// When `generate_rest` is true, an additional clip labeled `AnimationRest` /
/// named `"Rest"` is appended with a single keyframe at `t = 0` for each node
/// that appeared in any baked stack (rest local transform + morph weights).
pub fn process_animations(
    scene: &ufbx::Scene,
    load_context: &mut LoadContext,
    bake_fps: f32,
    generate_rest: bool,
) -> Result<ProcessedAnimations, FbxError> {
    let mut animations = Vec::new();
    let mut named_animations = HashMap::new();
    let mut animated_typed_ids = HashSet::new();
    let resample_rate = f64::from(bake_fps.max(0.0));

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
                resample_rate,
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
            animated_typed_ids.insert(baked_node.typed_id);
            let path = animation_name_path(scene, baked_node.typed_id);
            if path.is_empty() {
                continue;
            }
            let target_id = AnimationTargetId::from_names(path.iter());
            add_trs_curves(&mut clip, target_id, baked_node);
        }

        add_morph_weight_curves(scene, &stack.anim, &baked, &mut clip, bake_fps);

        let handle =
            load_context.add_labeled_asset(FbxAssetLabel::Animation(stack_index).to_string(), clip);
        named_animations.insert(Box::from(take_name.as_str()), handle.clone());
        animations.push(handle);
    }

    if generate_rest {
        let rest_handle =
            add_rest_animation_clip(scene, &animated_typed_ids, load_context);
        named_animations.insert(Box::from("Rest"), rest_handle.clone());
        animations.push(rest_handle);
    }

    Ok(ProcessedAnimations {
        animations,
        named_animations,
    })
}

/// Rest/bind clip: constant TRS (+ morph weights) at `t = 0` for animated nodes.
fn add_rest_animation_clip(
    scene: &ufbx::Scene,
    animated_typed_ids: &HashSet<u32>,
    load_context: &mut LoadContext,
) -> Handle<AnimationClip> {
    let mut clip = AnimationClip::default();
    clip.set_duration(0.0);

    let mut typed_ids: Vec<u32> = animated_typed_ids.iter().copied().collect();
    typed_ids.sort_unstable();

    for typed_id in typed_ids {
        let Some(node) = scene.nodes.get(typed_id as usize) else {
            continue;
        };
        if node.is_root {
            continue;
        }
        let path = animation_name_path(scene, typed_id);
        if path.is_empty() {
            continue;
        }
        let target_id = AnimationTargetId::from_names(path.iter());
        let transform = convert_transform(&node.local_transform);
        add_constant_trs(&mut clip, target_id, &transform);
        add_rest_morph_weights(node, target_id, &mut clip);
    }

    load_context.add_labeled_asset(FbxAssetLabel::AnimationRest.to_string(), clip)
}

fn add_constant_trs(clip: &mut AnimationClip, target_id: AnimationTargetId, transform: &Transform) {
    clip.add_curve_to_target(
        target_id,
        AnimatableCurve::new(
            animated_field!(Transform::translation),
            ConstantCurve::new(Interval::EVERYWHERE, transform.translation),
        ),
    );
    clip.add_curve_to_target(
        target_id,
        AnimatableCurve::new(
            animated_field!(Transform::rotation),
            ConstantCurve::new(Interval::EVERYWHERE, transform.rotation.normalize()),
        ),
    );
    clip.add_curve_to_target(
        target_id,
        AnimatableCurve::new(
            animated_field!(Transform::scale),
            ConstantCurve::new(Interval::EVERYWHERE, transform.scale),
        ),
    );
}

fn add_rest_morph_weights(
    node: &ufbx::Node,
    target_id: AnimationTargetId,
    clip: &mut AnimationClip,
) {
    let Some(mesh_ref) = node.mesh.as_ref() else {
        return;
    };
    let mesh = mesh_ref.as_ref();
    if mesh.blend_deformers.is_empty() {
        return;
    }

    let mut weights: Vec<f32> = Vec::new();
    for deformer in mesh.blend_deformers.as_ref().iter() {
        for channel in deformer.channels.as_ref().iter() {
            if channel.target_shape.is_none() {
                continue;
            }
            if weights.len() >= MAX_MORPH_WEIGHTS {
                break;
            }
            weights.push(channel.weight as f32);
        }
    }
    if weights.is_empty() {
        return;
    }

    clip.add_curve_to_target(
        target_id,
        WeightsCurve(ConstantCurve::new(Interval::EVERYWHERE, weights)),
    );
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
    bake_fps: f32,
) {
    let duration = baked.playback_duration as f32;
    if duration < 0.0 {
        return;
    }

    let fps = bake_fps.max(1.0);

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

        let sample_count = ((duration * fps).ceil() as usize).clamp(1, 256);
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
