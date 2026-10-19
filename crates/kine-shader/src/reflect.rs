//! Reflection over a validated naga module: what the engine must bind.

use crate::ShaderError;

use naga::AddressSpace;

/// Shader stage of an entry point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum ShaderStage {
    Vertex,
    Fragment,
    Compute,
}

/// An entry point the engine can bind.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EntryPoint {
    pub name: String,
    pub stage: ShaderStage,
    /// `@workgroup_size`, for compute entry points.
    pub workgroup_size: Option<[u32; 3]>,
}

/// WGSL type of one uniform member, as far as uniform packing cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum UniformType {
    F32,
    F64,
    I32,
    U32,
    Vec2,
    Vec3,
    Vec4,
    Mat3,
    Mat4,
    Array,
    Other,
}

/// One field of a uniform struct.
#[derive(Debug, Clone, serde::Serialize)]
pub struct UniformMember {
    pub name: String,
    /// Byte offset inside the buffer (computed by naga per WGSL layout rules).
    pub offset: u32,
    pub size: u32,
    pub ty: UniformType,
}

/// A uniform buffer binding.
#[derive(Debug, Clone, serde::Serialize)]
pub struct UniformBinding {
    pub group: u32,
    pub binding: u32,
    pub name: String,
    /// Buffer size to allocate.
    pub size: u32,
    pub members: Vec<UniformMember>,
}

/// A sampled texture binding.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TextureBinding {
    pub group: u32,
    pub binding: u32,
    pub name: String,
    pub dimension: String,
    pub sample_type: String,
    /// True for the engine-provided scene input.
    pub engine_provided: bool,
}

/// A sampler binding.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SamplerBinding {
    pub group: u32,
    pub binding: u32,
    pub name: String,
    pub filtering: bool,
    pub engine_provided: bool,
}

/// Reflected surface of a validated shader module.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ShaderModule {
    pub entry_points: Vec<EntryPoint>,
    pub uniforms: Vec<UniformBinding>,
    pub textures: Vec<TextureBinding>,
    pub samplers: Vec<SamplerBinding>,
}

impl ShaderModule {
    pub fn fragment_entry_points(&self) -> impl Iterator<Item = &EntryPoint> {
        self.entry_points
            .iter()
            .filter(|e| e.stage == ShaderStage::Fragment)
    }

    /// The binding the engine writes frame constants into (group 0, binding 0).
    pub fn frame_uniform(&self) -> Option<&UniformBinding> {
        self.uniforms.iter().find(|u| u.group == 0 && u.binding == 0)
    }

    /// Agent-declared uniform buffers (everything above group 0).
    pub fn agent_uniforms(&self) -> impl Iterator<Item = &UniformBinding> {
        self.uniforms.iter().filter(|u| u.group > 0)
    }
}

