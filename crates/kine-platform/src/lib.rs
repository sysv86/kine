//! Platform detection and storage policy for Linux, Windows, and macOS.
//!
//! Kine targets **Linux** (`x86_64`, `aarch64`), **Windows** (`x86_64`,
//! `aarch64`), and **macOS** (`aarch64` Apple Silicon, `x86_64`). All core
//! subsystems — scene layout (`taffy`), text shaping (`cosmic-text`), CPU
//! rasterization (`vello_cpu`), CPU WGSL shader VM (`naga`), audio
//! synthesis/DSP/mixing (`symphonia`, `rubato`, `flacenc`, `hound`), container
//! muxing (`muxfin`, `mkv`), and the Xilem UI (`kine-studio`) — execute
//! deterministically across all three operating systems without OS-specific
//! divergence. AV1 and H.264 encoding run in an external FFmpeg process.
//!
//! Optional GPU acceleration uses `wgpu` + `vello_gpu` (Vulkan on Linux,
//! DirectX 12 / Vulkan on Windows, Metal on macOS).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Host operating system classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PlatformKind {
    /// Linux (desktop, server, container, or CI runner).
    Linux,
    /// Windows (desktop, workstation, or CI runner).
    Windows,
    /// macOS (Apple Silicon or Intel).
    MacOs,
    /// Any other host OS where the portable CPU pipeline runs.
    Other,
}

impl PlatformKind {
    pub const fn is_linux(self) -> bool {
        matches!(self, Self::Linux)
    }

    pub const fn is_windows(self) -> bool {
        matches!(self, Self::Windows)
    }

    pub const fn is_macos(self) -> bool {
        matches!(self, Self::MacOs)
    }

    /// Primary `wgpu` graphics backend expected on this OS.
    pub const fn graphics_backend(self) -> &'static str {
        match self {
            Self::Linux => "vulkan",
            Self::Windows => "dx12",
            Self::MacOs => "metal",
            Self::Other => "wgpu",
        }
    }
}

/// Raw, injectable observations about the host environment.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    /// Target OS string (e.g. `"linux"`, `"windows"`, `"macos"`). When empty, defaults to `std::env::consts::OS`.
    pub os: String,
    /// Target architecture string (e.g. `"x86_64"`, `"aarch64"`). When empty, defaults to `std::env::consts::ARCH`.
    pub arch: String,
    /// Current working directory of the process.
    pub cwd: Option<PathBuf>,
    /// Relevant environment variables (`HOME`, `USERPROFILE`, `KINE_OUTPUT_DIR`, `CI`, `GITHUB_ACTIONS`, etc.).
    pub env: BTreeMap<String, String>,
    /// Whether Linux DRM render nodes (`/dev/dri/renderD128` or `/dev/dri/card0`) exist.
    pub linux_drm_device_present: bool,
}

impl Facts {
    /// Observe the real process environment and filesystem.
    pub fn collect() -> Self {
        let mut env = BTreeMap::new();
        for key in [
            "HOME",
            "USERPROFILE",
            "KINE_OUTPUT_DIR",
            "VK_ICD_FILENAMES",
            "VK_DRIVER_FILES",
            "KINE_ALLOW_SOFTWARE_GPU",
            "CI",
            "GITHUB_ACTIONS",
        ] {
            if let Ok(v) = std::env::var(key) {
                env.insert(key.to_string(), v);
            }
        }

        let exists = |p: &str| Path::new(p).exists();
        let linux_drm_device_present = exists("/dev/dri/renderD128") || exists("/dev/dri/card0");

        Self {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            cwd: std::env::current_dir().ok(),
            env,
            linux_drm_device_present,
        }
    }

    fn has(&self, key: &str) -> bool {
        self.env.get(key).is_some_and(|v| !v.is_empty())
    }

    fn is_ci(&self) -> bool {
        self.has("CI") || self.has("GITHUB_ACTIONS")
    }
}

