//! Deterministic CPU Virtual Machine for executing validated `naga` WGSL fragment
//! shaders when running on the CPU backend (e.g., headless Linux/macOS CI or
//! `--backend cpu`).
//!
//! Instead of silently dropping `NodeKind::Shader` layers on hosts without a GPU,
//! this interpreter evaluates the reflected `naga::Module` AST per pixel across
//! hardware threads (`std::thread::scope`), reading the engine's `kine_frame`
//! uniform buffer (group 0, binding 0), sampling `kine_input` with bilinear
//! filtering (group 0, binding 1/2), reading packed agent uniforms (group 1,
//! binding 0), and compositing with the exact premultiplied blend modes used by
//! the `wgpu` pipeline.

use crate::{
    parse_and_validate, reflect, relocate, wrap, ShaderError, ShaderModule, UniformPack,
};
use naga::{
    AddressSpace, BinaryOperator, Block, Expression, Function, GlobalVariable, Handle, ImageQuery,
    Literal, LocalVariable, MathFunction, Module, ScalarKind, ShaderStage, Statement,
    SwizzleComponent, TypeInner, UnaryOperator, VectorSize,
};
use serde_json::{Map, Value};

/// Blending mode for compositing a shader pass over the premultiplied RGBA target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuBlendMode {
    Normal,
    Add,
    Multiply,
    Screen,
}

/// Compiled WGSL fragment shader ready for multi-threaded CPU execution.
pub struct CpuShaderProgram {
    module: Module,
    entry_index: usize,
    reflected: ShaderModule,
    agent_bytes: Vec<u8>,
}

impl CpuShaderProgram {
    /// Compile and pack an agent-authored WGSL fragment shader and its uniform map.
    pub fn compile(
        agent_source: &str,
        uniforms: &Map<String, Value>,
    ) -> Result<Self, String> {
        let wrapped = wrap(agent_source);
        let module = parse_and_validate(&wrapped)
            .map_err(|e| relocate(e, agent_source).to_string())?;
        reflect::check_fragment_signature(&module)
            .map_err(|e| relocate(e, agent_source).to_string())?;
        let reflected = reflect::reflect_ir(&module)
            .map_err(|e| relocate(e, agent_source).to_string())?;

        let entry_index = module
            .entry_points
            .iter()
            .position(|e| e.name == crate::ENTRY_POINT && e.stage == ShaderStage::Fragment)
            .ok_or_else(|| ShaderError::MissingEntryPoint.to_string())?;

        let agent_bindings: Vec<_> = reflected.agent_uniforms().collect();
        if agent_bindings.len() > 1 {
            return Err(format!(
                "{} uniform buffers declared; v1 supports one (group 1, binding 0)",
                agent_bindings.len()
            ));
        }

        let agent_bytes = match agent_bindings.first() {
            Some(binding) => UniformPack::pack(binding, uniforms)
                .map_err(|e| e.to_string())?
                .bytes,
            None => vec![0u8; 16],
        };

        Ok(Self {
            module,
            entry_index,
            reflected,
            agent_bytes,
        })
    }

    pub fn reflected(&self) -> &ShaderModule {
        &self.reflected
    }

    /// Execute the fragment shader across `scissor = (x, y, w, h)` over a
    /// premultiplied RGBA8 framebuffer (`width * height * 4` bytes) in place.
    ///
    /// Uses a snapshot of `target_rgba` as `kine_input` (matching the ping-pong
    /// texture architecture on the GPU) and parallelizes scanlines across
    /// available CPU cores.
    pub fn execute_on_premul_rgba(
        &self,
        target_rgba: &mut [u8],
        width: u32,
        height: u32,
        scissor: (u32, u32, u32, u32),
        time: f32,
        frame: u32,
        blend: CpuBlendMode,
        opacity: f32,
    ) {
        if width == 0 || height == 0 || target_rgba.len() < (width as usize) * (height as usize) * 4 {
            return;
        }
        let (sx, sy, sw, sh) = scissor;
        let x0 = sx.min(width);
        let y0 = sy.min(height);
        let x1 = sx.saturating_add(sw).min(width);
        let y1 = sy.saturating_add(sh).min(height);
        if x0 >= x1 || y0 >= y1 {
            return;
        }

        let input_snapshot = target_rgba.to_vec();
        let frame_pack = UniformPack::frame((width as f32, height as f32), time, frame);
        let frame_bytes = frame_pack.as_bytes();
        let opacity = opacity.clamp(0.0, 1.0);

        let row_bytes = (width as usize) * 4;
        let active_rows = (y1 - y0) as usize;
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .min(active_rows.max(1));

        let slice_start = (y0 as usize) * row_bytes;
        let slice_end = (y1 as usize) * row_bytes;
        let active_slice = &mut target_rgba[slice_start..slice_end];

        let rows_per_chunk = active_rows.div_ceil(threads).max(1);
        let chunk_bytes = rows_per_chunk * row_bytes;

        std::thread::scope(|s| {
            for (chunk_idx, chunk) in active_slice.chunks_mut(chunk_bytes).enumerate() {
                let start_y = y0 + (chunk_idx * rows_per_chunk) as u32;
                let input_ref = &input_snapshot;
                s.spawn(move || {
                    let chunk_rows = chunk.len() / row_bytes;
                    let mut ctx = EvalContext::new(
                        &self.module,
                        frame_bytes,
                        &self.agent_bytes,
                        input_ref,
                        width,
                        height,
                    );
                    for local_r in 0..chunk_rows {
                        let py = start_y + local_r as u32;
                        let v = (py as f32 + 0.5) / (height as f32);
                        let row_out = &mut chunk[local_r * row_bytes..(local_r + 1) * row_bytes];
                        for px in x0..x1 {
                            let u = (px as f32 + 0.5) / (width as f32);
                            let out_color = ctx.eval_entry(self.entry_index, [u, v]);
                            let o = (px as usize) * 4;
                            let dst = [
                                row_out[o],
                                row_out[o + 1],
                                row_out[o + 2],
                                row_out[o + 3],
                            ];
                            let blended = blend_pixel(dst, out_color, blend, opacity);
                            row_out[o..o + 4].copy_from_slice(&blended);
                        }
                    }
                });
            }
        });
    }
}

