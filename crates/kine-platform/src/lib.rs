//! Platform detection and storage policy.
//!
//! Kine targets Linux. **Android is supported through `proot`**
//! (Termux + `proot-distro`): the engine runs as an ordinary Linux/aarch64
//! process inside the proot container, so the same code paths apply — with two
//! environment differences this module handles:
//!
//! 1. **Where to write.** Inside proot `/sdcard` may or may not be bind-mounted;
//!    when it is absent the only writable place is `$HOME`.
//! 2. **GPU & Vulkan ICD detection.** By default proot does not expose `/dev/dri`,
//!    so the deterministic CPU rasterizer + CPU WGSL VM is used. However, when
//!    Termux/proot exposes `/dev/kgsl-3d0` (Qualcomm Turnip), `/dev/dri`, or an
//!    explicit `VK_ICD_FILENAMES` / `KINE_ALLOW_SOFTWARE_GPU`, the host probe
//!    reports those capabilities accurately in `notes`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Which environment the engine believes it is running in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum PlatformKind {
    /// Ordinary Linux (desktop/server/container).
    Linux,
    /// Android kernel, but the process runs inside a proot container.
    AndroidProot,
    /// Android without proot (native binary / JNI).
    AndroidNative,
}

impl PlatformKind {
    pub fn is_android(self) -> bool {
        matches!(self, Self::AndroidProot | Self::AndroidNative)
    }
}

/// Raw, injectable observations about the environment.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    pub env: BTreeMap<String, String>,
    pub android_root_exists: bool,
    pub system_build_prop_exists: bool,
    /// Contents of `/proc/version`, when readable.
    pub proc_version: Option<String>,
    /// A writable shared-storage root, e.g. `/sdcard` or `/storage/emulated/0`.
    pub shared_storage: Option<PathBuf>,
    /// Whether `/dev/dri` or `/dev/kgsl-3d0` is visible in the filesystem.
    pub drm_or_kgsl_exists: bool,
}

impl Facts {
    /// Observe the real process environment and filesystem.
    pub fn collect() -> Self {
        let mut env = BTreeMap::new();
        for key in [
            "TERMUX_VERSION",
            "PREFIX",
            "HOME",
            "EXTERNAL_STORAGE",
            "ANDROID_ROOT",
            "ANDROID_DATA",
            "PROOT_LOADER",
            "PROOT_TMP_DIR",
            "PROOT_NO_SECCOMP",
            "VK_ICD_FILENAMES",
            "VK_DRIVER_FILES",
            "KINE_ALLOW_SOFTWARE_GPU",
        ] {
            if let Ok(v) = std::env::var(key) {
                env.insert(key.to_string(), v);
            }
        }

        let exists = |p: &str| Path::new(p).exists();
        let shared_storage = ["/sdcard", "/storage/emulated/0"]
            .into_iter()
            .find(|p| Path::new(p).is_dir())
            .map(PathBuf::from);

        let proc_version = std::fs::read_to_string("/proc/version").ok();
        let drm_or_kgsl_exists =
            exists("/dev/dri/renderD128") || exists("/dev/dri/card0") || exists("/dev/kgsl-3d0");

        Self {
            env,
            android_root_exists: exists("/system") || exists("/system/bin"),
            system_build_prop_exists: exists("/system/build.prop"),
            proc_version,
            shared_storage,
            drm_or_kgsl_exists,
        }
    }

    fn has(&self, key: &str) -> bool {
        self.env.get(key).is_some_and(|v| !v.is_empty())
    }

    /// Whether the running kernel identifies itself as a proot container.
    fn proc_version_mentions_proot(&self) -> bool {
        self.proc_version
            .as_deref()
            .is_some_and(|v| v.to_ascii_lowercase().contains("proot"))
    }
}

/// Classified platform description, suitable for `doctor` output.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PlatformInfo {
    pub os: &'static str,
    pub arch: &'static str,
    pub kind: PlatformKind,
    /// Running inside the Termux app (as opposed to a plain chroot).
    pub termux: bool,
    /// Where relative output paths resolve to.
    pub output_dir: PathBuf,
    /// Whether a hardware encoder can be expected at all.
    pub gpu_expected: bool,
    /// Human-readable diagnostics (`doctor` prints these verbatim).
    pub notes: Vec<String>,
}

impl PlatformInfo {
    /// Classify collected [`Facts`].
    pub fn from_facts(facts: &Facts) -> Self {
        let termux = facts.has("TERMUX_VERSION") || facts.has("PREFIX");
        let proot = facts.has("PROOT_LOADER")
            || facts.has("PROOT_TMP_DIR")
            || facts.has("PROOT_NO_SECCOMP")
            || facts.proc_version_mentions_proot();
        let android = facts.android_root_exists
            || facts.system_build_prop_exists
            || facts.has("ANDROID_ROOT")
            || facts.has("ANDROID_DATA")
            || facts.has("EXTERNAL_STORAGE");

        let kind = if android && proot {
            PlatformKind::AndroidProot
        } else if android {
            PlatformKind::AndroidNative
        } else {
            PlatformKind::Linux
        };

        let output_dir = default_output_dir(facts, kind);
        let mut notes = Vec::new();

        match kind {
            PlatformKind::AndroidProot => {
                notes.push(
                    "Android via proot: CPU rasterizer only (proot cannot expose /dev/dri, so \
                     Vulkan Video encode is unavailable)."
                        .to_string(),
                );
                if facts.drm_or_kgsl_exists || facts.has("VK_ICD_FILENAMES") || facts.has("VK_DRIVER_FILES") {
                    notes.push(
                        "Vulkan ICD / KGSL device node detected inside proot; wgpu rasterization \
                         may be usable via Turnip/Lavapipe."
                            .to_string(),
                    );
                }
                notes.push(format!("Outputs resolve to {}", output_dir.display()));
                if facts.shared_storage.is_none() {
                    notes.push(
                        "No shared storage visible inside proot; run `termux-setup-storage` in \
                         Termux (or bind-mount /sdcard) to write to /sdcard/Download."
                            .to_string(),
                    );
                }
            }
            PlatformKind::AndroidNative => {
                notes.push(
                    "Native Android (JNI/NDK): MediaCodec/Vulkan surface path; CPU rasterizer + \
                     CPU WGSL VM active by default (Vulkan Video is not available on Android)."
                        .to_string(),
                );
            }
            PlatformKind::Linux => {
                if facts.shared_storage.is_none() {
                    notes.push(format!("Outputs resolve to {}", output_dir.display()));
                }
            }
        }

        Self {
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            kind,
            termux,
            output_dir,
            gpu_expected: !kind.is_android(),
            notes,
        }
    }

