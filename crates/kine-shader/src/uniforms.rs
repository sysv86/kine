//! Packing agent-supplied uniform values into a GPU buffer.
//!
//! The agent writes `uniforms: { speed: 2.0, tint: [1, 0, 0, 1] }` in
//! JavaScript; naga already told us the byte offset of every field, so this
//! module is the only place that knows about layout rules. Unknown field names
//! are rejected with the list of valid ones — the most common authoring mistake
//! is a typo, and it should never reach the GPU.

use crate::reflect::{UniformBinding, UniformType};
use serde_json::{Map, Value};

/// Size in bytes of the engine's frame constants (`KineFrame`).
pub const FRAME_BINDING_SIZE: u32 = 16;

#[derive(Debug, thiserror::Error)]
pub enum UniformPackError {
    #[error("unknown uniform field(s) {unknown:?}; this shader declares {known:?}")]
    UnknownField { unknown: Vec<String>, known: Vec<String> },

    #[error("uniform `{field}` expects {expected}, got {got}")]
    WrongType {
        field: String,
        expected: &'static str,
        got: String,
    },

    #[error("uniform `{field}` needs {expected} components, got {got}")]
    WrongArity {
        field: String,
        expected: usize,
        got: usize,
    },

    #[error("uniform `{field}` has unsupported WGSL type {ty:?}")]
    UnsupportedType {
        field: String,
        ty: UniformType,
    },
}

/// A packed uniform buffer: bytes plus the declared size.
#[derive(Debug, Clone)]
pub struct UniformPack {
    pub bytes: Vec<u8>,
}

impl UniformPack {
    /// Pack `values` for the given reflected binding.
    pub fn pack(binding: &UniformBinding, values: &Map<String, Value>) -> Result<Self, UniformPackError> {
        let known: Vec<String> = binding.members.iter().map(|m| m.name.clone()).collect();
        let unknown: Vec<String> = values
            .keys()
            .filter(|k| !known.contains(k))
            .cloned()
            .collect();
        if !unknown.is_empty() {
            return Err(UniformPackError::UnknownField { unknown, known });
        }

        let mut bytes = vec![0u8; binding.size.max(FRAME_BINDING_SIZE) as usize];
        for member in &binding.members {
            let Some(value) = values.get(&member.name) else {
                continue; // absent field keeps its zero default
            };
            write_member(&mut bytes, member.offset, &member.name, member.ty, value)?;
        }
        Ok(Self { bytes })
    }