fn blend_pixel(dst: [u8; 4], src_rgba: [f32; 4], blend: CpuBlendMode, opacity: f32) -> [u8; 4] {
    let sa = (src_rgba[3].clamp(0.0, 1.0) * opacity).clamp(0.0, 1.0);
    // Shader output is scaled by effective alpha for premultiplied compositing
    let sr = src_rgba[0].clamp(0.0, 1.0) * opacity;
    let sg = src_rgba[1].clamp(0.0, 1.0) * opacity;
    let sb = src_rgba[2].clamp(0.0, 1.0) * opacity;

    let dr = f32::from(dst[0]) / 255.0;
    let dg = f32::from(dst[1]) / 255.0;
    let db = f32::from(dst[2]) / 255.0;
    let da = f32::from(dst[3]) / 255.0;

    let (or, og, ob) = match blend {
        CpuBlendMode::Normal => (
            sr + dr * (1.0 - sa),
            sg + dg * (1.0 - sa),
            sb + db * (1.0 - sa),
        ),
        CpuBlendMode::Add => (sr + dr, sg + dg, sb + db),
        CpuBlendMode::Multiply => (sr * dr, sg * dg, sb * db),
        CpuBlendMode::Screen => (
            sr + dr * (1.0 - sr),
            sg + dg * (1.0 - sg),
            sb + db * (1.0 - sb),
        ),
    };
    let oa = sa + da * (1.0 - sa);

    [
        (or.clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
        (og.clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
        (ob.clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
        (oa.clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
    ]
}

// ── Runtime value representation ────────────────────────────────────────────

#[derive(Debug, Clone)]
enum Val {
    Void,
    Scalar(f32),
    Vec2([f32; 2]),
    Vec3([f32; 3]),
    Vec4([f32; 4]),
    Struct(Vec<Val>),
    LocalPtr(Handle<LocalVariable>, Option<usize>),
    GlobalUniformPtr { group: u32, offset: u32, ty: Handle<naga::Type> },
}

impl Val {
    fn as_f32(&self) -> f32 {
        match self {
            Self::Scalar(v) => *v,
            Self::Vec2(v) => v[0],
            Self::Vec3(v) => v[0],
            Self::Vec4(v) => v[0],
            _ => 0.0,
        }
    }

    fn as_bool(&self) -> bool {
        self.as_f32() != 0.0
    }

    fn as_vec2(&self) -> [f32; 2] {
        match self {
            Self::Scalar(v) => [*v, *v],
            Self::Vec2(v) => *v,
            Self::Vec3(v) => [v[0], v[1]],
            Self::Vec4(v) => [v[0], v[1]],
            _ => [0.0, 0.0],
        }
    }

    fn as_vec3(&self) -> [f32; 3] {
        match self {
            Self::Scalar(v) => [*v, *v, *v],
            Self::Vec2(v) => [v[0], v[1], 0.0],
            Self::Vec3(v) => *v,
            Self::Vec4(v) => [v[0], v[1], v[2]],
            _ => [0.0, 0.0, 0.0],
        }
    }

    fn as_vec4(&self) -> [f32; 4] {
        match self {
            Self::Scalar(v) => [*v, *v, *v, *v],
            Self::Vec2(v) => [v[0], v[1], 0.0, 1.0],
            Self::Vec3(v) => [v[0], v[1], v[2], 1.0],
            Self::Vec4(v) => *v,
            _ => [0.0, 0.0, 0.0, 1.0],
        }
    }

    fn component(&self, idx: usize) -> f32 {
        match self {
            Self::Scalar(v) => *v,
            Self::Vec2(v) => v.get(idx).copied().unwrap_or(0.0),
            Self::Vec3(v) => v.get(idx).copied().unwrap_or(0.0),
            Self::Vec4(v) => v.get(idx).copied().unwrap_or(0.0),
            Self::Struct(fields) => fields.get(idx).map(Val::as_f32).unwrap_or(0.0),
            _ => 0.0,
        }
    }

    fn set_component(&mut self, idx: usize, val: f32) {
        match self {
            Self::Scalar(v) if idx == 0 => *v = val,
            Self::Vec2(v) if idx < 2 => v[idx] = val,
            Self::Vec3(v) if idx < 3 => v[idx] = val,
            Self::Vec4(v) if idx < 4 => v[idx] = val,
            _ => {}
        }
    }

    fn map_unary(&self, f: impl Fn(f32) -> f32) -> Self {
        match self {
            Self::Scalar(x) => Self::Scalar(f(*x)),
            Self::Vec2([x, y]) => Self::Vec2([f(*x), f(*y)]),
            Self::Vec3([x, y, z]) => Self::Vec3([f(*x), f(*y), f(*z)]),
            Self::Vec4([x, y, z, w]) => Self::Vec4([f(*x), f(*y), f(*z), f(*w)]),
            _ => Self::Scalar(0.0),
        }
    }

    fn map_binary(&self, rhs: &Self, f: impl Fn(f32, f32) -> f32) -> Self {
        match (self, rhs) {
            (Self::Scalar(a), Self::Scalar(b)) => Self::Scalar(f(*a, *b)),
            (Self::Vec2(a), Self::Vec2(b)) => Self::Vec2([f(a[0], b[0]), f(a[1], b[1])]),
            (Self::Vec2(a), Self::Scalar(b)) => Self::Vec2([f(a[0], *b), f(a[1], *b)]),
            (Self::Scalar(a), Self::Vec2(b)) => Self::Vec2([f(*a, b[0]), f(*a, b[1])]),
            (Self::Vec3(a), Self::Vec3(b)) => {
                Self::Vec3([f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2])])
            }
            (Self::Vec3(a), Self::Scalar(b)) => Self::Vec3([f(a[0], *b), f(a[1], *b), f(a[2], *b)]),
            (Self::Scalar(a), Self::Vec3(b)) => Self::Vec3([f(*a, b[0]), f(*a, b[1]), f(*a, b[2])]),
            (Self::Vec4(a), Self::Vec4(b)) => {
                Self::Vec4([f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2]), f(a[3], b[3])])
            }
            (Self::Vec4(a), Self::Scalar(b)) => {
                Self::Vec4([f(a[0], *b), f(a[1], *b), f(a[2], *b), f(a[3], *b)])
            }
            (Self::Scalar(a), Self::Vec4(b)) => {
                Self::Vec4([f(*a, b[0]), f(*a, b[1]), f(*a, b[2]), f(*a, b[3])])
            }
            _ => Self::Scalar(f(self.as_f32(), rhs.as_f32())),
        }
    }

    fn map_ternary(&self, b: &Self, c: &Self, f: impl Fn(f32, f32, f32) -> f32) -> Self {
        match self {
            Self::Scalar(a) => Self::Scalar(f(*a, b.as_f32(), c.as_f32())),
            Self::Vec2(a) => {
                let bv = b.as_vec2();
                let cv = c.as_vec2();
                Self::Vec2([f(a[0], bv[0], cv[0]), f(a[1], bv[1], cv[1])])
            }
            Self::Vec3(a) => {
                let bv = b.as_vec3();
                let cv = c.as_vec3();
                Self::Vec3([
                    f(a[0], bv[0], cv[0]),
                    f(a[1], bv[1], cv[1]),
                    f(a[2], bv[2], cv[2]),
                ])
            }
            Self::Vec4(a) => {
                let bv = b.as_vec4();
                let cv = c.as_vec4();
                Self::Vec4([
                    f(a[0], bv[0], cv[0]),
                    f(a[1], bv[1], cv[1]),
                    f(a[2], bv[2], cv[2]),
                    f(a[3], bv[3], cv[3]),
                ])
            }
            _ => Self::Scalar(0.0),
        }
    }
}

