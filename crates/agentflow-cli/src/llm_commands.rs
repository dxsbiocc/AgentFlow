use std::fs;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use agentflow_core::storage::ProjectStore;

use crate::cli_args::{LlmArgs, LlmCommand, LlmConfigArgs, PathJsonArgs};
use crate::{last_value, CliError};

const LLM_ENV_FILE: &str = "llm.env";
const LLM_SYNTH_SH: &str = "llm-synth.sh";
const LLM_SYNTH_PY: &str = "llm-synth.py";

#[derive(Debug, Clone, PartialEq, Eq)]
struct LlmProvider {
    name: &'static str,
    key_var: &'static str,
    base_url_var: &'static str,
    model_var: &'static str,
    default_model: Option<&'static str>,
}

#[derive(Debug)]
struct LlmConfigOptions {
    project: PathJsonArgs,
    provider: String,
    api_key: String,
    model: String,
    base_url: Option<String>,
    synthesizer: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LlmEnvEntry {
    pub(crate) key: String,
    pub(crate) value: String,
}

pub(crate) fn llm_command(args: LlmArgs) -> Result<String, CliError> {
    match args.command {
        LlmCommand::Config(args) => llm_config_command(args),
    }
}

pub(crate) fn llm_env_path(project_root: &Path) -> PathBuf {
    project_root.join(".agentflow").join(LLM_ENV_FILE)
}

pub(crate) fn configured_synthesizer(project_root: &Path) -> Result<Option<String>, CliError> {
    Ok(load_project_llm_env(project_root)?
        .into_iter()
        .find(|entry| entry.key == "AGENTFLOW_SYNTHESIZER")
        .map(|entry| entry.value))
}

pub(crate) fn load_project_llm_env(project_root: &Path) -> Result<Vec<LlmEnvEntry>, CliError> {
    let env_path = llm_env_path(project_root);
    if !env_path.exists() {
        return Ok(Vec::new());
    }
    let contents = fs::read_to_string(&env_path).map_err(|error| {
        CliError::Core(format!(
            "failed to read LLM env file {}: {error}",
            env_path.display()
        ))
    })?;
    parse_env_file(&contents, &env_path)
}

fn llm_config_command(args: LlmConfigArgs) -> Result<String, CliError> {
    let options = LlmConfigOptions::try_from(args)?;
    let provider = provider_spec(&options.provider)?;
    let project_path = options
        .project
        .path
        .last()
        .cloned()
        .unwrap_or(std::env::current_dir()?);
    let store = ProjectStore::open(&project_path)?;
    let root = store.root_path();
    let absolute_root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let env_path = llm_env_path(root);
    let synth_sh = absolute_root.join(".agentflow").join(LLM_SYNTH_SH);
    let synth_py = absolute_root.join(".agentflow").join(LLM_SYNTH_PY);
    let synthesizer = options
        .synthesizer
        .clone()
        .unwrap_or_else(|| shell_command_arg(&synth_sh.display().to_string()));

    if let Some(parent) = env_path.parent() {
        fs::create_dir_all(parent)?;
    }
    write_llm_synth_scripts(&synth_sh, &synth_py)?;
    write_llm_env_file(&env_path, &provider, &options, &synthesizer)?;

    if options.project.json {
        Ok(llm_config_json(
            provider.name,
            &env_path,
            provider.key_var,
            &options.model,
            options.base_url.is_some(),
            &synthesizer,
        ))
    } else {
        Ok(format!(
            concat!(
                "LLM config written\n",
                "Provider: {}\n",
                "Env file: {}\n",
                "API key: {}={}\n",
                "Model: {}\n",
                "Synthesizer: {}\n"
            ),
            provider.name,
            env_path.display(),
            provider.key_var,
            mask_secret(&options.api_key),
            options.model,
            synthesizer
        ))
    }
}

impl TryFrom<LlmConfigArgs> for LlmConfigOptions {
    type Error = CliError;

