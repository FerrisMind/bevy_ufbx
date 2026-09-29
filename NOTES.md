# bevy_ufbx NOTES

Forked loader in `tir_refs/fbx` aiming at **Bevy glTF-class FBX parity** (Bevy 0.19 + ufbx 0.9).

## Parity bar

Match what `bevy_gltf` exposes at runtime, fed by ufbx — **not** 1:1 Autodesk FBX fidelity. There is no
complete Autodesk FBX solver here: no live IK/constraint/cache-deformer solving, no dual-quaternion skinning,
no character presets, no audio. Anything Bevy cannot consume at runtime is either baked (`bake_anim` TRS),
tessellated (NURBS), approximated (DQuat → LBS), or warned about and skipped. Docs (README/NOTES) state
current semantics honestly, including remaining correctness gaps.

## Implemented

- Hierarchy `WorldAsset`, skins + IBM, bake_anim TRS + morph `WeightsCurve`
- Mesh attrs: color, tangent with derived bitangent-handedness sign (or generated), UV0/UV1
- Shared mesh instances: same ufbx mesh `element_id` on multiple nodes reuses one `FbxMesh` / primitives
- Blend shapes → morph targets + `MorphWeights` / `MeshMorphWeights`
- Embedded + **pre-read external** textures (async `read_asset_bytes` before material processing; `load_external_files: false` disables ufbx unmanaged C I/O); wrap modes; richer PBR factors (coat/transmission/IOR)
- NURBS → tessellate to `Mesh`
- Constraint / non-linear skinning: warn and continue
- Skin weights from ufbx per-vertex tables (`skin_deformer.vertices[]/weights[]`): top-4, influence-sorted,
  renormalized; a bad cluster warns and is skipped instead of failing the load (see Skin section above).
  Public helper API preserved through review: top-4 weights keeps its 2 args; `bind_unweighted_vertex`
  public helper applies a slot-0 fallback with a warning.
- Animation: stepped keys → holds, antipodal rotation continuity, failed curves warn with node path + track
- Spot lights: FBX full-aperture degrees → Bevy half-angle radians (missing → 45°, clamp 160°), unit-tested
- `TransformRoot` space conversion spawns a synthetic-root wrapper entity so top-level nodes keep the requested conversion (wrapper excluded from animation name paths)
- Real-loader corpus sweep test (`UFBX_TEST_DATA`, hostile files isolated) + parity contract tests
- Honest `axis_system` / `unit_scale` when `convert_coordinates: false`
- `FbxExtras` from **USER_DEFINED** custom props only (built-in / synthetic props skipped)
- `FbxMeshName` components
- `FbxMesh` container asset (glTF-style): `Mesh{N}` labels `FbxMesh`; Bevy meshes are `Mesh{N}/Primitive{P}`
- `FbxPrimitive` on `FbxMesh.primitives` (mesh + optional `StandardMaterial` + extras); `Fbx.primitive_meshes` flat Bevy [`Mesh`] list; `FbxNode.mesh` is `Option<Handle<FbxMesh>>`
- `FbxNode`: `children` + `skin` handles filled (two-pass label reservation, glTF-style)
- `FbxMaterial` container asset (glTF-style): `Material{N}` → `FbxMaterial` (`.material` is `StandardMaterial`); inverted twins stay `Material{N} (inverted)` as bare `StandardMaterial`
- World **neg-scale**: odd count of negative axes on **world** scale (not local `sx*sy*sz`) selects `Material{N} (inverted)` cull twin (skips double-sided)
- Texture samplers: FBX `wrap_u` / `wrap_v` → address modes; mag/min/mip from `FbxLoaderSettings::default_sampler` (or full replace via `override_sampler`) — ufbx 0.9 has no filter fields
- Blender PBR: `use_blender_pbr_material` when probe exporter is Blender binary/ascii
- Anim takes: `Animation{N}` labels + `Fbx.named_animations["Take 001"]` map (no second name label)
- Dual-published texture color space labels: `Texture{N}` (sRGB) / `Texture{N}/Linear`; packed labels
  `Material{i}/BaseColorOpacity` + `Material{i}/MetallicRoughness`
- `bake_fps` (default 30) → `BakeOpts::resample_rate` + morph sampling; optional `generate_rest_animation` → `AnimationRest` / `"Rest"`
- `--no-default-features` lib build: CI job enforces it with `RUSTFLAGS: "-C debuginfo=0 -D warnings"` +
  `cargo check --lib --no-default-features` (plain cargo args — warning enforcement via env, keeping the
  shared `debuginfo=0` flag; node.rs/scene.rs cfg-gating landed by L2/L4)

## Skin inverse bind matrices (canonical `geometry_to_bone`)