enum ControlFlow {
    ContinueExec,
    Break,
    ContinueLoop,
    Return(Val),
}

struct EvalContext<'a> {
    module: &'a Module,
    frame_bytes: &'a [u8],
    agent_bytes: &'a [u8],
    input_rgba: &'a [u8],
    width: u32,
    height: u32,
}

impl<'a> EvalContext<'a> {
    fn new(
        module: &'a Module,
        frame_bytes: &'a [u8],
        agent_bytes: &'a [u8],
        input_rgba: &'a [u8],
        width: u32,
        height: u32,
    ) -> Self {
        Self {
            module,
            frame_bytes,
            agent_bytes,
            input_rgba,
            width,
            height,
        }
    }

    fn eval_entry(&mut self, entry_idx: usize, uv: [f32; 2]) -> [f32; 4] {
        let func = &self.module.entry_points[entry_idx].function;
        let out = self.eval_function(func, &[Val::Vec2(uv)]);
        out.as_vec4()
    }

    fn eval_function(&mut self, func: &Function, args: &[Val]) -> Val {
        let mut locals: Vec<Val> = func
            .local_variables
            .iter()
            .map(|(_, lv)| {
                lv.init
                    .map(|h| self.eval_const_or_expr(func, h, args, &[], &[]))
                    .unwrap_or_else(|| self.zero_value(lv.ty))
            })
            .collect();
        let mut cached_exprs: Vec<Option<Val>> = vec![None; func.expressions.len()];

        match self.exec_block(func, &func.body, args, &mut locals, &mut cached_exprs) {
            ControlFlow::Return(v) => v,
            _ => Val::Vec4([0.0, 0.0, 0.0, 1.0]),
        }
    }

    fn exec_block(
        &mut self,
        func: &Function,
        block: &Block,
        args: &[Val],
        locals: &mut [Val],
        cached: &mut [Option<Val>],
    ) -> ControlFlow {
        for stmt in block.iter() {
            match stmt {
                Statement::Emit(range) => {
                    for h in range.clone() {
                        let v = self.eval_expr(func, h, args, locals, cached);
                        cached[h.index()] = Some(v);
                    }
                }
                Statement::Block(inner) => {
                    let cf = self.exec_block(func, inner, args, locals, cached);
                    if !matches!(cf, ControlFlow::ContinueExec) {
                        return cf;
                    }
                }
                Statement::If {
                    condition,
                    accept,
                    reject,
                } => {
                    let cond = self.eval_expr(func, *condition, args, locals, cached).as_bool();
                    let branch = if cond { accept } else { reject };
                    let cf = self.exec_block(func, branch, args, locals, cached);
                    if !matches!(cf, ControlFlow::ContinueExec) {
                        return cf;
                    }
                }
                Statement::Switch { selector, cases } => {
                    let sel = self.eval_expr(func, *selector, args, locals, cached).as_f32() as i32;
                    for case in cases {
                        let matched = match case.value {
                            naga::SwitchValue::I32(v) => v == sel,
                            naga::SwitchValue::U32(v) => (v as i32) == sel,
                            naga::SwitchValue::Default => true,
                        };
                        if matched {
                            let cf = self.exec_block(func, &case.body, args, locals, cached);
                            match cf {
                                ControlFlow::Break => break,
                                ControlFlow::ContinueExec => {
                                    if !case.fall_through {
                                        break;
                                    }
                                }
                                other => return other,
                            }
                        }
                    }
                }
                Statement::Loop {
                    body,
                    continuing,
                    break_if,
                } => {
                    // Deterministic iteration cap so an agent authoring an accidental
                    // infinite loop never hangs the host process.
                    for _ in 0..1024 {
                        match self.exec_block(func, body, args, locals, cached) {
                            ControlFlow::Break => break,
                            ControlFlow::Return(v) => return ControlFlow::Return(v),
                            ControlFlow::ContinueLoop | ControlFlow::ContinueExec => {}
                        }
                        match self.exec_block(func, continuing, args, locals, cached) {
                            ControlFlow::Break => break,
                            ControlFlow::Return(v) => return ControlFlow::Return(v),
                            _ => {}
                        }
                        if let Some(cond_h) = break_if {
                            if self.eval_expr(func, *cond_h, args, locals, cached).as_bool() {
                                break;
                            }
                        }
                    }
                }
                Statement::Break => return ControlFlow::Break,
                Statement::Continue => return ControlFlow::ContinueLoop,
                Statement::Return { value } => {
                    let v = value
                        .map(|h| self.eval_expr(func, h, args, locals, cached))
                        .unwrap_or(Val::Void);
                    return ControlFlow::Return(v);
                }
                Statement::Kill => return ControlFlow::Return(Val::Vec4([0.0, 0.0, 0.0, 0.0])),
                Statement::Store { pointer, value } => {
                    let val = self.eval_expr(func, *value, args, locals, cached);
                    let ptr = self.eval_expr(func, *pointer, args, locals, cached);
                    if let Val::LocalPtr(lv_h, comp) = ptr {
                        if let Some(slot) = locals.get_mut(lv_h.index()) {
                            match comp {
                                Some(idx) => slot.set_component(idx, val.as_f32()),
                                None => *slot = val,
                            }
                        }
                    }
                }
                Statement::Call {
                    function,
                    arguments,
                    result,
                } => {
                    let call_args: Vec<Val> = arguments
                        .iter()
                        .map(|&a| self.eval_expr(func, a, args, locals, cached))
                        .collect();
                    let callee = &self.module.functions[*function];
                    let ret = self.eval_function(callee, &call_args);
                    if let Some(res_h) = result {
                        cached[res_h.index()] = Some(ret);
                    }
                }
                _ => {}
            }
        }
        ControlFlow::ContinueExec
    }