/// Reflect a validated naga module (no fragment-contract check).
pub(crate) fn reflect_ir(module: &naga::Module) -> Result<ShaderModule, ShaderError> {
    // naga's own layout engine is authoritative for sizes and member offsets.
    let mut layouter = naga::proc::Layouter::default();
    layouter
        .update(module.to_ctx())
        .map_err(|e| ShaderError::Validate {
            message: e.to_string(),
            diagnostic: e.to_string(),
            line: 0,
            column: 0,
        })?;
    let size_of = |ty: naga::Handle<naga::Type>| layouter[ty].size;

    let entry_points: Vec<EntryPoint> = module
        .entry_points
        .iter()
        .map(|e| EntryPoint {
            name: e.name.clone(),
            stage: match e.stage {
                naga::ShaderStage::Vertex => ShaderStage::Vertex,
                naga::ShaderStage::Fragment => ShaderStage::Fragment,
                // Compute and the ray-tracing/mesh stages are not used by the
                // engine's image pipeline; they are reflected as compute.
                _ => ShaderStage::Compute,
            },
            workgroup_size: match e.stage {
                naga::ShaderStage::Compute => Some(e.workgroup_size),
                _ => None,
            },
        })
        .collect();

    let mut uniforms = Vec::new();
    let mut textures = Vec::new();
    let mut samplers = Vec::new();

    for (_, var) in module.global_variables.iter() {
        let Some(rb) = var.binding else { continue };
        match var.space {
            AddressSpace::Uniform => {
                // WGSL requires a uniform struct's size to be a multiple of
                // its alignment; `Layouter` applies that, so the buffer we
                // allocate is neither short nor padded by hand.
                let members = struct_members(module, var.ty, &size_of);
                uniforms.push(UniformBinding {
                    group: rb.group,
                    binding: rb.binding,
                    name: var.name.clone().unwrap_or_default(),
                    size: size_of(var.ty).max(1),
                    members,
                });
            }
            AddressSpace::Handle => {
                let (name, dimension, sample_type, is_texture, filtering) = match module.types[var.ty]
                    .inner
                {
                    naga::TypeInner::Image {
                        dim,
                        arrayed: _,
                        class,
                    } => (
                        var.name.clone().unwrap_or_default(),
                        format!("{dim:?}"),
                        format!("{class:?}"),
                        true,
                        false,
                    ),
                    naga::TypeInner::Sampler { comparison } => (
                        var.name.clone().unwrap_or_default(),
                        "Sampler".to_string(),
                        if comparison { "Comparison" } else { "Filtering" }.to_string(),
                        false,
                        !comparison,
                    ),
                    _ => continue,
                };
                let engine_provided = rb.group == 0;
                if is_texture {
                    textures.push(TextureBinding {
                        group: rb.group,
                        binding: rb.binding,
                        name,
                        dimension,
                        sample_type,
                        engine_provided,
                    });
                } else {
                    samplers.push(SamplerBinding {
                        group: rb.group,
                        binding: rb.binding,
                        name,
                        filtering,
                        engine_provided,
                    });
                }
            }
            _ => {}
        }
    }

    uniforms.sort_by_key(|u| (u.group, u.binding));
    textures.sort_by_key(|t| (t.group, t.binding));
    samplers.sort_by_key(|s| (s.group, s.binding));

    let reflected = ShaderModule {
        entry_points,
        uniforms,
        textures,
        samplers,
    };
    Ok(reflected)
}

/// Enforce the exact fragment signature the engine's vertex stage feeds.
///
/// This runs on the naga IR, so a mismatch is reported precisely instead of
/// surfacing as a wgpu pipeline error at render time.
pub(crate) fn check_fragment_signature(module: &naga::Module) -> Result<(), ShaderError> {
    let Some(entry) = module
        .entry_points
        .iter()
        .find(|e| e.name == crate::ENTRY_POINT && e.stage == naga::ShaderStage::Fragment)
    else {
        return Err(ShaderError::MissingEntryPoint);
    };

    let bad = |reason: String| ShaderError::BadEntryPointSignature { reason };

    if entry.function.arguments.len() != 1 {
        return Err(bad(format!(
            "expected exactly 1 input, found {}",
            entry.function.arguments.len()
        )));
    }
    let arg = &entry.function.arguments[0];
    match arg.binding {
        Some(naga::Binding::Location { location: 0, .. }) => {}
        ref other => {
            return Err(bad(format!(
                "input must be @location(0), found {other:?}"
            )));
        }
    }
    if !matches!(
        module.types[arg.ty].inner,
        naga::TypeInner::Vector {
            size: naga::VectorSize::Bi,
            scalar: naga::Scalar {
                kind: naga::ScalarKind::Float,
                width: 4,
            },
        }
    ) {
        return Err(bad("input must be `vec2<f32>`".to_string()));
    }

    let Some(result) = &entry.function.result else {
        return Err(bad("missing return value".to_string()));
    };
    match result.binding {
        Some(naga::Binding::Location { location: 0, .. }) => {}
        ref other => {
            return Err(bad(format!(
                "return value must be @location(0), found {other:?}"
            )));
        }
    }
    if !matches!(
        module.types[result.ty].inner,
        naga::TypeInner::Vector {
            size: naga::VectorSize::Quad,
            scalar: naga::Scalar {
                kind: naga::ScalarKind::Float,
                width: 4,
            },
        }
    ) {
        return Err(bad("return value must be `vec4<f32>`".to_string()));
    }

    Ok(())
}