    fn try_from(args: LlmConfigArgs) -> Result<Self, Self::Error> {
        let provider = last_value(args.provider).ok_or_else(|| {
            CliError::InvalidArgument("llm config requires --provider <name>".to_string())
        })?;
        let provider_spec = provider_spec(&provider)?;
        let model = last_value(args.model)
            .or_else(|| provider_spec.default_model.map(ToOwned::to_owned))
            .ok_or_else(|| {
                CliError::InvalidArgument("llm config requires --model <model>".to_string())
            })?;
        let api_key = match (last_value(args.api_key), last_value(args.api_key_env)) {
            (Some(value), None) => value,
            (None, Some(env_name)) => std::env::var(&env_name).map_err(|_| {
                CliError::InvalidArgument(format!("environment variable {env_name} is not set"))
            })?,
            (Some(_), Some(_)) => {
                return Err(CliError::InvalidArgument(
                    "use either --api-key or --api-key-env, not both".to_string(),
                ));
            }
            (None, None) => {
                return Err(CliError::InvalidArgument(
                    "llm config requires --api-key <key> or --api-key-env <env-var>".to_string(),
                ));
            }
        };
        if api_key.trim().is_empty() {
            return Err(CliError::InvalidArgument(
                "LLM API key must not be empty".to_string(),
            ));
        }
        Ok(Self {
            project: args.project,
            provider,
            api_key,
            model,
            base_url: last_value(args.base_url),
            synthesizer: last_value(args.synthesizer),
        })
    }
}

fn provider_spec(provider: &str) -> Result<LlmProvider, CliError> {
    match provider.trim().to_ascii_lowercase().as_str() {
        "anthropic" | "claude" => Ok(LlmProvider {
            name: "anthropic",
            key_var: "ANTHROPIC_API_KEY",
            base_url_var: "ANTHROPIC_BASE_URL",
            model_var: "ANTHROPIC_MODEL",
            default_model: None,
        }),
        "openai" | "codex" => Ok(LlmProvider {
            name: "openai",
            key_var: "OPENAI_API_KEY",
            base_url_var: "OPENAI_BASE_URL",
            model_var: "OPENAI_MODEL",
            default_model: None,
        }),
        "google" | "gemini" => Ok(LlmProvider {
            name: "gemini",
            key_var: "GEMINI_API_KEY",
            base_url_var: "GEMINI_BASE_URL",
            model_var: "GEMINI_MODEL",
            default_model: None,
        }),
        "deepseek" => Ok(LlmProvider {
            name: "deepseek",
            key_var: "DEEPSEEK_API_KEY",
            base_url_var: "DEEPSEEK_BASE_URL",
            model_var: "DEEPSEEK_MODEL",
            default_model: Some("deepseek-v4-flash"),
        }),
        other => Err(CliError::InvalidArgument(format!(
            "unsupported LLM provider {other}; supported providers are anthropic, openai, gemini, and deepseek"
        ))),
    }
}

fn write_llm_env_file(
    env_path: &Path,
    provider: &LlmProvider,
    options: &LlmConfigOptions,
    synthesizer: &str,
) -> Result<(), CliError> {
    let mut entries = vec![
        ("AGENTFLOW_LLM_PROVIDER", provider.name.to_string()),
        (provider.key_var, options.api_key.clone()),
        (provider.model_var, options.model.clone()),
        ("AGENTFLOW_SYNTHESIZER", synthesizer.to_string()),
    ];
    if let Some(base_url) = options.base_url.as_ref() {
        entries.push((provider.base_url_var, base_url.clone()));
    }
    let mut contents = String::from(
        "# Managed by agentflow llm config.\n# Keep this file local; it may contain API secrets.\n",
    );
    for (key, value) in entries {
        validate_env_key(key)?;
        contents.push_str(key);
        contents.push('=');
        contents.push_str(&shell_single_quote(&value));
        contents.push('\n');
    }
    fs::write(env_path, contents)?;
    set_secret_permissions(env_path)?;
    Ok(())
}

fn write_llm_synth_scripts(synth_sh: &Path, synth_py: &Path) -> Result<(), CliError> {
    fs::write(synth_sh, llm_synth_shell())?;
    fs::write(synth_py, llm_synth_python())?;
    set_executable_permissions(synth_sh)?;
    set_secret_permissions(synth_py)?;
    Ok(())
}

fn parse_env_file(contents: &str, env_path: &Path) -> Result<Vec<LlmEnvEntry>, CliError> {
    let mut entries = Vec::new();
    for (index, line) in contents.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let assignment = trimmed.strip_prefix("export ").unwrap_or(trimmed);
        let Some((key, value)) = assignment.split_once('=') else {
            return Err(CliError::InvalidArgument(format!(
                "invalid LLM env line {} in {}",
                index + 1,
                env_path.display()
            )));
        };
        validate_env_key(key)?;
        entries.push(LlmEnvEntry {
            key: key.to_string(),
            value: parse_env_value(value).map_err(|message| {
                CliError::InvalidArgument(format!(
                    "invalid LLM env line {} in {}: {message}",
                    index + 1,
                    env_path.display()
                ))
            })?,
        });
    }
    Ok(entries)
}