    fn eval_const_or_expr(
        &self,
        func: &Function,
        h: Handle<Expression>,
        args: &[Val],
        locals: &[Val],
        cached: &[Option<Val>],
    ) -> Val {
        self.eval_expr(func, h, args, locals, cached)
    }

    fn eval_expr(
        &self,
        func: &Function,
        h: Handle<Expression>,
        args: &[Val],
        locals: &[Val],
        cached: &[Option<Val>],
    ) -> Val {
        if let Some(Some(v)) = cached.get(h.index()) {
            return v.clone();
        }
        match &func.expressions[h] {
            Expression::Literal(lit) => match lit {
                Literal::F32(v) => Val::Scalar(*v),
                Literal::F64(v) => Val::Scalar(*v as f32),
                Literal::U32(v) => Val::Scalar(*v as f32),
                Literal::I32(v) => Val::Scalar(*v as f32),
                Literal::U64(v) => Val::Scalar(*v as f32),
                Literal::I64(v) => Val::Scalar(*v as f32),
                Literal::Bool(b) => Val::Scalar(if *b { 1.0 } else { 0.0 }),
                Literal::AbstractInt(v) => Val::Scalar(*v as f32),
                Literal::AbstractFloat(v) => Val::Scalar(*v as f32),
                _ => Val::Scalar(0.0),
            },
            Expression::Constant(ch) => {
                let c = &self.module.constants[*ch];
                self.eval_module_expr(c.init)
            }
            Expression::ZeroValue(ty) => self.zero_value(*ty),
            Expression::Compose { ty, components } => {
                let vals: Vec<Val> = components
                    .iter()
                    .map(|&c| self.eval_expr(func, c, args, locals, cached))
                    .collect();
                self.compose_type(*ty, &vals)
            }
            Expression::Splat { size, value } => {
                let s = self.eval_expr(func, *value, args, locals, cached).as_f32();
                match size {
                    VectorSize::Bi => Val::Vec2([s, s]),
                    VectorSize::Tri => Val::Vec3([s, s, s]),
                    VectorSize::Quad => Val::Vec4([s, s, s, s]),
                }
            }
            Expression::Swizzle {
                size,
                vector,
                pattern,
            } => {
                let vec = self.eval_expr(func, *vector, args, locals, cached);
                let idx = |c: SwizzleComponent| match c {
                    SwizzleComponent::X => 0,
                    SwizzleComponent::Y => 1,
                    SwizzleComponent::Z => 2,
                    SwizzleComponent::W => 3,
                };
                match size {
                    VectorSize::Bi => {
                        Val::Vec2([vec.component(idx(pattern[0])), vec.component(idx(pattern[1]))])
                    }
                    VectorSize::Tri => Val::Vec3([
                        vec.component(idx(pattern[0])),
                        vec.component(idx(pattern[1])),
                        vec.component(idx(pattern[2])),
                    ]),
                    VectorSize::Quad => Val::Vec4([
                        vec.component(idx(pattern[0])),
                        vec.component(idx(pattern[1])),
                        vec.component(idx(pattern[2])),
                        vec.component(idx(pattern[3])),
                    ]),
                }
            }
            Expression::Access { base, index } => {
                let idx = self.eval_expr(func, *index, args, locals, cached).as_f32().max(0.0) as u32;
                self.eval_access_index(func, *base, idx, args, locals, cached)
            }
            Expression::AccessIndex { base, index } => {
                self.eval_access_index(func, *base, *index, args, locals, cached)
            }
            Expression::FunctionArgument(i) => {
                args.get(*i as usize).cloned().unwrap_or(Val::Scalar(0.0))
            }
            Expression::GlobalVariable(gv_h) => self.eval_global_var(*gv_h),
            Expression::LocalVariable(lv_h) => Val::LocalPtr(*lv_h, None),
            Expression::Load { pointer } => {
                let ptr = self.eval_expr(func, *pointer, args, locals, cached);
                self.load_ptr(ptr, locals)
            }
            Expression::ImageSample {
                coordinate,
                ..
            } => {
                let uv = self.eval_expr(func, *coordinate, args, locals, cached).as_vec2();
                Val::Vec4(self.sample_bilinear(uv[0], uv[1]))
            }
            Expression::ImageLoad { coordinate, .. } => {
                let xy = self.eval_expr(func, *coordinate, args, locals, cached).as_vec2();
                let x = (xy[0] as i32).clamp(0, self.width as i32 - 1) as u32;
                let y = (xy[1] as i32).clamp(0, self.height as i32 - 1) as u32;
                Val::Vec4(self.texel_at(x, y))
            }
            Expression::ImageQuery { query, .. } => match query {
                ImageQuery::Size { .. } => Val::Vec2([self.width as f32, self.height as f32]),
                _ => Val::Scalar(1.0),
            },
            Expression::Unary { op, expr } => {
                let v = self.eval_expr(func, *expr, args, locals, cached);
                match op {
                    UnaryOperator::Negate => v.map_unary(|x| -x),
                    UnaryOperator::LogicalNot => v.map_unary(|x| if x == 0.0 { 1.0 } else { 0.0 }),
                    UnaryOperator::BitwiseNot => v.map_unary(|x| (!(x as i32)) as f32),
                }
            }
            Expression::Binary { op, left, right } => {
                let l = self.eval_expr(func, *left, args, locals, cached);
                let r = self.eval_expr(func, *right, args, locals, cached);
                self.eval_binary(*op, &l, &r)
            }
            Expression::Select {
                condition,
                accept,
                reject,
            } => {
                let c = self.eval_expr(func, *condition, args, locals, cached);
                let a = self.eval_expr(func, *accept, args, locals, cached);
                let b = self.eval_expr(func, *reject, args, locals, cached);
                a.map_ternary(&b, &c, |av, bv, cv| if cv != 0.0 { av } else { bv })
            }
            Expression::Relational { fun, argument } => {
                let v = self.eval_expr(func, *argument, args, locals, cached);
                let comps: Vec<f32> = match v {
                    Val::Scalar(x) => vec![x],
                    Val::Vec2(x) => x.to_vec(),
                    Val::Vec3(x) => x.to_vec(),
                    Val::Vec4(x) => x.to_vec(),
                    _ => vec![0.0],
                };
                let b = match fun {
                    naga::RelationalFunction::All => comps.iter().all(|&x| x != 0.0),
                    naga::RelationalFunction::Any => comps.iter().any(|&x| x != 0.0),
                    naga::RelationalFunction::IsNan => comps.iter().any(|x| x.is_nan()),
                    naga::RelationalFunction::IsInf => comps.iter().any(|x| x.is_infinite()),
                };
                Val::Scalar(if b { 1.0 } else { 0.0 })
            }
            Expression::Math {
                fun,
                arg,
                arg1,
                arg2,
                ..
            } => {
                let a0 = self.eval_expr(func, *arg, args, locals, cached);
                let a1 = arg1.map(|h| self.eval_expr(func, h, args, locals, cached));
                let a2 = arg2.map(|h| self.eval_expr(func, h, args, locals, cached));
                self.eval_math(*fun, &a0, a1.as_ref(), a2.as_ref())
            }
            Expression::As { expr, kind, .. } => {
                let v = self.eval_expr(func, *expr, args, locals, cached);
                match kind {
                    ScalarKind::Sint => v.map_unary(|x| (x as i32) as f32),
                    ScalarKind::Uint => v.map_unary(|x| (x.max(0.0) as u32) as f32),
                    ScalarKind::Bool => v.map_unary(|x| if x != 0.0 { 1.0 } else { 0.0 }),
                    _ => v,
                }
            }
            Expression::CallResult(_) => Val::Scalar(0.0),
            _ => Val::Scalar(0.0),
        }
    }

