//! End-to-end: TypeScript source → scene → layout → rasterized PNG.

use kine_render::{png, FrameRenderer};

const FONT: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");

const SCENE_TS: &str = r##"
interface Card { w: number; h: number }

const card: Card = { w: 200, h: 80 };
const accent = "#5ee0c0" as const;

return scene(320, 200,
  container({ size: { width: 320, height: 200 } }, [
    rect({ fill: "#121829", size: { width: 320, height: 200 }, position: "absolute", inset: { top: 0, left: 0 } }),
    rect({ fill: accent, cornerRadius: 12, size: { width: card.w, height: card.h }, inset: { top: 60, left: 60 } }),
    text("script ok", {
      fontFamily: "JetBrains Mono",
      fontSize: 24,
      color: "#0b1220",
      align: "center",
      size: { width: card.w, height: 40 },
      inset: { top: 80, left: 60 }
    })
  ])
);
"##;

#[test]
fn typescript_scene_renders_to_png() {
    let mut scene = kine_script::eval_scene_ts(SCENE_TS).expect("eval scene");
    scene.compute_layout().expect("layout");

    let mut renderer = FrameRenderer::new();
    renderer.load_font("JetBrains Mono", FONT.to_vec());

    let pixmap = renderer.render(&scene).expect("render");
    let bytes = png::pixmap_to_png(&pixmap).expect("png");

    assert_eq!(&bytes[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);

    // The accent card spans x∈[60,260), y∈[60,140): its centre must be opaque.
    let px = pixmap.data()[100 * 320 + 100];
    assert_eq!(px.a, 255, "card centre should be opaque, got {px:?}");
}