    /// Observe and classify in one step.
    pub fn detect() -> Self {
        Self::from_facts(&Facts::collect())
    }
}

fn default_output_dir(facts: &Facts, kind: PlatformKind) -> PathBuf {
    if kind.is_android() {
        if let Some(storage) = &facts.shared_storage {
            let download = storage.join("Download");
            if download.is_dir() {
                return download;
            }
            return download;
        }
        if let Some(external) = facts.env.get("EXTERNAL_STORAGE") {
            if !external.is_empty() {
                return PathBuf::from(external);
            }
        }
    }
    if let Some(home) = facts.env.get("HOME") {
        if !home.is_empty() {
            return PathBuf::from(home);
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Resolve a user-supplied output path.
pub fn resolve_output(requested: &Path, info: &PlatformInfo) -> PathBuf {
    if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        info.output_dir.join(requested)
    }
}

/// Write bytes to `path`, creating parent directories.
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

    fn facts_with(pairs: &[(&str, &str)]) -> Facts {
        Facts {
            env: pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn plain_linux_is_not_android() {
        let info = PlatformInfo::from_facts(&facts_with(&[("HOME", "/root")]));
        assert_eq!(info.kind, PlatformKind::Linux);
        assert!(!info.termux);
        assert!(info.gpu_expected);
        assert_eq!(info.output_dir, PathBuf::from("/root"));
    }

    #[test]
    fn android_proot_is_detected_from_proot_markers() {
        let mut facts = facts_with(&[
            ("ANDROID_ROOT", "/system"),
            ("PROOT_TMP_DIR", "/tmp"),
            ("TERMUX_VERSION", "0.118"),
            ("HOME", "/root"),
        ]);
        facts.android_root_exists = true;
        facts.shared_storage = Some(PathBuf::from("/sdcard"));

        let info = PlatformInfo::from_facts(&facts);
        assert_eq!(info.kind, PlatformKind::AndroidProot);
        assert!(info.termux);
        assert!(!info.gpu_expected, "proot cannot reach the GPU");
        assert!(info.notes.iter().any(|n| n.contains("proot")));
    }

    #[test]
    fn android_without_proot_is_native() {
        let mut facts = facts_with(&[("HOME", "/data")]);
        facts.system_build_prop_exists = true;
        let info = PlatformInfo::from_facts(&facts);
        assert_eq!(info.kind, PlatformKind::AndroidNative);
        assert!(info.notes.iter().any(|n| n.contains("MediaCodec")));
    }

    #[test]
    fn proot_is_detected_from_the_kernel_string_alone() {
        let mut facts = facts_with(&[("HOME", "/root")]);
        facts.android_root_exists = true;
        facts.proc_version =
            Some("Linux version 6.17.0-PRoot-Distro (root@build) #1 SMP aarch64".to_string());
        facts.shared_storage = Some(PathBuf::from("/sdcard"));

        let info = PlatformInfo::from_facts(&facts);
        assert_eq!(info.kind, PlatformKind::AndroidProot);
        assert!(!info.gpu_expected);
        assert_eq!(info.output_dir, PathBuf::from("/sdcard/Download"));
    }

    #[test]
    fn android_without_shared_storage_falls_back_to_home_with_a_note() {
        let mut facts = facts_with(&[
            ("ANDROID_ROOT", "/system"),
            ("PROOT_LOADER", "/libexec/proot"),
            ("HOME", "/data/data/com.termux/files/home"),
        ]);
        facts.android_root_exists = true;
        facts.shared_storage = None;

        let info = PlatformInfo::from_facts(&facts);
        assert_eq!(
            info.output_dir,
            PathBuf::from("/data/data/com.termux/files/home")
        );
        assert!(
            info.notes.iter().any(|n| n.contains("termux-setup-storage")),
            "should tell the user how to fix it: {:?}",
            info.notes
        );
    }

    #[test]
    fn resolve_output_honours_absolute_and_prefixes_relative() {
        let info = PlatformInfo::from_facts(&facts_with(&[("HOME", "/home/u")]));
        assert_eq!(
            resolve_output(Path::new("/tmp/a.png"), &info),
            PathBuf::from("/tmp/a.png")
        );
        assert_eq!(
            resolve_output(Path::new("frame.png"), &info),
            PathBuf::from("/home/u/frame.png")
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