    fn eval_module_expr(&self, h: Handle<Expression>) -> Val {
        match &self.module.global_expressions[h] {
            Expression::Literal(lit) => match lit {
                Literal::F32(v) => Val::Scalar(*v),
                Literal::F64(v) => Val::Scalar(*v as f32),
                Literal::U32(v) => Val::Scalar(*v as f32),
                Literal::I32(v) => Val::Scalar(*v as f32),
                Literal::Bool(b) => Val::Scalar(if *b { 1.0 } else { 0.0 }),
                Literal::AbstractInt(v) => Val::Scalar(*v as f32),
                Literal::AbstractFloat(v) => Val::Scalar(*v as f32),
                _ => Val::Scalar(0.0),
            },
            Expression::ZeroValue(ty) => self.zero_value(*ty),
            Expression::Compose { ty, components } => {
                let vals: Vec<Val> = components
                    .iter()
                    .map(|&c| self.eval_module_expr(c))
                    .collect();
                self.compose_type(*ty, &vals)
            }
            Expression::Splat { size, value } => {
                let s = self.eval_module_expr(*value).as_f32();
                match size {
                    VectorSize::Bi => Val::Vec2([s, s]),
                    VectorSize::Tri => Val::Vec3([s, s, s]),
                    VectorSize::Quad => Val::Vec4([s, s, s, s]),
                }
            }
            _ => Val::Scalar(0.0),
        }
    }

    fn eval_access_index(
        &self,
        func: &Function,
        base: Handle<Expression>,
        index: u32,
        args: &[Val],
        locals: &[Val],
        cached: &[Option<Val>],
    ) -> Val {
        let b = self.eval_expr(func, base, args, locals, cached);
        match b {
            Val::LocalPtr(lv_h, None) => Val::LocalPtr(lv_h, Some(index as usize)),
            Val::GlobalUniformPtr { group, offset, ty } => {
                if let TypeInner::Struct { ref members, .. } = self.module.types[ty].inner {
                    if let Some(m) = members.get(index as usize) {
                        return Val::GlobalUniformPtr {
                            group,
                            offset: offset + m.offset,
                            ty: m.ty,
                        };
                    }
                }
                if let TypeInner::Vector { scalar, .. } = self.module.types[ty].inner {
                    let elem_offset = offset + index * u32::from(scalar.width);
                    return Val::Scalar(self.read_uniform_scalar(group, elem_offset, scalar));
                }
                Val::Scalar(0.0)
            }
            Val::Struct(fields) => fields
                .get(index as usize)
                .cloned()
                .unwrap_or(Val::Scalar(0.0)),
            other => Val::Scalar(other.component(index as usize)),
        }
    }

