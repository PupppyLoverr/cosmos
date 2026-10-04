use std::path::{Path, PathBuf};

use chrono::Utc;
use zeron_harness::RunEnvironment;
use zeron_proto::{ByokProvider, CloudSeatBinding, HarnessId};

use crate::runner_credentials::{
    RunnerCredentialKind, RunnerCredentialPlaintext, RunnerCredentialVault,
};

const PROVIDER_ENV: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "OPENAI_API_KEY",
    "OPENAI_BASE_URL",
    "CODEX_API_KEY",
    "OPENROUTER_API_KEY",
    "OPENROUTER_BASE_URL",
    "XAI_API_KEY",
    "XAI_BASE_URL",
    "DEEPSEEK_API_KEY",
    "DEEPSEEK_BASE_URL",
    "XIAOMI_API_KEY",
    "XIAOMI_BASE_URL",
    "DEVIN_API_KEY",
    "DEVIN_BASE_URL",
    "CUSTOM_API_KEY",
    "CUSTOM_API_BASE_URL",
    "GOOGLE_API_KEY",
    "GEMINI_API_KEY",
];

pub(crate) fn harness_environment(
    vault: Option<&RunnerCredentialVault>,
    harness: HarnessId,
    model: Option<&str>,
) -> Result<RunEnvironment, String> {
    let (providers, label) = required_providers(harness, model);
    if providers.is_empty() {
        return Ok(RunEnvironment {
            remove: PROVIDER_ENV
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
            set: Vec::new(),
        });
    }
    let runner_login = vault.is_some_and(|vault| vault.runner_logins().contains(&harness));
    if runner_login {
        return Ok(RunEnvironment::default());
    }

    let mut expired = Vec::new();
    if let Some(vault) = vault {
        for credential in vault.credentials() {
            if credential.metadata.kind != RunnerCredentialKind::ApiKey
                || !credential
                    .metadata
                    .provider
                    .is_some_and(|provider| providers.contains(&provider))
            {
                continue;
            }
            if credential
                .metadata
                .expires_at
                .is_some_and(|expires_at| expires_at <= Utc::now())
            {
                expired.push(credential.metadata.clone());
                continue;
            }
            let Ok(plaintext) = vault.unseal(&credential) else {
                continue;
            };
            return Ok(environment_for_credential(plaintext));
        }
    }
    if let Some(credential) = expired.first() {
        let provider = credential.provider.unwrap_or(ByokProvider::Custom);
        let expires = credential
            .expires_at
            .map(|value| value.to_rfc3339())
            .unwrap_or_else(|| "an unknown date".into());
        return Err(format!(
            "The {} key sealed for this runner expired on {expires}.",
            provider.label()
        ));
    }
    if harness_uses_devin_subscription(harness) {
        Err("No Devin subscription key or runner login is configured for this runner.".into())
    } else {
        Err(format!(
            "No key sealed for this runner ({label}). Seal one in Cloud → this seat → Runner keys."
        ))
    }
}

fn environment_for_credential(credential: RunnerCredentialPlaintext) -> RunEnvironment {
    let Some(provider) = credential.provider else {
        return RunEnvironment {
            remove: PROVIDER_ENV
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
            set: Vec::new(),
        };
    };
    let mut set = vec![(
        crate::cloud::provider_key_env(provider).to_string(),
        credential.key,
    )];
    if let Some(base) = credential.base_url
        && let Some(base) = crate::agent_accounts::stores::trusted_base(
            &base,
            crate::agent_accounts::stores::HostPolicy::AnyDns,
            false,
        )
    {
        set.push((
            crate::cloud::provider_base_env(provider).to_string(),
            base.to_string(),
        ));
    }
    RunEnvironment {
        remove: PROVIDER_ENV
            .iter()
            .map(|name| (*name).to_string())
            .collect(),
        set,
    }
}

