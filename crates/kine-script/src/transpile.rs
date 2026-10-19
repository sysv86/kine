//! TypeScript → JavaScript via oxc: parse, semantic pass, TS transform, codegen.
//!
//! Only the TypeScript constructs are transformed (type annotations,
//! interfaces, `as` casts, `as const`); no React/JSX, no downleveling — QuickJS
//! supports modern ECMAScript natively.

use std::path::Path;

use oxc::allocator::Allocator;
use oxc::codegen::{Codegen, CodegenOptions};
use oxc::parser::{ParseOptions, Parser};
use oxc::semantic::SemanticBuilder;
use oxc::span::SourceType;
use oxc::transformer::{HelperLoaderMode, TransformOptions, Transformer};

use crate::ScriptError;

/// Transpile TypeScript source into plain JavaScript.
///
/// Scripts are *function bodies*: a top-level `return scene(...)` is expected,
/// so `return` outside a function is allowed at parse time.
pub fn transpile_ts(src: &str) -> Result<String, ScriptError> {
    let allocator = Allocator::default();
    let source_type = SourceType::ts();

    let parse_options = ParseOptions {
        allow_return_outside_function: true,
        ..ParseOptions::default()
    };
    let parsed = Parser::new(&allocator, src, source_type)
        .with_options(parse_options)
        .parse();
    if !parsed.diagnostics.is_empty() {
        let msgs: Vec<String> = parsed
            .diagnostics
            .iter()
            .map(|d| d.to_string())
            .collect();
        return Err(ScriptError::Transpile(msgs.join("; ")));
    }

    let mut program = parsed.program;

    let semantic = SemanticBuilder::new()
        .with_excess_capacity(2.0)
        .build(&program);
    if !semantic.diagnostics.is_empty() {
        let msgs: Vec<String> = semantic.diagnostics.iter().map(|d| d.to_string()).collect();
        return Err(ScriptError::Transpile(msgs.join("; ")));
    }
    let scoping = semantic.semantic.into_scoping();

    let mut options = TransformOptions::default();
    // Types are always stripped; nothing else is enabled (QuickJS handles the
    // rest of the language natively).
    options.typescript = oxc::transformer::TypeScriptOptions::default();
    options.helper_loader.mode = HelperLoaderMode::External;

    let path = Path::new("scene.ts");
    let transformed = Transformer::new(&allocator, path, &options).build_with_scoping(scoping, &mut program);
    if !transformed.diagnostics.is_empty() {
        let msgs: Vec<String> = transformed
            .diagnostics
            .iter()
            .map(|d| d.to_string())
            .collect();
        return Err(ScriptError::Transpile(msgs.join("; ")));
    }

    let codegen_options = CodegenOptions {
        ..CodegenOptions::default()
    };
    let code = Codegen::new()
        .with_options(codegen_options)
        .build(&program)
        .code;
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_type_annotations() {
        let src = r#"
            interface Point { x: number; y: number }
            const origin: Point = { x: 0, y: 0 };
            const label = "kine" as const;
            const scale = (p: Point, k: number): Point => ({ x: p.x * k, y: p.y * k });
            const doubled = scale(origin, 2);
            doubled.x + label.length;
        "#;
        let js = transpile_ts(src).expect("transpile");
        assert!(!js.contains("interface"), "interface survived: {js}");
        assert!(!js.contains(": Point"), "type annotation survived: {js}");
        assert!(!js.contains("as const"), "as const survived: {js}");
        assert!(js.contains("scale"), "runtime code lost: {js}");
    }

    #[test]
    fn reports_syntax_errors() {
        let src = "const x: = 1;";
        let err = transpile_ts(src).unwrap_err();
        assert!(matches!(err, ScriptError::Transpile(_)), "got {err:?}");
    }

    #[test]
    fn transpiled_output_runs_in_quickjs() {
        // TypeScript features are erased before QuickJS sees the code.
        let src = r#"
            interface Box { w: number }
            const b: Box = { w: 7 };
            b.w;
        "#;
        let js = transpile_ts(src).unwrap();
        let rt = rquickjs::Runtime::new().unwrap();
        let ctx = rquickjs::Context::full(&rt).unwrap();
        let value: i32 = ctx.with(|ctx| ctx.eval(js).unwrap());
        assert_eq!(value, 7);
    }
}