/// Classified platform description, suitable for `kine host` and `kine doctor`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PlatformInfo {
    pub os: String,
    pub arch: String,
    pub kind: PlatformKind,
    /// Primary GPU backend on this platform (`"vulkan"`, `"dx12"`, `"metal"`, or `"wgpu"`).
    pub graphics_backend: &'static str,
    /// Running inside an automated CI environment (`CI` or `GITHUB_ACTIONS` set).
    pub ci: bool,
    /// Where relative output paths resolve to (`KINE_OUTPUT_DIR`, or current working directory).
    pub output_dir: PathBuf,
    /// Whether a GPU / hardware encoder can be expected on this host.
    pub gpu_expected: bool,
    /// Human-readable diagnostics (`doctor` and `host` print these verbatim).
    pub notes: Vec<String>,
}

impl PlatformInfo {
    /// Classify collected [`Facts`].
    pub fn from_facts(facts: &Facts) -> Self {
        let os_str = if facts.os.is_empty() {
            std::env::consts::OS
        } else {
            facts.os.as_str()
        };
        let arch_str = if facts.arch.is_empty() {
            std::env::consts::ARCH
        } else {
            facts.arch.as_str()
        };

        let kind = match os_str {
            "linux" => PlatformKind::Linux,
            "windows" => PlatformKind::Windows,
            "macos" => PlatformKind::MacOs,
            _ => PlatformKind::Other,
        };

        let ci = facts.is_ci();
        let output_dir = default_output_dir(facts);
        let graphics_backend = kind.graphics_backend();

        let sw_gpu = facts
            .env
            .get("KINE_ALLOW_SOFTWARE_GPU")
            .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
        let vk_icd = facts.has("VK_ICD_FILENAMES") || facts.has("VK_DRIVER_FILES");

        let gpu_expected = match kind {
            PlatformKind::Linux => facts.linux_drm_device_present || vk_icd || sw_gpu,
            PlatformKind::Windows | PlatformKind::MacOs => true,
            PlatformKind::Other => false,
        };

        let mut notes = Vec::new();
        match kind {
            PlatformKind::Linux => {
                if facts.linux_drm_device_present {
                    notes.push(
                        "Linux host with DRM device (/dev/dri): Vulkan GPU rasterization supported when built with `--features gpu`."
                            .to_string(),
                    );
                } else if vk_icd || sw_gpu {
                    notes.push(
                        "Linux host without /dev/dri, but explicit Vulkan ICD or software GPU enabled."
                            .to_string(),
                    );
                } else {
                    notes.push(
                        "Linux headless host (no /dev/dri): deterministic CPU rasterizer (`vello_cpu`) + CPU WGSL VM active."
                            .to_string(),
                    );
                }
            }
            PlatformKind::Windows => {
                notes.push(
                    "Windows host: DirectX 12 / Vulkan GPU rasterization (`wgpu`) supported alongside the deterministic CPU pipeline."
                        .to_string(),
                );
            }
            PlatformKind::MacOs => {
                notes.push(
                    "macOS host: Metal GPU rasterization (`wgpu`) supported alongside the deterministic CPU pipeline."
                        .to_string(),
                );
            }
            PlatformKind::Other => {
                notes.push(format!(
                    "Unclassified OS ({os_str}): using portable deterministic CPU pipeline."
                ));
            }
        }
        notes.push(format!("Outputs resolve to {}", output_dir.display()));

        Self {
            os: os_str.to_string(),
            arch: arch_str.to_string(),
            kind,
            graphics_backend,
            ci,
            output_dir,
            gpu_expected,
            notes,
        }
    }

    /// Observe and classify in one step.
    pub fn detect() -> Self {
        Self::from_facts(&Facts::collect())
    }
}

fn default_output_dir(facts: &Facts) -> PathBuf {
    if let Some(custom) = facts.env.get("KINE_OUTPUT_DIR") {
        if !custom.is_empty() {
            return PathBuf::from(custom);
        }
    }
    if let Some(cwd) = &facts.cwd {
        if !cwd.as_os_str().is_empty() {
            return cwd.clone();
        }
    }
    if let Some(home) = facts.env.get("HOME") {
        if !home.is_empty() {
            return PathBuf::from(home);
        }
    }
    if let Some(profile) = facts.env.get("USERPROFILE") {
        if !profile.is_empty() {
            return PathBuf::from(profile);
        }
    }
    PathBuf::from(".")
}

/// Resolve a user-supplied output path.
pub fn resolve_output(requested: &Path, info: &PlatformInfo) -> PathBuf {
    if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        info.output_dir.join(requested)
    }
}

