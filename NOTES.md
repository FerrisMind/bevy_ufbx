# bevy_ufbx NOTES

Working notes for the forked loader in `tir_refs/fbx` (glTF character parity on Bevy 0.19).

## Done (parity target)

- Hierarchy `WorldAsset` with parented `Name`d nodes and geometry offsets
- `SkinnedMesh` + IBM label `Skin{i}/InverseBindMatrices`
- IBM formula: `inverse(cluster.bind_to_world) * cluster.geometry_to_world`
- Joint / vertex weight indices follow cluster order; top-4 by magnitude, renormalized
- `ufbx::bake_anim` → labeled `AnimationClip`s (`Animation{i}`) + `Fbx.named_animations`
- Path-root policy A: animation name paths start under the synthetic ufbx root (Mixamo-friendly)
- `AnimationPlayer` on animation roots; `AnimationTarget` + `AnimatedBy` on the path
- Materials, lights, inactive cameras; `load_animations` / `convert_coordinates` / `include_source`
- Fixtures: `assets/cube_anim.fbx`, `assets/rigged_triangle.fbx`
- Example: `examples/animated_mesh_fbx.rs` (app builds `AnimationGraph`; loader does not auto-play)

## Non-goals

- Morph / blend shapes
- NURBS / subdivision surfaces (ufbx may triangulate some cases; not a dedicated path)
- Landing this as a bevyengine upstream PR
- Full T.I.R. game wiring (character controller, animation graphs for combat, etc.)
- Raw-curve FBX fallback that binds every curve to the first named node (rejected; broken)

## LoadOpts contract

When `convert_coordinates` is true (default):

- RH Y-up, metres (`target_unit_meters: 1.0`)
- `SpaceConversion::AdjustTransforms`
- `GeometryTransformHandling::HelperNodes`
- `InheritModeHandling::Compensate`

## Remaining caveats

- Delivery / courier fees and game-side retargeting are out of scope
- Very large skins (>256 joints) are truncated with a warning
- `FbxMeta` fills creator when present; other metadata fields stay optional stubs
- Coordinate conversion off (`convert_coordinates: false`) uses bare ufbx defaults — axis systems may not match Bevy