    fn eval_global_var(&self, gv_h: Handle<GlobalVariable>) -> Val {
        let gv = &self.module.global_variables[gv_h];
        if gv.space == AddressSpace::Uniform {
            let group = gv.binding.as_ref().map(|b| b.group).unwrap_or(0);
            return Val::GlobalUniformPtr {
                group,
                offset: 0,
                ty: gv.ty,
            };
        }
        Val::Scalar(0.0)
    }

    fn load_ptr(&self, ptr: Val, locals: &[Val]) -> Val {
        match ptr {
            Val::LocalPtr(lv_h, None) => {
                locals.get(lv_h.index()).cloned().unwrap_or(Val::Scalar(0.0))
            }
            Val::LocalPtr(lv_h, Some(comp)) => Val::Scalar(
                locals
                    .get(lv_h.index())
                    .map(|v| v.component(comp))
                    .unwrap_or(0.0),
            ),
            Val::GlobalUniformPtr { group, offset, ty } => self.read_uniform_type(group, offset, ty),
            other => other,
        }
    }

    fn read_uniform_type(&self, group: u32, offset: u32, ty: Handle<naga::Type>) -> Val {
        match self.module.types[ty].inner {
            TypeInner::Scalar(s) => Val::Scalar(self.read_uniform_scalar(group, offset, s)),
            TypeInner::Vector { size, scalar } => {
                let s0 = self.read_uniform_scalar(group, offset, scalar);
                let s1 = self.read_uniform_scalar(group, offset + 4, scalar);
                match size {
                    VectorSize::Bi => Val::Vec2([s0, s1]),
                    VectorSize::Tri => {
                        let s2 = self.read_uniform_scalar(group, offset + 8, scalar);
                        Val::Vec3([s0, s1, s2])
                    }
                    VectorSize::Quad => {
                        let s2 = self.read_uniform_scalar(group, offset + 8, scalar);
                        let s3 = self.read_uniform_scalar(group, offset + 12, scalar);
                        Val::Vec4([s0, s1, s2, s3])
                    }
                }
            }
            TypeInner::Struct { ref members, .. } => {
                let fields = members
                    .iter()
                    .map(|m| self.read_uniform_type(group, offset + m.offset, m.ty))
                    .collect();
                Val::Struct(fields)
            }
            _ => Val::Scalar(0.0),
        }
    }

    fn read_uniform_scalar(&self, group: u32, offset: u32, scalar: naga::Scalar) -> f32 {
        let buf = if group == 0 {
            self.frame_bytes
        } else {
            self.agent_bytes
        };
        let o = offset as usize;
        if o + 4 > buf.len() {
            return 0.0;
        }
        let bytes: [u8; 4] = [buf[o], buf[o + 1], buf[o + 2], buf[o + 3]];
        match scalar.kind {
            ScalarKind::Sint => i32::from_le_bytes(bytes) as f32,
            ScalarKind::Uint => u32::from_le_bytes(bytes) as f32,
            ScalarKind::Bool => {
                if u32::from_le_bytes(bytes) != 0 {
                    1.0
                } else {
                    0.0
                }
            }
            _ => f32::from_le_bytes(bytes),
        }
    }

    fn zero_value(&self, ty: Handle<naga::Type>) -> Val {
        match self.module.types[ty].inner {
            TypeInner::Scalar(_) => Val::Scalar(0.0),
            TypeInner::Vector { size, .. } => match size {
                VectorSize::Bi => Val::Vec2([0.0, 0.0]),
                VectorSize::Tri => Val::Vec3([0.0, 0.0, 0.0]),
                VectorSize::Quad => Val::Vec4([0.0, 0.0, 0.0, 0.0]),
            },
            TypeInner::Struct { ref members, .. } => {
                Val::Struct(members.iter().map(|m| self.zero_value(m.ty)).collect())
            }
            _ => Val::Scalar(0.0),
        }
    }

    fn compose_type(&self, ty: Handle<naga::Type>, vals: &[Val]) -> Val {
        match self.module.types[ty].inner {
            TypeInner::Vector { size, .. } => {
                let mut flat = Vec::with_capacity(4);
                for v in vals {
                    match v {
                        Val::Scalar(x) => flat.push(*x),
                        Val::Vec2(x) => flat.extend_from_slice(x),
                        Val::Vec3(x) => flat.extend_from_slice(x),
                        Val::Vec4(x) => flat.extend_from_slice(x),
                        _ => flat.push(0.0),
                    }
                }
                while flat.len() < 4 {
                    flat.push(0.0);
                }
                match size {
                    VectorSize::Bi => Val::Vec2([flat[0], flat[1]]),
                    VectorSize::Tri => Val::Vec3([flat[0], flat[1], flat[2]]),
                    VectorSize::Quad => Val::Vec4([flat[0], flat[1], flat[2], flat[3]]),
                }
            }
            TypeInner::Struct { .. } => Val::Struct(vals.to_vec()),
            _ => vals.first().cloned().unwrap_or(Val::Scalar(0.0)),
        }
    }

    fn eval_binary(&self, op: BinaryOperator, l: &Val, r: &Val) -> Val {
        let bool_f = |b: bool| if b { 1.0 } else { 0.0 };
        l.map_binary(r, |a, b| match op {
            BinaryOperator::Add => a + b,
            BinaryOperator::Subtract => a - b,
            BinaryOperator::Multiply => a * b,
            BinaryOperator::Divide => {
                if b.abs() < 1e-20 {
                    0.0
                } else {
                    a / b
                }
            }
            BinaryOperator::Modulo => {
                if b.abs() < 1e-20 {
                    0.0
                } else {
                    a % b
                }
            }
            BinaryOperator::Equal => bool_f((a - b).abs() <= f32::EPSILON),
            BinaryOperator::NotEqual => bool_f((a - b).abs() > f32::EPSILON),
            BinaryOperator::Less => bool_f(a < b),
            BinaryOperator::LessEqual => bool_f(a <= b),
            BinaryOperator::Greater => bool_f(a > b),
            BinaryOperator::GreaterEqual => bool_f(a >= b),
            BinaryOperator::And => ((a as i32) & (b as i32)) as f32,
            BinaryOperator::ExclusiveOr => ((a as i32) ^ (b as i32)) as f32,
            BinaryOperator::InclusiveOr => ((a as i32) | (b as i32)) as f32,
            BinaryOperator::LogicalAnd => bool_f(a != 0.0 && b != 0.0),
            BinaryOperator::LogicalOr => bool_f(a != 0.0 || b != 0.0),
            BinaryOperator::ShiftLeft => ((a as u32).wrapping_shl(b as u32)) as f32,
            BinaryOperator::ShiftRight => ((a as u32).wrapping_shr(b as u32)) as f32,
        })
    }