    /// The engine's own frame constants: resolution, seconds, frame index.
    pub fn frame(resolution: (f32, f32), time: f32, frame: u32) -> Self {
        let mut bytes = vec![0u8; FRAME_BINDING_SIZE as usize];
        put_f32(&mut bytes, 0, resolution.0);
        put_f32(&mut bytes, 4, resolution.1);
        put_f32(&mut bytes, 8, time);
        put_f32(&mut bytes, 12, frame as f32);
        Self { bytes }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

fn write_member(
    out: &mut [u8],
    offset: u32,
    field: &str,
    ty: UniformType,
    value: &Value,
) -> Result<(), UniformPackError> {
    match ty {
        UniformType::F32 | UniformType::F64 => {
            let n = as_number(field, value, "a number")?;
            put_f32(out, offset, n as f32);
        }
        UniformType::I32 => {
            let n = as_integer(field, value, "an integer")?;
            put_i32(out, offset, n as i32);
        }
        UniformType::U32 => {
            let n = as_integer(field, value, "an integer")?;
            put_u32(out, offset, n as u32);
        }
        UniformType::Vec2 | UniformType::Vec3 | UniformType::Vec4 => {
            let arity = match ty {
                UniformType::Vec2 => 2,
                UniformType::Vec3 => 3,
                _ => 4,
            };
            let comps = components(field, value, arity)?;
            for (i, c) in comps.iter().enumerate() {
                put_f32(out, offset + 4 * i as u32, *c);
            }
        }
        UniformType::Mat3 => {
            let comps = components(field, value, 9)?;
            // Each column is padded to 16 bytes in uniform space.
            for col in 0..3u32 {
                for row in 0..3u32 {
                    put_f32(out, offset + col * 16 + row * 4, comps[(col * 3 + row) as usize]);
                }
            }
        }
        UniformType::Mat4 => {
            let comps = components(field, value, 16)?;
            for (i, c) in comps.iter().enumerate() {
                put_f32(out, offset + 4 * i as u32, *c);
            }
        }
        UniformType::Array | UniformType::Other => {
            return Err(UniformPackError::UnsupportedType {
                field: field.to_string(),
                ty,
            })
        }
    }
    Ok(())
}

fn as_number(field: &str, value: &Value, expected: &'static str) -> Result<f64, UniformPackError> {
    value.as_f64().ok_or_else(|| UniformPackError::WrongType {
        field: field.to_string(),
        expected,
        got: kind_of(value).to_string(),
    })
}

fn as_integer(field: &str, value: &Value, expected: &'static str) -> Result<i64, UniformPackError> {
    value
        .as_i64()
        .or_else(|| value.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64))
        .ok_or_else(|| UniformPackError::WrongType {
            field: field.to_string(),
            expected,
            got: kind_of(value).to_string(),
        })
}

/// Accept either an array of `arity` numbers or a single number broadcast to all
/// components (`tint: 1` is friendlier than `[1,1,1,1]`).
fn components(field: &str, value: &Value, arity: usize) -> Result<Vec<f32>, UniformPackError> {
    if let Some(n) = value.as_f64() {
        return Ok(vec![n as f32; arity]);
    }
    let arr = value.as_array().ok_or_else(|| UniformPackError::WrongType {
        field: field.to_string(),
        expected: "an array of numbers",
        got: kind_of(value).to_string(),
    })?;
    if arr.len() != arity {
        return Err(UniformPackError::WrongArity {
            field: field.to_string(),
            expected: arity,
            got: arr.len(),
        });
    }
    arr.iter()
        .map(|v| {
            v.as_f64()
                .map(|n| n as f32)
                .ok_or_else(|| UniformPackError::WrongType {
                    field: field.to_string(),
                    expected: "a number",
                    got: kind_of(v).to_string(),
                })
        })
        .collect()
}

fn kind_of(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

fn put_f32(out: &mut [u8], offset: u32, v: f32) {
    let o = offset as usize;
    if o + 4 <= out.len() {
        out[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }
}

fn put_i32(out: &mut [u8], offset: u32, v: i32) {
    let o = offset as usize;
    if o + 4 <= out.len() {
        out[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }
}

fn put_u32(out: &mut [u8], offset: u32, v: u32) {
    let o = offset as usize;
    if o + 4 <= out.len() {
        out[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reflect::UniformBinding;

    fn params_binding() -> UniformBinding {
        // Matches the shape naga produces for `struct Params { speed: f32, tint: vec4<f32> }`
        UniformBinding {
            group: 1,
            binding: 0,
            name: "params".into(),
            size: 32,
            members: vec![
                crate::UniformMember {
                    name: "speed".into(),
                    offset: 0,
                    size: 4,
                    ty: UniformType::F32,
                },
                crate::UniformMember {
                    name: "tint".into(),
                    offset: 16,
                    size: 16,
                    ty: UniformType::Vec4,
                },
            ],
        }
    }

    fn f32_at(bytes: &[u8], offset: usize) -> f32 {
        f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }

    #[test]
    fn packs_values_at_the_reflected_offsets() {
        let values: Map<String, Value> =
            serde_json::from_str(r#"{"speed": 2.5, "tint": [1, 0, 0, 1]}"#).unwrap();
        let pack = UniformPack::pack(&params_binding(), &values).unwrap();
        assert_eq!(pack.bytes.len(), 32);
        assert_eq!(f32_at(&pack.bytes, 0), 2.5);
        assert_eq!(f32_at(&pack.bytes, 16), 1.0);
        assert_eq!(f32_at(&pack.bytes, 20), 0.0);
        assert_eq!(f32_at(&pack.bytes, 28), 1.0);
    }

    #[test]
    fn a_scalar_broadcasts_into_a_vector() {
        let values: Map<String, Value> = serde_json::from_str(r#"{"tint": 0.5}"#).unwrap();
        let pack = UniformPack::pack(&params_binding(), &values).unwrap();
        for i in 0..4 {
            assert_eq!(f32_at(&pack.bytes, 16 + i * 4), 0.5);
        }
    }

    #[test]
    fn missing_fields_keep_their_zero_default() {
        let values: Map<String, Value> = serde_json::from_str(r#"{"speed": 1}"#).unwrap();
        let pack = UniformPack::pack(&params_binding(), &values).unwrap();
        assert_eq!(f32_at(&pack.bytes, 0), 1.0);
        assert_eq!(f32_at(&pack.bytes, 16), 0.0);
    }

    #[test]
    fn a_typo_is_rejected_with_the_valid_names() {
        let values: Map<String, Value> = serde_json::from_str(r#"{"sped": 1}"#).unwrap();
        let err = UniformPack::pack(&params_binding(), &values).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("sped"), "{msg}");
        assert!(msg.contains("speed"), "{msg}");
        assert!(msg.contains("tint"), "{msg}");
    }

    #[test]
    fn wrong_arity_and_wrong_type_are_distinguished() {
        let values: Map<String, Value> = serde_json::from_str(r#"{"tint": [1, 0]}"#).unwrap();
        let err = UniformPack::pack(&params_binding(), &values).unwrap_err();
        assert!(matches!(err, UniformPackError::WrongArity { expected: 4, got: 2, .. }));

        let values: Map<String, Value> = serde_json::from_str(r#"{"speed": "fast"}"#).unwrap();
        let err = UniformPack::pack(&params_binding(), &values).unwrap_err();
        assert!(matches!(err, UniformPackError::WrongType { .. }));
    }

    #[test]
    fn frame_constants_are_sixteen_bytes() {
        let pack = UniformPack::frame((1920.0, 1080.0), 1.25, 7);
        assert_eq!(pack.bytes.len(), FRAME_BINDING_SIZE as usize);
        assert_eq!(f32_at(&pack.bytes, 0), 1920.0);
        assert_eq!(f32_at(&pack.bytes, 4), 1080.0);
        assert_eq!(f32_at(&pack.bytes, 8), 1.25);
        assert_eq!(f32_at(&pack.bytes, 12), 7.0);
    }

    #[test]
    fn packing_matches_a_real_reflected_shader() {
        // End-to-end: reflect the actual WGSL, then pack using its offsets.
        let module = crate::validate(
            r#"
struct Params { speed: f32, tint: vec4<f32> };
@group(1) @binding(0) var<uniform> params: Params;
@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(params.tint.rgb * params.speed, 1.0);
}
"#,
        )
        .unwrap();
        let binding = module.agent_uniforms().next().unwrap();
        assert_eq!(binding.members[1].offset, 16, "reflection drives packing");
        let values: Map<String, Value> =
            serde_json::from_str(r#"{"speed": 3, "tint": [0.1, 0.2, 0.3, 1]}"#).unwrap();
        let pack = UniformPack::pack(binding, &values).unwrap();
        assert_eq!(f32_at(&pack.bytes, 0), 3.0);
        assert!((f32_at(&pack.bytes, 16) - 0.1).abs() < 1e-6);
    }
}