fn struct_members(
    module: &naga::Module,
    ty: naga::Handle<naga::Type>,
    size_of: &impl Fn(naga::Handle<naga::Type>) -> u32,
) -> Vec<UniformMember> {
    let naga::TypeInner::Struct { ref members, .. } = module.types[ty].inner else {
        return Vec::new();
    };
    members
        .iter()
        .enumerate()
        .map(|(i, m)| UniformMember {
            name: m
                .name
                .clone()
                .unwrap_or_else(|| format!("field{i}")),
            offset: m.offset,
            size: size_of(m.ty),
            ty: classify(module, m.ty),
        })
        .collect()
}

/// Map a WGSL type onto the packing vocabulary.
pub(crate) fn classify(module: &naga::Module, ty: naga::Handle<naga::Type>) -> UniformType {
    use naga::ScalarKind;
    match module.types[ty].inner {
        naga::TypeInner::Scalar(s) => match s.kind {
            ScalarKind::Float => {
                if s.width == 8 {
                    UniformType::F64
                } else {
                    UniformType::F32
                }
            }
            ScalarKind::Sint => UniformType::I32,
            ScalarKind::Uint => UniformType::U32,
            _ => UniformType::Other,
        },
        naga::TypeInner::Vector { size, scalar } => match (size, scalar.kind) {
            (naga::VectorSize::Bi, ScalarKind::Float) => UniformType::Vec2,
            (naga::VectorSize::Tri, ScalarKind::Float) => UniformType::Vec3,
            (naga::VectorSize::Quad, ScalarKind::Float) => UniformType::Vec4,
            _ => UniformType::Other,
        },
        naga::TypeInner::Matrix { rows, columns, .. } => {
            if rows as u32 == 4 && columns as u32 == 4 {
                UniformType::Mat4
            } else if rows as u32 == 3 && columns as u32 == 3 {
                UniformType::Mat3
            } else {
                UniformType::Other
            }
        }
        naga::TypeInner::Array { .. } => UniformType::Array,
        _ => UniformType::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_binding_is_located_at_group_zero() {
        let module = crate::validate(
            "@fragment\nfn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> { return vec4<f32>(uv, 0.0, 1.0); }",
        )
        .unwrap();
        let frame = module.frame_uniform().expect("frame uniform");
        assert_eq!(frame.size, 16, "vec2 + f32 + f32");
        let offsets: Vec<u32> = frame.members.iter().map(|m| m.offset).collect();
        assert_eq!(offsets, vec![0, 8, 12]);
        assert!(module.textures.iter().all(|t| t.engine_provided));
        assert!(module.samplers.iter().all(|s| s.engine_provided));
        assert_eq!(module.agent_uniforms().count(), 0);
    }

    #[test]
    fn vec3_member_advances_by_its_aligned_size() {
        let module = crate::validate(
            r#"
struct Params { a: vec3<f32>, b: f32 };
@group(1) @binding(0) var<uniform> params: Params;
@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> { return vec4<f32>(params.a + vec3<f32>(params.b), 1.0); }
"#,
        )
        .unwrap();
        let params = module.agent_uniforms().next().unwrap();
        assert_eq!(params.members[0].ty, UniformType::Vec3);
        // vec3 occupies 12 bytes and the following f32 lands at 12 in WGSL.
        assert_eq!(params.members[0].offset, 0);
        assert_eq!(params.members[0].size, 12);
        assert_eq!(params.members[1].offset, 12);
        assert_eq!(params.size, 16);
    }

    #[test]
    fn compute_entry_points_report_workgroup_size() {
        // Reflection alone accepts a compute module...
        let module =
            crate::reflect_source("@compute @workgroup_size(8, 8, 1) fn main() {}").unwrap();
        assert_eq!(module.entry_points[0].workgroup_size, Some([8, 8, 1]));
        // ...but the effect contract requires a fragment `main`.
        assert!(matches!(
            crate::validate_full("@compute @workgroup_size(1) fn tick() {}"),
            Err(ShaderError::MissingEntryPoint)
        ));
    }
}