fn required_providers(
    harness: HarnessId,
    model: Option<&str>,
) -> (Vec<ByokProvider>, &'static str) {
    match harness {
        HarnessId::Mock => (Vec::new(), "none"),
        HarnessId::ClaudeCode | HarnessId::Amp => (vec![ByokProvider::Anthropic], "Anthropic"),
        HarnessId::Codex => (
            vec![ByokProvider::OpenAi, ByokProvider::ChatgptSub],
            "OpenAI or ChatGPT subscription",
        ),
        HarnessId::Opencode => {
            let provider = model
                .and_then(provider_for_model)
                .unwrap_or(ByokProvider::Custom);
            (vec![provider], provider.label())
        }
        HarnessId::Cursor
        | HarnessId::Devin
        | HarnessId::Grok
        | HarnessId::Hermes
        | HarnessId::Pi
        | HarnessId::Antigravity
        | HarnessId::KimiCode => (vec![ByokProvider::DevinSub], "Devin subscription"),
    }
}

fn harness_uses_devin_subscription(harness: HarnessId) -> bool {
    matches!(
        harness,
        HarnessId::Cursor
            | HarnessId::Devin
            | HarnessId::Grok
            | HarnessId::Hermes
            | HarnessId::Pi
            | HarnessId::Antigravity
            | HarnessId::KimiCode
    )
}

fn provider_for_model(model: &str) -> Option<ByokProvider> {
    let prefix = model.split_once('/')?.0.to_ascii_lowercase();
    Some(match prefix.as_str() {
        "anthropic" => ByokProvider::Anthropic,
        "openai" => ByokProvider::OpenAi,
        "openrouter" => ByokProvider::OpenRouter,
        "xai" | "grok" => ByokProvider::Xai,
        "deepseek" => ByokProvider::DeepSeek,
        "xiaomi" => ByokProvider::Xiaomi,
        "chatgpt-sub" => ByokProvider::ChatgptSub,
        "devin-sub" => ByokProvider::DevinSub,
        _ => ByokProvider::Custom,
    })
}

pub(crate) fn runner_github_token(vault: Option<&RunnerCredentialVault>) -> Option<String> {
    let vault = vault?;
    vault.credentials().into_iter().find_map(|credential| {
        (credential.metadata.kind == RunnerCredentialKind::Github
            && !credential
                .metadata
                .expires_at
                .is_some_and(|expires_at| expires_at <= Utc::now()))
        .then(|| vault.unseal(&credential).ok().map(|secret| secret.key))
        .flatten()
    })
}

pub(crate) async fn prepare_workspace(
    binding: &CloudSeatBinding,
    vault: Option<&RunnerCredentialVault>,
) -> Result<PathBuf, String> {
    let workspace = binding
        .workspace_path
        .as_deref()
        .filter(|path| !path.trim().is_empty())
        .ok_or_else(|| "this runner chat has no workspace path".to_string())?;
    let workspace = crate::repos::expand_home(workspace).map_err(|error| error.to_string())?;
    let path = PathBuf::from(workspace);
    tokio::fs::create_dir_all(&path)
        .await
        .map_err(|error| format!("could not create runner workspace: {error}"))?;

    if is_git_checkout(&path).await {
        return Ok(path);
    }
    let Some(repo) = binding
        .repo
        .as_deref()
        .filter(|repo| !repo.trim().is_empty())
    else {
        return Ok(path);
    };
    let remote = github_https_url(repo)?;
    let output = clone_repository(
        &remote,
        binding.branch.as_deref(),
        &path,
        runner_github_token(vault),
    )
    .await?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr)
            .lines()
            .next()
            .unwrap_or("git clone failed")
            .to_string();
        return Err(format!("could not clone {repo}: {detail}"));
    }
    Ok(path)
}

async fn is_git_checkout(path: &Path) -> bool {
    tokio::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .await
        .is_ok_and(|output| output.status.success() && output.stdout.starts_with(b"true"))
}