fn validate_env_key(key: &str) -> Result<(), CliError> {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return Err(CliError::InvalidArgument(
            "environment key must not be empty".to_string(),
        ));
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return Err(CliError::InvalidArgument(format!(
            "invalid environment key {key}"
        )));
    }
    if !chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric()) {
        return Err(CliError::InvalidArgument(format!(
            "invalid environment key {key}"
        )));
    }
    Ok(())
}

fn parse_env_value(value: &str) -> Result<String, &'static str> {
    let trimmed = value.trim();
    if trimmed.starts_with('\'') {
        parse_single_quoted(trimmed)
    } else if trimmed.starts_with('"') {
        parse_double_quoted(trimmed)
    } else {
        Ok(trimmed.to_string())
    }
}

fn parse_single_quoted(value: &str) -> Result<String, &'static str> {
    if !value.ends_with('\'') {
        return Err("unterminated single-quoted value");
    }
    Ok(value[1..value.len() - 1].replace("'\\''", "'"))
}

fn parse_double_quoted(value: &str) -> Result<String, &'static str> {
    if !value.ends_with('"') {
        return Err("unterminated double-quoted value");
    }
    let inner = &value[1..value.len() - 1];
    let mut output = String::new();
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(next) = chars.next() {
                output.push(next);
            }
        } else {
            output.push(ch);
        }
    }
    Ok(output)
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn shell_command_arg(value: &str) -> String {
    if value.chars().all(|ch| {
        ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | ':' | '+' | '=')
    }) {
        value.to_string()
    } else {
        shell_single_quote(value)
    }
}

fn mask_secret(secret: &str) -> String {
    let trimmed = secret.trim();
    if trimmed.len() <= 8 {
        "****".to_string()
    } else {
        format!("{}…{}", &trimmed[..4], &trimmed[trimmed.len() - 4..])
    }
}

fn llm_config_json(
    provider: &str,
    env_path: &Path,
    key_var: &str,
    model: &str,
    base_url_configured: bool,
    synthesizer: &str,
) -> String {
    format!(
        concat!(
            "{{\"schema_version\":\"agentflow.llm_config.v0\",",
            "\"provider\":\"{}\",",
            "\"env_file\":\"{}\",",
            "\"api_key_var\":\"{}\",",
            "\"api_key_configured\":true,",
            "\"model\":\"{}\",",
            "\"base_url_configured\":{},",
            "\"synthesizer\":\"{}\"}}"
        ),
        json_escape(provider),
        json_escape(&env_path.display().to_string()),
        json_escape(key_var),
        json_escape(model),
        base_url_configured,
        json_escape(synthesizer)
    )
}

fn json_escape(value: &str) -> String {
    value
        .chars()
        .flat_map(|ch| match ch {
            '"' => "\\\"".chars().collect::<Vec<_>>(),
            '\\' => "\\\\".chars().collect::<Vec<_>>(),
            '\n' => "\\n".chars().collect::<Vec<_>>(),
            '\r' => "\\r".chars().collect::<Vec<_>>(),
            '\t' => "\\t".chars().collect::<Vec<_>>(),
            ch => vec![ch],
        })
        .collect()
}

