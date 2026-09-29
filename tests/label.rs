//! Tests for FBX asset labels.

use bevy_ufbx::FbxAssetLabel;

#[test]
fn test_scene_label() {
    let label = FbxAssetLabel::Scene(0);
    assert_eq!(label.to_string(), "Scene0");

    let label = FbxAssetLabel::Scene(42);
    assert_eq!(label.to_string(), "Scene42");
}

#[test]
fn test_node_label() {
    let label = FbxAssetLabel::Node(5);
    assert_eq!(label.to_string(), "Node5");

    let label = FbxAssetLabel::Node(999);
    assert_eq!(label.to_string(), "Node999");
}

#[test]
fn test_mesh_label() {
    let label = FbxAssetLabel::Mesh(10);
    assert_eq!(label.to_string(), "Mesh10");
}

#[test]
fn test_primitive_label() {
    let label = FbxAssetLabel::Primitive {
        mesh: 2,
        primitive: 1,
    };
    assert_eq!(label.to_string(), "Mesh2/Primitive1");
}

#[test]
fn test_material_inverted_label() {
    let label = FbxAssetLabel::MaterialInverted(3);
    assert_eq!(label.to_string(), "Material3 (inverted)");
}

#[test]
fn test_material_standard_label() {
    let label = FbxAssetLabel::MaterialStandard(3);
    assert_eq!(label.to_string(), "Material3/Standard");
}

#[test]
fn test_texture_label() {
    let label = FbxAssetLabel::Texture(7);
    assert_eq!(label.to_string(), "Texture7");
}

#[test]
fn test_animation_label() {
    let label = FbxAssetLabel::Animation(2);
    assert_eq!(label.to_string(), "Animation2");
}

#[test]
fn test_animation_rest_label() {
    let label = FbxAssetLabel::AnimationRest;
    assert_eq!(label.to_string(), "AnimationRest");
}

#[test]
fn test_inverse_bind_matrices_label() {
    let label = FbxAssetLabel::InverseBindMatrices(3);
    assert_eq!(label.to_string(), "Skin3/InverseBindMatrices");
}

#[test]
fn test_default_material_label() {
    let label = FbxAssetLabel::DefaultMaterial;
    assert_eq!(label.to_string(), "DefaultMaterial");
}

#[test]
fn test_label_uniqueness() {
    // Test that different label types with same index produce different strings
    let scene = FbxAssetLabel::Scene(1);
    let node = FbxAssetLabel::Node(1);
    let mesh = FbxAssetLabel::Mesh(1);

    assert_ne!(scene.to_string(), node.to_string());
    assert_ne!(scene.to_string(), mesh.to_string());
    assert_ne!(node.to_string(), mesh.to_string());
}

#[test]
fn test_label_consistency() {
    // Test that the same label always produces the same string
    let label1 = FbxAssetLabel::Material(100);
    let label2 = FbxAssetLabel::Material(100);

    assert_eq!(label1.to_string(), label2.to_string());
}