/// Write bytes to `path`, creating parent directories and cleaning up on failure.
pub fn write_output(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    match std::fs::write(path, bytes) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(path);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts_for(os: &str, arch: &str, pairs: &[(&str, &str)]) -> Facts {
        Facts {
            os: os.to_string(),
            arch: arch.to_string(),
            cwd: Some(PathBuf::from("/work/project")),
            env: pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            linux_drm_device_present: false,
        }
    }

    #[test]
    fn linux_desktop_with_drm_expects_vulkan_gpu() {
        let mut facts = facts_for("linux", "x86_64", &[("HOME", "/home/dev")]);
        facts.linux_drm_device_present = true;
        let info = PlatformInfo::from_facts(&facts);
        assert_eq!(info.kind, PlatformKind::Linux);
        assert!(info.kind.is_linux());
        assert!(!info.kind.is_windows());
        assert!(!info.kind.is_macos());
        assert_eq!(info.graphics_backend, "vulkan");
        assert!(info.gpu_expected);
        assert!(!info.ci);
        assert_eq!(info.output_dir, PathBuf::from("/work/project"));
    }

    #[test]
    fn linux_headless_ci_uses_cpu_when_no_drm_present() {
        let facts = facts_for(
            "linux",
            "x86_64",
            &[("HOME", "/home/runner"), ("GITHUB_ACTIONS", "true")],
        );
        let info = PlatformInfo::from_facts(&facts);
        assert_eq!(info.kind, PlatformKind::Linux);
        assert!(info.ci);
        assert!(!info.gpu_expected);
        assert!(info.notes.iter().any(|n| n.contains("headless")));
    }

    #[test]
    fn windows_host_reports_dx12() {
        let facts = facts_for("windows", "x86_64", &[("USERPROFILE", "C:\\Users\\runner")]);
        let info = PlatformInfo::from_facts(&facts);
        assert_eq!(info.kind, PlatformKind::Windows);
        assert!(info.kind.is_windows());
        assert!(!info.kind.is_linux());
        assert!(!info.kind.is_macos());
        assert_eq!(info.graphics_backend, "dx12");
        assert!(info.gpu_expected);
        assert!(info.notes.iter().any(|n| n.contains("DirectX 12")));
    }

    #[test]
    fn macos_apple_silicon_reports_metal() {
        let facts = facts_for("macos", "aarch64", &[("HOME", "/Users/runner")]);
        let info = PlatformInfo::from_facts(&facts);
        assert_eq!(info.kind, PlatformKind::MacOs);
        assert!(info.kind.is_macos());
        assert!(!info.kind.is_linux());
        assert_eq!(info.graphics_backend, "metal");
        assert!(info.gpu_expected);
        assert!(info.notes.iter().any(|n| n.contains("Metal")));
    }

    #[test]
    fn kine_output_dir_env_overrides_cwd() {
        let facts = facts_for(
            "linux",
            "x86_64",
            &[("KINE_OUTPUT_DIR", "/tmp/kine-renders")],
        );
        let info = PlatformInfo::from_facts(&facts);
        assert_eq!(info.output_dir, PathBuf::from("/tmp/kine-renders"));
    }

    #[test]
    fn resolve_output_honours_absolute_and_prefixes_relative() {
        let info = PlatformInfo::from_facts(&facts_for("macos", "aarch64", &[]));
        assert_eq!(
            resolve_output(Path::new("/tmp/a.png"), &info),
            PathBuf::from("/tmp/a.png")
        );
        assert_eq!(
            resolve_output(Path::new("frame.png"), &info),
            PathBuf::from("/work/project/frame.png")
        );
    }

    #[test]
    fn write_output_creates_parents_and_removes_partial_files() {
        let dir = std::env::temp_dir().join(format!("kine-platform-{}", std::process::id()));
        let nested = dir.join("a").join("b").join("out.bin");
        write_output(&nested, b"hello").expect("write");
        assert_eq!(std::fs::read(&nested).unwrap(), b"hello");

        let as_dir = dir.join("a").join("b");
        assert!(write_output(&as_dir, b"x").is_err());
        assert!(as_dir.is_dir(), "existing directory must survive");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