pub(crate) async fn inspect_workspace(
    path: &Path,
) -> Result<Option<(bool, Option<String>)>, String> {
    if !is_git_checkout(path).await {
        return Ok(None);
    }
    let status = git_output(path, &["status", "--porcelain"], None).await?;
    let branch = git_output(path, &["rev-parse", "--abbrev-ref", "HEAD"], None).await?;
    let branch = String::from_utf8_lossy(&branch.stdout).trim().to_string();
    Ok(Some((
        !status.stdout.is_empty(),
        (!branch.is_empty() && branch != "HEAD").then_some(branch),
    )))
}

pub(crate) async fn origin_url(path: &Path) -> Result<Option<String>, String> {
    if !is_git_checkout(path).await {
        return Ok(None);
    }
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["remote", "get-url", "origin"])
        .output()
        .await
        .map_err(|error| format!("could not inspect git origin: {error}"))?;
    if !output.status.success() {
        return Ok(None);
    }
    let remote = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!remote.is_empty()).then_some(remote))
}

pub(crate) async fn push_wip_branch(
    path: &Path,
    chat_id: &str,
    commit_message: &str,
    token: Option<String>,
) -> Result<String, String> {
    let chat_prefix = chat_id
        .chars()
        .take(8)
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    let branch = format!(
        "cosmos/wip/{}-{}",
        if chat_prefix.is_empty() {
            "chat"
        } else {
            &chat_prefix
        },
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    );
    let askpass = if token.is_some() {
        Some(create_askpass()?)
    } else {
        None
    };
    git_output(
        path,
        &["switch", "-c", &branch],
        askpass_env(&askpass, &token),
    )
    .await?;
    git_output(path, &["add", "-A"], askpass_env(&askpass, &token)).await?;
    git_output(
        path,
        &["commit", "-m", commit_message],
        askpass_env(&askpass, &token),
    )
    .await?;
    git_output(
        path,
        &["push", "-u", "origin", &branch],
        askpass_env(&askpass, &token),
    )
    .await?;
    Ok(branch)
}

fn askpass_env<'a>(
    askpass: &'a Option<(PathBuf, tempfile::TempDir)>,
    token: &'a Option<String>,
) -> Option<(&'a Path, &'a str)> {
    match (askpass, token.as_deref()) {
        (Some((script, _)), Some(token)) => Some((script, token)),
        _ => None,
    }
}

async fn git_output(
    path: &Path,
    args: &[&str],
    askpass: Option<(&Path, &str)>,
) -> Result<std::process::Output, String> {
    let mut command = tokio::process::Command::new("git");
    command.arg("-C").arg(path);
    if askpass.is_some() {
        command.arg("-c").arg("credential.helper=");
    }
    command.args(args);
    if let Some((script, token)) = askpass {
        command
            .env("GIT_ASKPASS", script)
            .env("COSMOS_GIT_TOKEN", token)
            .env("GIT_TERMINAL_PROMPT", "0");
    }
    command.kill_on_drop(true);
    let output = command
        .output()
        .await
        .map_err(|error| format!("could not run git {}: {error}", args[0]))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr)
            .lines()
            .next()
            .unwrap_or("git command failed")
            .to_string();
        return Err(format!("git {} failed: {detail}", args[0]));
    }
    Ok(output)
}

fn github_https_url(repo: &str) -> Result<String, String> {
    let mut path = repo.trim();
    path = path
        .strip_prefix("https://github.com/")
        .or_else(|| path.strip_prefix("http://github.com/"))
        .or_else(|| path.strip_prefix("git@github.com:"))
        .unwrap_or(path);
    path = path.trim_end_matches('/').trim_end_matches(".git");
    let parts = path.split('/').collect::<Vec<_>>();
    if parts.len() != 2
        || parts.iter().any(|part| {
            part.is_empty()
                || *part == "."
                || *part == ".."
                || !part
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        })
    {
        return Err("runner repository must be a GitHub owner/repository".into());
    }
    Ok(format!("https://github.com/{}/{}.git", parts[0], parts[1]))
}

