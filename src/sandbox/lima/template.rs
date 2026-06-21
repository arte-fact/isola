use std::path::Path;

/// Generate a Lima YAML configuration for a sandbox VM.
pub fn generate_lima_yaml(workspace: Option<&Path>, session_dir: &Path) -> String {
    let mut yaml = String::from(
        r#"vmType: "vz"
vmOpts:
  vz:
    rosetta:
      enabled: true
      binfmt: true
images:
  - location: "https://cloud-images.ubuntu.com/releases/24.04/release/ubuntu-24.04-server-cloudimg-arm64.img"
    arch: "aarch64"
  - location: "https://cloud-images.ubuntu.com/releases/24.04/release/ubuntu-24.04-server-cloudimg-amd64.img"
    arch: "x86_64"
cpus: 4
memory: "4GiB"
disk: "5GiB"
mountType: "virtiofs"
containerd:
  system: false
  user: false
mounts:
  - location: "~"
    writable: false
"#,
    );

    if let Some(ws) = workspace {
        yaml.push_str(&format!(
            "  - location: \"{}\"\n    mountPoint: \"/workspace\"\n    writable: true\n",
            ws.display()
        ));
    }

    yaml.push_str(&format!(
        "  - location: \"{}\"\n    mountPoint: \"/tmp/isola-session\"\n    writable: true\n",
        session_dir.display()
    ));

    yaml
}

/// Cloud image URL used by Lima (informational, for config storage).
pub const CLOUD_IMAGE_URL: &str =
    "https://cloud-images.ubuntu.com/releases/24.04/release/ubuntu-24.04-server-cloudimg-arm64.img";
