//! Ready-made modules: a project's `Modules/` directory is discovered on scene
//! load and its factories become callable from the scene as `module()`.

use kine_server::Engine;
use std::fs;
use std::path::{Path, PathBuf};

const FONT: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "kine-module-test-{tag}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch");
    dir
}

fn write_module(project: &Path, name: &str, meta: &str, source: &str) {
    let dir = project.join("Modules").join(name);
    fs::create_dir_all(&dir).expect("module dir");
    fs::write(dir.join("module.json"), meta).expect("module.json");
    fs::write(dir.join("scene.ts"), source).expect("scene.ts");
}

fn module_scene() -> &'static str {
    r##"
// A lower third built once and reused from the module library.
return (props) => container(
  {
    position: "absolute",
    inset: { left: 40, bottom: 40 },
    size: { width: 420, height: 72 },
  },
  [
    rect({ fill: props.accent || "#f5b642", size: { width: 8, height: 72 } }),
    text(props.title || "UNTITLED", {
      fontFamily: "JetBrains Mono",
      fontSize: 34,
      color: "#ffffff",
      position: "absolute",
      inset: { left: 24, top: 8 },
      size: { width: 380, height: 44 },
    }),
    text(props.subtitle || "", {
      fontFamily: "JetBrains Mono",
      fontSize: 18,
      color: "#aab6c8",
      position: "absolute",
      inset: { left: 24, top: 48 },
      size: { width: 380, height: 24 },
    }),
  ]
);
"##
}

fn scene_using_module() -> &'static str {
    r##"
return scene(640, 360,
  container({ size: { width: 640, height: 360 } }, [
    rect({
      fill: "#0d1220",
      position: "absolute",
      inset: { top: 0, left: 0, right: 0, bottom: 0 },
    }),
    module("lower-third", { title: "MODULE OK", subtitle: "ready-made element" }),
  ])
);
"##
}

#[test]
fn a_scene_can_use_a_module_from_the_project_folder() {
    let project = scratch("happy");
    write_module(
        &project,
        "lower-third",
        r##"{"kind":"subtitle","description":"Broadcast lower third","params":{"title":"UNTITLED"}}"##,
        module_scene(),
    );

    let mut engine = Engine::new();
    engine.load_font("JetBrains Mono", FONT.to_vec());
    let handle = engine
        .load_scene_json_with_base(scene_using_module(), &project)
        .expect("scene with module");

    let png = engine.render_frame_png(handle, 0).expect("frame");
    assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);

    let report = engine.inspect(handle).expect("inspect");
    assert!(
        report
            .findings
            .iter()
            .all(|f| f.severity != kine_server::FindingSeverity::Error),
        "{:?}",
        report.findings
    );

    let _ = fs::remove_dir_all(&project);
}

#[test]
fn an_unknown_module_names_what_is_available() {
    let project = scratch("unknown");
    write_module(&project, "lower-third", "{}", module_scene());
    let scene = r##"
        return scene(64, 64, container({ size: { width: 64, height: 64 } }, [
            module("does-not-exist")
        ]));
    "##;

    let mut engine = Engine::new();
    let err = engine
        .load_scene_json_with_base(scene, &project)
        .expect_err("unknown module must fail");
    let message = err.to_string();
    assert!(message.contains("unknown module"), "{message}");
    assert!(message.contains("lower-third"), "{message}");

    let _ = fs::remove_dir_all(&project);
}

#[test]
fn a_module_without_an_entry_script_fails_discovery() {
    let project = scratch("broken");
    let dir = project.join("Modules").join("no-entry");
    fs::create_dir_all(&dir).expect("module dir");
    fs::write(dir.join("module.json"), "{}").expect("module.json");

    let mut engine = Engine::new();
    let err = engine
        .load_scene_json_with_base("return scene(10, 10, container({}, []));", &project)
        .expect_err("broken module must fail");
    assert!(err.to_string().contains("missing scene.ts"), "{err}");

    let _ = fs::remove_dir_all(&project);
}

#[test]
fn a_module_can_be_parameterised_per_call_site() {
    let project = scratch("props");
    write_module(&project, "badge", "{}", module_scene());

    let scene = r##"
        return scene(320, 180, container({ size: { width: 320, height: 180 } }, [
            module("badge", { title: "LEFT", subtitle: "one" }),
            module("badge", { title: "RIGHT", subtitle: "two", accent: "#4fd1c5" }),
        ]));
    "##;
    let mut engine = Engine::new();
    engine.load_font("JetBrains Mono", FONT.to_vec());
    let handle = engine
        .load_scene_json_with_base(scene, &project)
        .expect("two module instances");
    assert_eq!(engine.scene(handle).expect("scene").nodes.len(), 9);

    let _ = fs::remove_dir_all(&project);
}