async fn clone_repository(
    remote: &str,
    branch: Option<&str>,
    path: &Path,
    token: Option<String>,
) -> Result<std::process::Output, String> {
    let askpass = if token.is_some() {
        Some(create_askpass()?)
    } else {
        None
    };
    let mut command = tokio::process::Command::new("git");
    command
        .args(["-c", "credential.helper="])
        .arg("clone")
        .arg("--quiet");
    if let Some(branch) = branch.filter(|branch| !branch.trim().is_empty()) {
        command.arg("--branch").arg(branch);
    }
    command
        .arg(remote)
        .arg(path)
        .env("GIT_TERMINAL_PROMPT", "0");
    if let (Some((script, _)), Some(token)) = (&askpass, token) {
        command
            .env("GIT_ASKPASS", script)
            .env("COSMOS_GIT_TOKEN", token);
    }
    command.kill_on_drop(true);
    tokio::time::timeout(std::time::Duration::from_secs(300), command.output())
        .await
        .map_err(|_| "git clone timed out".to_string())?
        .map_err(|error| format!("could not run git clone: {error}"))
}

fn create_askpass() -> Result<(PathBuf, tempfile::TempDir), String> {
    let dir = tempfile::Builder::new()
        .prefix("cosmos-runner-askpass-")
        .tempdir()
        .map_err(|error| format!("could not create git credential helper: {error}"))?;
    let script = dir.path().join("askpass");
    std::fs::write(
        &script,
        "#!/bin/sh\ncase \"$1\" in\n  *sername*) printf '%s\\n' 'x-access-token' ;;\n  *) printf '%s\\n' \"$COSMOS_GIT_TOKEN\" ;;\nesac\n",
    )
    .map_err(|error| format!("could not write git credential helper: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("could not secure git credential helper: {error}"))?;
    }
    Ok((script, dir))
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};

    use super::{harness_environment, inspect_workspace, push_wip_branch};
    use crate::runner_credentials::{
        RunnerCredentialKind, RunnerCredentialPlaintext, RunnerCredentialVault, seal_for_runner,
    };
    use zeron_proto::{ByokProvider, HarnessId};

    fn vault() -> (tempfile::TempDir, RunnerCredentialVault) {
        let root = tempfile::tempdir().unwrap();
        let vault = RunnerCredentialVault::load(root.path()).unwrap();
        (root, vault)
    }

    fn save_key(
        vault: &RunnerCredentialVault,
        provider: ByokProvider,
        key: &str,
        base_url: Option<&str>,
        expires_at: Option<chrono::DateTime<Utc>>,
    ) {
        let plaintext = RunnerCredentialPlaintext {
            kind: RunnerCredentialKind::ApiKey,
            provider: Some(provider),
            key: key.into(),
            base_url: base_url.map(str::to_string),
            expires_at,
        };
        let sealed = seal_for_runner(&vault.public_key(), &plaintext).unwrap();
        vault.store(sealed).unwrap();
    }

    #[test]
    fn required_credentials_fail_before_a_run_can_start() {
        let (_root, vault) = vault();
        let error = harness_environment(Some(&vault), HarnessId::ClaudeCode, None).unwrap_err();
        assert!(error.contains("No key sealed for this runner (Anthropic)"));
    }

    #[test]
    fn expired_sealed_key_has_expiry_specific_message() {
        let (_root, vault) = vault();
        save_key(
            &vault,
            ByokProvider::Anthropic,
            "test-key",
            None,
            Some(Utc::now() - Duration::days(1)),
        );
        let error = harness_environment(Some(&vault), HarnessId::ClaudeCode, None).unwrap_err();
        assert!(error.contains("expired on"));
        assert!(!error.contains("No key sealed"));
    }

    #[test]
    fn runner_environment_isolates_and_injects_only_the_selected_key() {
        let (_root, vault) = vault();
        save_key(
            &vault,
            ByokProvider::OpenRouter,
            "sealed-key",
            Some("https://api.example.com/v1"),
            None,
        );
        let environment =
            harness_environment(Some(&vault), HarnessId::Opencode, Some("openrouter/model"))
                .unwrap();
        assert!(
            environment
                .remove
                .iter()
                .any(|name| name == "CLAUDE_CODE_OAUTH_TOKEN")
        );
        assert!(
            environment
                .remove
                .iter()
                .any(|name| name == "OPENAI_API_KEY")
        );
        assert_eq!(
            environment.set,
            vec![
                ("OPENROUTER_API_KEY".into(), "sealed-key".into()),
                (
                    "OPENROUTER_BASE_URL".into(),
                    "https://api.example.com/v1".into(),
                ),
            ]
        );
    }

    #[test]
    fn untrusted_base_url_is_not_injected_and_mock_needs_no_key() {
        let (_root, vault) = vault();
        save_key(
            &vault,
            ByokProvider::Custom,
            "sealed-key",
            Some("http://127.0.0.1:1234"),
            None,
        );
        let environment =
            harness_environment(Some(&vault), HarnessId::Opencode, Some("unknown/model")).unwrap();
        assert!(
            environment
                .set
                .contains(&("CUSTOM_API_KEY".into(), "sealed-key".into()))
        );
        assert!(
            !environment
                .set
                .iter()
                .any(|(name, _)| name == "CUSTOM_API_BASE_URL")
        );

        let mock = harness_environment(None, HarnessId::Mock, None).unwrap();
        assert!(mock.remove.iter().any(|name| name == "OPENAI_API_KEY"));
        assert!(mock.set.is_empty());
    }

    #[test]
    fn explicit_runner_login_keeps_the_runner_process_environment() {
        let (_root, vault) = vault();
        vault.set_login(HarnessId::ClaudeCode, true).unwrap();
        assert!(
            harness_environment(Some(&vault), HarnessId::ClaudeCode, None)
                .unwrap()
                .remove
                .is_empty()
        );
    }

    #[tokio::test]
    async fn dirty_runner_workspace_pushes_a_wip_branch_to_its_origin() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        let remote = root.path().join("remote.git");
        std::fs::create_dir_all(&repo).unwrap();
        let output = std::process::Command::new("git")
            .args(["init", "--bare"])
            .arg(&remote)
            .output()
            .unwrap();
        assert!(output.status.success());
        for args in [
            vec!["init"],
            vec!["config", "user.name", "Runner Test"],
            vec!["config", "user.email", "runner@example.com"],
        ] {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success());
        }
        std::fs::write(repo.join("work.txt"), "initial").unwrap();
        for args in [vec!["add", "-A"], vec!["commit", "-m", "initial"]] {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success());
        }
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["remote", "add", "origin"])
            .arg(&remote)
            .output()
            .unwrap();
        assert!(output.status.success());
        std::fs::write(repo.join("work.txt"), "changed").unwrap();
        assert_eq!(
            inspect_workspace(&repo).await.unwrap(),
            Some((
                true,
                Some(
                    String::from_utf8_lossy(
                        &std::process::Command::new("git")
                            .arg("-C")
                            .arg(&repo)
                            .args(["rev-parse", "--abbrev-ref", "HEAD"])
                            .output()
                            .unwrap()
                            .stdout,
                    )
                    .trim()
                    .to_string(),
                )
            ))
        );

        let branch = push_wip_branch(&repo, "chat-123456789", "WIP from Cosmos runner", None)
            .await
            .unwrap();
        assert!(branch.starts_with("cosmos/wip/chat-123-"));
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["log", "-1", "--format=%s"])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "WIP from Cosmos runner"
        );
        let output = std::process::Command::new("git")
            .arg("--git-dir")
            .arg(&remote)
            .args(["branch", "--list", &branch])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), branch);
    }
}