`src/node.rs` reads the inverse bind matrix **directly from `cluster.geometry_to_bone`** — ufbx documents it
as the transform from mesh geometry space to local bone space, "often referred to as the inverse bind matrix"
([ufbx deformers guide](https://ufbx.github.io/elements/deformers/)).

Correction history (do not reintroduce the old form): the loader previously computed
`bind_to_world⁻¹ · geometry_to_world` on the assumption that ufbx sets
`geometry_to_world = bind_to_world · geometry_to_bone`. ufbx actually computes
`geometry_to_world = bone_node.node_to_world · geometry_to_bone` (`ufbx.c`, `ufbxi_update_skin_cluster`) —
with the bone's **load-time** pose, not the bind pose. The composed form therefore diverges from the true IBM
whenever a file's load pose differs from its bind pose (L2 corpus: up to 2.97 relative error) and can produce
non-finite matrices for singular bind poses. Reading `cluster.geometry_to_bone` directly is a **correctness
semantic fix**, not a cosmetic simplification.

Skin weights come from ufbx's per-vertex tables (`skin_deformer.vertices[]` / `weights[]`), sorted by
decreasing influence, first 4 taken and renormalized — the recipe documented on the same deformers page.

## Texture semantics (current)

- Slot bindings resolve from ufbx's resolved `MaterialMap.texture` (`pbr.base_color`, `pbr.opacity`,
  `pbr.emission_color`, `pbr.normal_map`, `pbr.metalness`, `pbr.roughness`, `pbr.ambient_occlusion`), with the
  FBX property-name match as fallback. Color slots (base color, emissive, opacity) decode sRGB; data slots
  (normal, metallic, roughness, AO, height) decode linear — `bevy_gltf` parity. A map feeding both kinds of
  slots is dual-published: `Texture{N}` (sRGB) and `Texture{N}/Linear`.
- Embedded **and pre-read external** images are packed at load time: an opacity **luminance mask** is written
  into the base-color alpha (`Material{i}/BaseColorOpacity`, so `AlphaMode::Mask` works without graying the
  albedo); separate metallic/roughness maps are packed to the glTF layout (`Material{i}/MetallicRoughness`,
  G = roughness, B = metallic). Format detection sniffs magic bytes (PNG/JPEG/DDS/WebP) before trusting the
  filename extension. Verified against on-disk checkerboard PNGs (`blender_293_textures_7400_binary.fbx`):
  sRGB base == diffuse PNG, packed G/B == roughness/metallic PNGs (targeted tests 5/5 on the current tree).
- **Runtime external reads**: the loader pre-reads external texture bytes before processing — send-safe owned
  `(texture id, candidate paths)` requests, scene dropped, `read_asset_bytes().await`, re-parse, then sync
  processing. `load_external_files: false` on all three ufbx parses (probe, main, …): no unmanaged C file
  I/O, so geometry-cache / `.mtl` side-loads never happen. All per-slot color space, UV selection, and
  packing applies **equally to external and embedded**. Caveats: absolute DCC paths reduce to the adjacent
  basename or `.fbm` folder; escaping reads never happen (candidates clamped); missing bytes warn and omit
  packing (no wrong-channel binding). The synchronous compatibility entry (`process_materials` without
  pre-read bytes) remains limited to embedded content.
- Per-map UV channel (`*_channel: UvChannel`) is selected from the texture's `uv_set`. Bevy 0.19 supports
  **one** `uv_transform` per `StandardMaterial`, so per-map transforms collapse to the base-color transform.
- Compressed embedded containers (DDS/KTX2) additionally need the matching Bevy image features enabled
  (the default feature set ships PNG and JPEG decoders).
- Packing skips when the maps to combine have mismatched UV wrap modes (one sampler cannot satisfy both).
  The opacity luminance mask is packed from **raw** luminance bytes — authored values are preserved, no
  color management applied at pack time (the mask slot itself decodes sRGB when displayed standalone).
  Alpha mode: scalar opacity factor < 0.98 → `AlphaMode::Blend` (wins over the texture cutout heuristic);
  texture-only opacity → `AlphaMode::Mask(0.5)`.

## Engine-impossible / bake-only

| FBX feature | Behavior |
|-------------|----------|
| Constraints / IK | Warning; bake TRS in DCC / bake_anim |
| Dual-quaternion skin | Warning; LBS approximation |
| Native NURBS | Tessellate only |
| Stereo / character / audio | Ignored |

## Known limitations (current)

- **Untrusted input**: ufbx is a native parser; malformed FBX data can abort the host process. The loader is
  built for DCC-exported assets — **no claim of full support for arbitrary/hostile files** (the corpus test
  isolates hostile files from the test process; the verified corpus sweep still contains 2 files that crash
  the native parser).
- **One `uv_transform` per material** (Bevy 0.19 runtime constraint); per-map **channel** selection is
  supported via `*_channel`. Color space, UV selection, and packing apply equally to external and embedded
  textures (see Texture semantics for the external-read caveats).
- **Duplicate node/mesh names** overwrite each other in `named_*` maps and can collide `AnimationTargetId`
  paths (bevy_gltf has the identical limitation).
- **Light conversion** uses fixed intensity multipliers (directional ×10000, point/spot ×1000) — heuristics,
  not physical units; area lights become point lights. Spot cones are converted correctly (FBX full-aperture
  degrees → Bevy half-angle radians; missing → 45°, clamped to 160°).
- **Emission color** is treated as linear.
- **GPU instancing**: dedicated FBX instance metadata is not imported — **missing loader work, not an engine
  limitation**. Bevy already auto-batches and shares mesh assets; shared ufbx mesh `element_id`s reuse one
  `FbxMesh` today.
- **Verification**: full corpus sweep verified against `target/l5-corpus/final-sweep.log` — **714 files:
  702 loaded, 10 expected, 2 hostile (isolated subprocesses; ufbx native terminations 0xC0000409 and
  0xC0000005), 0 unexpected, 0 timeouts, 0 panics** (wall 11.0 s). ID-collision empty-stack loader bug
  fixed; collision case 637 ms. Fresh full `cargo test` exit 0: **140 passed / 0 failed** (26 lib +
  114 integration, incl. 3 corpus tests; doc-tests 0). Examples check exit 0; feature-off lib check
  (incl. JPEG) and feature-off rustdoc exit 0 clean.

## LoadOpts (when `convert_coordinates`)

RH Y-up, metres. Default [`FbxSpaceConversion::Auto`]: probe exporter with `ignore_all_content`, then
- Blender binary/ascii → `AdjustTransforms` (undo export root ×100)
- otherwise (Maya/cm, Mixamo, unknown) → `ModifyGeometry` (bake units into verts; clean node scales)

Maya centimetre fixtures correctly become ~1 cm in world metres — examples may apply a **demo** root scale for readability.

## Fixtures

Local `assets/` (visual examples + tests):

| File | Proves |
|------|--------|
| `blend_shape_cube.fbx` | morph / blend shapes (`morph_fbx`) |
| `cube_anim.fbx` / `cube_anim_bevy.fbx` | TRS bake (`animated_mesh_fbx`) |
| `cube.fbx` | static mesh / CLI default (`load_fbx`, `static_mesh_fbx` fallback) |
| `maya_cube_7400_binary.fbx` | Maya cm cube |
| `nurbs_saddle.fbx` | NURBS tessellate (`nurbs_fbx`) |
| `rigged_triangle.fbx` | skin IBM / fallback for `skinned_mesh_fbx` |
| `blender_279_sausage_7400_binary.fbx` | skinned character-ish + takes (`skinned_mesh_fbx`, `showcase_fbx`) |
| `blender_279_internal_textures_7400_binary.fbx` | embedded textures (`textures_fbx`) |
| `blender_282_suzanne_7400_binary.fbx` | Blender AdjustTransforms static mesh |
| `blender_279_nested_meshes_7400_binary.fbx` | nested mesh hierarchy (`nested_meshes_fbx`) |
| `blender_suzanne_multimaterial_7400_binary.fbx` | material-split primitives / color regions (`multimaterial_fbx`, `showcase_fbx`) |
| `maya_camera_light_axes_y_up_6100_binary.fbx` | lights + inactive camera (no mesh; `lights_cameras_fbx`) |
| `zbrush_vertex_color_7500_ascii.fbx` | vertex colors / `ATTRIBUTE_COLOR` (`vertex_color_fbx`) |
| `blender_340_mirrored_normals_7400_binary.fbx` | neg world scale → inverted cull (`neg_scale_fbx`) |
| `blender340_tangent_sign_7400_binary.fbx` | tangent `w` signs (21 020 bytes, md5 `e8898c0049dbd17c8b621a9d31add642`, verbatim copy of `libs/ufbx/data/…`) |
| `maya_anim_linear_7700_ascii.fbx` | stepped keys → holds (17 157 bytes, md5 `4bcfe71710dbb66f3cba44b729faa723`, verified identical to `libs/ufbx/data/…`) |
| `motionbuilder_lights_7700_ascii.fbx` | real spot-light instances (only corpus file with them; 95 940 bytes, md5 `062f87d244539370d37e79ccec9d8762`, copy of `libs/ufbx/data/…`) |

Copied from `libs/ufbx/data/` where noted; do not invent fixtures.

## Non-goals

- Forking / upstreaming into bevyengine
- Full T.I.R. game character wiring