#[cfg(unix)]
fn set_secret_permissions(path: &Path) -> Result<(), CliError> {
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_secret_permissions(_path: &Path) -> Result<(), CliError> {
    Ok(())
}

#[cfg(unix)]
fn set_executable_permissions(path: &Path) -> Result<(), CliError> {
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable_permissions(_path: &Path) -> Result<(), CliError> {
    Ok(())
}

fn llm_synth_shell() -> &'static str {
    r#"#!/bin/sh
set -eu
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
if [ -f "$script_dir/llm.env" ]; then
  set -a
  # shellcheck disable=SC1091
  . "$script_dir/llm.env"
  set +a
fi
exec python3 "$script_dir/llm-synth.py" "$@"
"#
}

fn llm_synth_python() -> &'static str {
    r#"#!/usr/bin/env python3
import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request


def env(name, default=None):
    value = os.environ.get(name)
    return value if value not in (None, "") else default


def post_json(url, headers, payload):
    data = json.dumps(payload).encode("utf-8")
    request = urllib.request.Request(url, data=data, headers=headers, method="POST")
    try:
        with urllib.request.urlopen(request, timeout=120) as response:
            return json.loads(response.read().decode("utf-8"))
    except urllib.error.HTTPError as error:
        body = error.read().decode("utf-8", errors="replace")
        raise SystemExit(f"LLM HTTP {error.code}: {body}") from error
    except urllib.error.URLError as error:
        raise SystemExit(f"LLM connection error: {error.reason}") from error


def call_anthropic(prompt):
    api_key = env("ANTHROPIC_API_KEY")
    model = env("ANTHROPIC_MODEL")
    if not api_key or not model:
        raise SystemExit("ANTHROPIC_API_KEY and ANTHROPIC_MODEL are required")
    base_url = env("ANTHROPIC_BASE_URL", "https://api.anthropic.com")
    url = base_url.rstrip("/") + "/v1/messages"
    payload = {
        "model": model,
        "max_tokens": int(env("AGENTFLOW_LLM_MAX_TOKENS", "4096")),
        "messages": [{"role": "user", "content": prompt}],
    }
    response = post_json(url, {
        "content-type": "application/json",
        "x-api-key": api_key,
        "anthropic-version": env("ANTHROPIC_VERSION", "2023-06-01"),
    }, payload)
    return "".join(part.get("text", "") for part in response.get("content", []) if part.get("type") == "text")


def call_openai_compatible(prompt, api_key_var, model_var, base_url_var, default_base_url):
    api_key = env(api_key_var)
    model = env(model_var)
    if not api_key or not model:
        raise SystemExit(f"{api_key_var} and {model_var} are required")
    base_url = env(base_url_var, default_base_url)
    url = base_url.rstrip("/") + "/chat/completions"
    payload = {
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "temperature": 0,
    }
    response = post_json(url, {
        "content-type": "application/json",
        "authorization": f"Bearer {api_key}",
    }, payload)
    return response["choices"][0]["message"]["content"]


def call_openai(prompt):
    return call_openai_compatible(
        prompt,
        "OPENAI_API_KEY",
        "OPENAI_MODEL",
        "OPENAI_BASE_URL",
        "https://api.openai.com/v1",
    )


def call_deepseek(prompt):
    return call_openai_compatible(
        prompt,
        "DEEPSEEK_API_KEY",
        "DEEPSEEK_MODEL",
        "DEEPSEEK_BASE_URL",
        "https://api.deepseek.com/v1",
    )


def call_gemini(prompt):
    api_key = env("GEMINI_API_KEY")
    model = env("GEMINI_MODEL")
    if not api_key or not model:
        raise SystemExit("GEMINI_API_KEY and GEMINI_MODEL are required")
    base_url = env("GEMINI_BASE_URL", "https://generativelanguage.googleapis.com/v1beta")
    quoted_model = urllib.parse.quote(model, safe="")
    url = f"{base_url.rstrip('/')}/models/{quoted_model}:generateContent?key={urllib.parse.quote(api_key)}"
    payload = {"contents": [{"parts": [{"text": prompt}]}]}
    response = post_json(url, {"content-type": "application/json"}, payload)
    parts = response["candidates"][0]["content"].get("parts", [])
    return "".join(part.get("text", "") for part in parts)


def main():
    if len(sys.argv) < 2:
        raise SystemExit("usage: llm-synth.py <prompt>")
    prompt = sys.argv[1]
    provider = env("AGENTFLOW_LLM_PROVIDER", "anthropic").lower()
    if provider in ("anthropic", "claude"):
        text = call_anthropic(prompt)
    elif provider in ("openai", "codex"):
        text = call_openai(prompt)
    elif provider == "deepseek":
        text = call_deepseek(prompt)
    elif provider in ("gemini", "google"):
        text = call_gemini(prompt)
    else:
        raise SystemExit(f"unsupported AGENTFLOW_LLM_PROVIDER: {provider}")
    print(text, end="" if text.endswith("\n") else "\n")


if __name__ == "__main__":
    main()
"#
}