    fn eval_math(
        &self,
        fun: MathFunction,
        a0: &Val,
        a1: Option<&Val>,
        a2: Option<&Val>,
    ) -> Val {
        match fun {
            MathFunction::Abs => a0.map_unary(f32::abs),
            MathFunction::Min => {
                let b = a1.cloned().unwrap_or(Val::Scalar(0.0));
                a0.map_binary(&b, f32::min)
            }
            MathFunction::Max => {
                let b = a1.cloned().unwrap_or(Val::Scalar(0.0));
                a0.map_binary(&b, f32::max)
            }
            MathFunction::Clamp => {
                let lo = a1.cloned().unwrap_or(Val::Scalar(0.0));
                let hi = a2.cloned().unwrap_or(Val::Scalar(1.0));
                a0.map_ternary(&lo, &hi, |x, l, h| x.clamp(l.min(h), h.max(l)))
            }
            MathFunction::Saturate => a0.map_unary(|x| x.clamp(0.0, 1.0)),
            MathFunction::Cos => a0.map_unary(f32::cos),
            MathFunction::Sin => a0.map_unary(f32::sin),
            MathFunction::Tan => a0.map_unary(f32::tan),
            MathFunction::Acos => a0.map_unary(|x| x.clamp(-1.0, 1.0).acos()),
            MathFunction::Asin => a0.map_unary(|x| x.clamp(-1.0, 1.0).asin()),
            MathFunction::Atan => a0.map_unary(f32::atan),
            MathFunction::Atan2 => {
                let b = a1.cloned().unwrap_or(Val::Scalar(1.0));
                a0.map_binary(&b, f32::atan2)
            }
            MathFunction::Sinh => a0.map_unary(f32::sinh),
            MathFunction::Cosh => a0.map_unary(f32::cosh),
            MathFunction::Tanh => a0.map_unary(f32::tanh),
            MathFunction::Sqrt => a0.map_unary(|x| x.max(0.0).sqrt()),
            MathFunction::InverseSqrt => a0.map_unary(|x| 1.0 / x.max(1e-20).sqrt()),
            MathFunction::Exp => a0.map_unary(f32::exp),
            MathFunction::Exp2 => a0.map_unary(f32::exp2),
            MathFunction::Log => a0.map_unary(|x| x.max(1e-20).ln()),
            MathFunction::Log2 => a0.map_unary(|x| x.max(1e-20).log2()),
            MathFunction::Pow => {
                let b = a1.cloned().unwrap_or(Val::Scalar(1.0));
                a0.map_binary(&b, f32::powf)
            }
            MathFunction::Sign => a0.map_unary(|x| {
                if x > 0.0 {
                    1.0
                } else if x < 0.0 {
                    -1.0
                } else {
                    0.0
                }
            }),
            MathFunction::Fract => a0.map_unary(|x| x - x.floor()),
            MathFunction::Trunc => a0.map_unary(f32::trunc),
            MathFunction::Floor => a0.map_unary(f32::floor),
            MathFunction::Ceil => a0.map_unary(f32::ceil),
            MathFunction::Round => a0.map_unary(f32::round),
            MathFunction::Step => {
                let x = a1.cloned().unwrap_or(Val::Scalar(0.0));
                a0.map_binary(&x, |edge, v| if v < edge { 0.0 } else { 1.0 })
            }
            MathFunction::Mix => {
                let y = a1.cloned().unwrap_or(Val::Scalar(0.0));
                let s = a2.cloned().unwrap_or(Val::Scalar(0.0));
                a0.map_ternary(&y, &s, |x, yv, t| x + (yv - x) * t)
            }
            MathFunction::SmoothStep => {
                let e1 = a1.cloned().unwrap_or(Val::Scalar(1.0));
                let x = a2.cloned().unwrap_or(Val::Scalar(0.0));
                a0.map_ternary(&e1, &x, |edge0, edge1, v| {
                    let span = (edge1 - edge0).max(1e-12);
                    let t = ((v - edge0) / span).clamp(0.0, 1.0);
                    t * t * (3.0 - 2.0 * t)
                })
            }
            MathFunction::Fma => {
                let b = a1.cloned().unwrap_or(Val::Scalar(0.0));
                let c = a2.cloned().unwrap_or(Val::Scalar(0.0));
                a0.map_ternary(&b, &c, |x, y, z| x.mul_add(y, z))
            }
            MathFunction::Dot => {
                let b = a1.cloned().unwrap_or(Val::Scalar(0.0));
                let sum = match (a0, &b) {
                    (Val::Vec2(u), Val::Vec2(v)) => u[0] * v[0] + u[1] * v[1],
                    (Val::Vec3(u), Val::Vec3(v)) => u[0] * v[0] + u[1] * v[1] + u[2] * v[2],
                    (Val::Vec4(u), Val::Vec4(v)) => {
                        u[0] * v[0] + u[1] * v[1] + u[2] * v[2] + u[3] * v[3]
                    }
                    _ => a0.as_f32() * b.as_f32(),
                };
                Val::Scalar(sum)
            }
            MathFunction::Cross => {
                let u = a0.as_vec3();
                let v = a1.map(Val::as_vec3).unwrap_or([0.0; 3]);
                Val::Vec3([
                    u[1] * v[2] - u[2] * v[1],
                    u[2] * v[0] - u[0] * v[2],
                    u[0] * v[1] - u[1] * v[0],
                ])
            }
            MathFunction::Length => {
                let sq = match a0 {
                    Val::Scalar(x) => x * x,
                    Val::Vec2(v) => v[0] * v[0] + v[1] * v[1],
                    Val::Vec3(v) => v[0] * v[0] + v[1] * v[1] + v[2] * v[2],
                    Val::Vec4(v) => v[0] * v[0] + v[1] * v[1] + v[2] * v[2] + v[3] * v[3],
                    _ => 0.0,
                };
                Val::Scalar(sq.sqrt())
            }
            MathFunction::Distance => {
                let b = a1.cloned().unwrap_or(Val::Scalar(0.0));
                let diff = self.eval_binary(BinaryOperator::Subtract, a0, &b);
                self.eval_math(MathFunction::Length, &diff, None, None)
            }
            MathFunction::Normalize => {
                let len = self
                    .eval_math(MathFunction::Length, a0, None, None)
                    .as_f32()
                    .max(1e-20);
                a0.map_unary(|x| x / len)
            }
            MathFunction::Reflect => {
                let n = a1.cloned().unwrap_or(Val::Scalar(0.0));
                let d = self
                    .eval_math(MathFunction::Dot, a0, Some(&n), None)
                    .as_f32();
                a0.map_binary(&n, |iv, nv| iv - 2.0 * d * nv)
            }
            _ => a0.clone(),
        }
    }

