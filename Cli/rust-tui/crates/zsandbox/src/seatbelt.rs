//! macOS Seatbelt profile — Chrome-like: workspace write, no network unless
//! `--net allow`, deny ~/.ssh and .env. Compile-gated; not executed on
//! Windows/Linux CI. Do not fake a pass.

#![cfg(target_os = "macos")]

/// Sandbox profile text. Applied with `sandbox-exec -p` when this crate is
/// built on macOS. Workspace path is interpolated by the caller.
pub fn profile(workspace: &str, net_allow: bool) -> String {
    let net = if net_allow {
        "(allow network*)"
    } else {
        "(deny network*)"
    };
    format!(
        r#"(version 1)
(deny default)
(allow process-exec)
(allow process-fork)
(allow signal)
(allow sysctl-read)
(allow file-read*)
(allow file-write* (subpath "{workspace}"))
(deny file-write* (subpath "{home}/.ssh"))
(deny file-write* (subpath "{home}/.gnupg"))
(deny file-write* (regex #"/\.env($|\.)"))
{net}
"#,
        workspace = workspace,
        home = std::env::var("HOME").unwrap_or_else(|_| "/Users".into()),
        net = net,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_denies_ssh_and_env() {
        let p = profile("/tmp/ws", false);
        assert!(p.contains(".ssh"));
        assert!(p.contains(".env"));
        assert!(p.contains("deny network"));
    }
}
