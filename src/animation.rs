//! Bake FBX animation stacks into Bevy [`AnimationClip`]s.

use crate::error::FbxError;
use crate::label::FbxAssetLabel;
use crate::names::animation_name_path;
use bevy::animation::prelude::AnimatableCurve;
use bevy::animation::{animated_field, AnimationClip, AnimationTargetId};
use bevy::asset::{Handle, LoadContext};
use bevy::math::curve::{ConstantCurve, Interval, UnevenSampleAutoCurve};
use bevy::prelude::*;
use std::collections::HashMap;

/// Result of animation baking.
pub struct ProcessedAnimations {
    pub animations: Vec<Handle<AnimationClip>>,
    pub named_animations: HashMap<Box<str>, Handle<AnimationClip>>,
}

/// Bake each FBX anim stack into a labeled [`AnimationClip`].
pub fn process_animations(
    scene: &ufbx::Scene,
    load_context: &mut LoadContext,
) -> Result<ProcessedAnimations, FbxError> {
    let mut animations = Vec::new();
    let mut named_animations = HashMap::new();

    for (stack_index, stack) in scene.anim_stacks.as_ref().iter().enumerate() {
        let take_name = if stack.element.name.is_empty() {
            format!("take{stack_index}")
        } else {
            stack.element.name.to_string()
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

            if baked_node.constant_translation && baked_node.translation_keys.len() == 1 {
                let v = baked_node.translation_keys[0].value;
                let translation = Vec3::new(v.x as f32, v.y as f32, v.z as f32);
                clip.add_curve_to_target(
                    target_id,
                    AnimatableCurve::new(
                        animated_field!(Transform::translation),
                        ConstantCurve::new(Interval::EVERYWHERE, translation),
                    ),
                );
            } else if baked_node.translation_keys.len() == 1 {
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
                    (
                        k.time as f32,
                        Vec3::new(v.x as f32, v.y as f32, v.z as f32),
                    )
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
                let rotation = Quat::from_xyzw(v.x as f32, v.y as f32, v.z as f32, v.w as f32);
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
                        Quat::from_xyzw(v.x as f32, v.y as f32, v.z as f32, v.w as f32),
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

        let handle = load_context
            .add_labeled_asset(FbxAssetLabel::Animation(stack_index).to_string(), clip);
        named_animations.insert(Box::from(take_name.as_str()), handle.clone());
        animations.push(handle);
    }

    Ok(ProcessedAnimations {
        animations,
        named_animations,
    })
}