    #[inline]
    fn texel_at(&self, x: u32, y: u32) -> [f32; 4] {
        let idx = ((y * self.width + x) as usize) * 4;
        if idx + 4 > self.input_rgba.len() {
            return [0.0, 0.0, 0.0, 0.0];
        }
        [
            f32::from(self.input_rgba[idx]) / 255.0,
            f32::from(self.input_rgba[idx + 1]) / 255.0,
            f32::from(self.input_rgba[idx + 2]) / 255.0,
            f32::from(self.input_rgba[idx + 3]) / 255.0,
        ]
    }

    /// Bilinear texture sampling with ClampToEdge addressing, matching `wgpu`
    /// `FilterMode::Linear` + `AddressMode::ClampToEdge`.
    fn sample_bilinear(&self, u: f32, v: f32) -> [f32; 4] {
        if self.width == 0 || self.height == 0 {
            return [0.0, 0.0, 0.0, 0.0];
        }
        let u = u.clamp(0.0, 1.0);
        let v = v.clamp(0.0, 1.0);
        let fx = (u * (self.width as f32) - 0.5).clamp(0.0, (self.width - 1) as f32);
        let fy = (v * (self.height as f32) - 0.5).clamp(0.0, (self.height - 1) as f32);

        let x0 = fx.floor() as u32;
        let y0 = fy.floor() as u32;
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);
        let tx = fx - (x0 as f32);
        let ty = fy - (y0 as f32);

        let c00 = self.texel_at(x0, y0);
        let c10 = self.texel_at(x1, y0);
        let c01 = self.texel_at(x0, y1);
        let c11 = self.texel_at(x1, y1);

        let mut out = [0.0f32; 4];
        for i in 0..4 {
            let top = c00[i] + (c10[i] - c00[i]) * tx;
            let bot = c01[i] + (c11[i] - c01[i]) * tx;
            out[i] = top + (bot - top) * ty;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executes_grain_shader_on_cpu_deterministically() {
        let wgsl = r#"
struct Grain {
    amount: f32,
    tint: vec3<f32>,
};
@group(1) @binding(0) var<uniform> grain: Grain;

fn hash(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let base = textureSample(kine_input, kine_sampler, uv);
    let n = (hash(uv * kine_frame.resolution + kine_frame.frame) - 0.5) * grain.amount;
    return vec4<f32>(base.rgb * grain.tint + n, base.a);
}
"#;
        let uniforms: Map<String, Value> =
            serde_json::from_str(r#"{"amount": 0.2, "tint": [1.0, 0.9, 0.8]}"#).unwrap();
        let prog = CpuShaderProgram::compile(wgsl, &uniforms).expect("compile grain");

        let mut fb1 = vec![128u8; 16 * 16 * 4];
        let mut fb2 = fb1.clone();
        let mut fb_frame2 = fb1.clone();

        prog.execute_on_premul_rgba(
            &mut fb1,
            16,
            16,
            (0, 0, 16, 16),
            0.5,
            0,
            CpuBlendMode::Normal,
            1.0,
        );
        prog.execute_on_premul_rgba(
            &mut fb2,
            16,
            16,
            (0, 0, 16, 16),
            0.5,
            0,
            CpuBlendMode::Normal,
            1.0,
        );
        prog.execute_on_premul_rgba(
            &mut fb_frame2,
            16,
            16,
            (0, 0, 16, 16),
            0.5,
            1,
            CpuBlendMode::Normal,
            1.0,
        );

        assert_eq!(fb1, fb2, "CPU WGSL execution must be deterministic");
        assert_ne!(
            fb1,
            vec![128u8; 16 * 16 * 4],
            "shader must modify the framebuffer"
        );
        assert_ne!(
            fb1, fb_frame2,
            "different frame index must change kine_frame.frame output"
        );
    }

    #[test]
    fn respects_scissor_rectangle() {
        let wgsl = r#"
@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(1.0, 0.0, 0.0, 1.0);
}
"#;
        let prog = CpuShaderProgram::compile(wgsl, &Map::new()).unwrap();
        let mut fb = vec![0u8; 8 * 8 * 4];
        prog.execute_on_premul_rgba(
            &mut fb,
            8,
            8,
            (2, 2, 4, 4),
            0.0,
            0,
            CpuBlendMode::Normal,
            1.0,
        );
        // Pixel (0, 0) outside scissor remains untouched
        assert_eq!(&fb[0..4], &[0, 0, 0, 0]);
        // Pixel (3, 3) inside scissor is pure red
        let idx = (3 * 8 + 3) * 4;
        assert_eq!(&fb[idx..idx + 4], &[255, 0, 0, 255]);
    }
}
