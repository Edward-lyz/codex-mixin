use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, ensure};

const MANAGED_PREFIX: &str = "codex-mixin managed imagegen skill";
const MANAGED_MARKER: &str = "codex-mixin managed imagegen skill v2";
const BACKUP_SUFFIX: &str = ".codex-mixin.bak";

const MANAGED_SKILL: &str = r#"---
name: "imagegen"
description: __DESCRIPTION__
---

# Codex Mixin Image Generation

Read this file at `__SKILL_PATH__` before acting. Do not search for another imagegen Skill or inspect unrelated imagegen directories.

Use the built-in `image_gen` tool when it is available. If it is unavailable for a generation request, run `python3 '__SCRIPT_PATH__'`; do not stop to ask for `OPENAI_API_KEY` or install Python packages.

The CLI uses only the Python standard library and sends image requests through the local Codex Mixin gateway. Image generation uses the enabled provider marked for voice, automatic review, and other auxiliary tasks when that provider configures an image generation path. With no auxiliary provider selected, the gateway uses the official Codex image backend.

Run `python3 scripts/image_gen.py generate --prompt <prompt> --out <path>`. Optional arguments include `--model`, `--n`, `--size`, `--quality`, `--background`, `--output-format`, `--force`, and `--dry-run`. Put final assets in the user's requested location, or in the current project when no location is specified. Report the generated file paths.

Image editing requires the built-in `image_gen` tool. If it is unavailable, report that the managed bridge does not support editing instead of approximating the edit with a new generation.

<!-- codex-mixin managed imagegen skill v2 -->
"#;

const MANAGED_WRAPPER: &str = r#"#!/usr/bin/env python3
"""Dependency-free image generation CLI for Codex Mixin."""

from __future__ import annotations

import argparse
import base64
import json
import os
from pathlib import Path
import sys
import urllib.error
import urllib.request


def _die(message: str) -> None:
    raise SystemExit(f"Error: {message}")


def _parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    generate = subparsers.add_parser("generate", help="Generate raster images")
    prompt = generate.add_mutually_exclusive_group(required=True)
    prompt.add_argument("--prompt")
    prompt.add_argument("--prompt-file", type=Path)
    generate.add_argument("--model", default="gpt-image-2")
    generate.add_argument("--n", type=int, default=1)
    generate.add_argument("--size", default="auto")
    generate.add_argument("--quality", default="medium")
    generate.add_argument("--background")
    generate.add_argument("--output-format", default="png", choices=("png", "jpeg", "webp"))
    generate.add_argument("--out", type=Path, default=Path("output/imagegen/output.png"))
    generate.add_argument("--force", action="store_true")
    generate.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()
    if args.n < 1:
        _die("--n must be at least 1")
    return args


def _gateway_config() -> tuple[str, str]:
    config_path = Path(os.getenv("CODEX_MIXIN_CONFIG", "~/.codex-mixin/config.json")).expanduser()
    try:
        config = json.loads(config_path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        _die(f"Codex Mixin config is missing: {config_path}")
    except (OSError, json.JSONDecodeError) as exc:
        _die(f"Cannot read Codex Mixin config {config_path}: {exc}")

    bind = config.get("gateway_bind") or "127.0.0.1:8787"
    if not isinstance(bind, str) or not bind:
        _die("Codex Mixin gateway_bind must be a non-empty string")
    if bind.startswith("["):
        gateway_authority = bind
    elif bind.count(":") > 1:
        gateway_authority = f"[{bind}]"
    else:
        gateway_authority = bind
    return f"http://{gateway_authority}/v1/images/generations", config.get("gateway_api_key") or "codex-mixin-local"


def _output_paths(output: Path, count: int, output_format: str) -> list[Path]:
    suffix = f".{output_format}"
    output = output.with_suffix(suffix)
    if count == 1:
        return [output]
    return [output.with_name(f"{output.stem}-{index}{suffix}") for index in range(1, count + 1)]


def main() -> None:
    args = _parse_args()
    prompt = args.prompt
    if args.prompt_file is not None:
        try:
            prompt = args.prompt_file.read_text(encoding="utf-8")
        except OSError as exc:
            _die(f"Cannot read prompt file {args.prompt_file}: {exc}")
    if not prompt or not prompt.strip():
        _die("prompt must not be empty")

    endpoint, api_key = _gateway_config()
    payload = {
        "model": args.model,
        "prompt": prompt.strip(),
        "n": args.n,
        "size": args.size,
        "quality": args.quality,
        "output_format": args.output_format,
    }
    if args.background is not None:
        payload["background"] = args.background
    outputs = _output_paths(args.out, args.n, args.output_format)
    if args.dry_run:
        print(json.dumps({"endpoint": endpoint, "payload": payload, "outputs": [str(path) for path in outputs]}, indent=2))
        return
    for output in outputs:
        if output.exists() and not args.force:
            _die(f"Output already exists: {output} (use --force to overwrite)")

    request = urllib.request.Request(
        endpoint,
        data=json.dumps(payload).encode("utf-8"),
        headers={"Authorization": f"Bearer {api_key}", "Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=300) as response:
            response_body = response.read()
    except urllib.error.HTTPError as exc:
        detail = exc.read().decode("utf-8", errors="replace")
        _die(f"Image API returned HTTP {exc.code}: {detail}")
    except urllib.error.URLError as exc:
        _die(f"Cannot reach Codex Mixin image endpoint {endpoint}: {exc.reason}")

    try:
        response_json = json.loads(response_body)
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        _die(f"Image API returned invalid JSON: {exc}")
    if not isinstance(response_json, dict):
        _die("Image API returned a non-object JSON response")
    image_items = response_json.get("data")
    if not isinstance(image_items, list) or len(image_items) != len(outputs):
        _die(f"Image API returned {0 if not isinstance(image_items, list) else len(image_items)} images, expected {len(outputs)}")

    for image_item, output in zip(image_items, outputs):
        if not isinstance(image_item, dict):
            _die("Image API returned an invalid image item")
        if isinstance(image_item.get("b64_json"), str):
            try:
                image_bytes = base64.b64decode(image_item["b64_json"], validate=True)
            except ValueError as exc:
                _die(f"Image API returned invalid base64 data: {exc}")
        elif isinstance(image_item.get("url"), str):
            try:
                with urllib.request.urlopen(image_item["url"], timeout=300) as response:
                    image_bytes = response.read()
            except urllib.error.URLError as exc:
                _die(f"Cannot download generated image: {exc.reason}")
        else:
            _die("Image API response contains neither b64_json nor url")
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_bytes(image_bytes)
        print(f"Wrote {output}")


if __name__ == "__main__":
    main()

# codex-mixin managed imagegen skill v2
"#;

pub(in crate::cli) fn reconcile_imagegen_skill(
    codex_home: &Path,
    auxiliary_provider_enabled: bool,
) -> anyhow::Result<bool> {
    if auxiliary_provider_enabled {
        install_imagegen_skill(codex_home)
    } else {
        restore_imagegen_skill(codex_home)
    }
}

fn install_imagegen_skill(codex_home: &Path) -> anyhow::Result<bool> {
    let skill_dir = codex_home.join("skills/.system/imagegen");
    let skill_path = skill_dir.join("SKILL.md");
    let script_path = skill_dir.join("scripts/image_gen.py");
    let description = serde_json::to_string(&format!(
        "Read {}, then generate raster images through Codex Mixin.",
        skill_path.display()
    ))?;
    let managed_skill = MANAGED_SKILL
        .replace("__DESCRIPTION__", &description)
        .replace("__SKILL_PATH__", &skill_path.to_string_lossy())
        .replace("__SCRIPT_PATH__", &script_path.to_string_lossy());
    if !skill_path.exists() && !script_path.exists() {
        return Ok(false);
    }
    ensure!(
        skill_path.is_file() && script_path.is_file(),
        "incomplete Codex imagegen skill at {}",
        skill_dir.display()
    );

    let script_changed = install_managed_file(&script_path, MANAGED_WRAPPER)?;
    let skill_changed = match install_managed_file(&skill_path, &managed_skill) {
        Ok(changed) => changed,
        Err(error) => {
            if script_changed {
                restore_managed_file(&script_path)?;
            }
            return Err(error);
        }
    };
    Ok(skill_changed || script_changed)
}

pub(in crate::cli) fn restore_imagegen_skill(codex_home: &Path) -> anyhow::Result<bool> {
    let skill_dir = codex_home.join("skills/.system/imagegen");
    let skill_restored = restore_managed_file(&skill_dir.join("SKILL.md"))?;
    let script_restored = restore_managed_file(&skill_dir.join("scripts/image_gen.py"))?;
    Ok(skill_restored || script_restored)
}

fn install_managed_file(path: &Path, managed_content: &str) -> anyhow::Result<bool> {
    let current = fs::read_to_string(path)
        .with_context(|| format!("read Codex imagegen file {}", path.display()))?;
    if current.contains(MANAGED_MARKER) {
        return Ok(false);
    }
    if current.contains(MANAGED_PREFIX) {
        ensure!(
            backup_path(path).is_file(),
            "managed Codex imagegen file {} is missing its official backup",
            path.display()
        );
        fs::write(path, managed_content.as_bytes())
            .with_context(|| format!("upgrade managed Codex imagegen file {}", path.display()))?;
        return Ok(true);
    }
    let backup = backup_path(path);
    fs::write(&backup, current.as_bytes())
        .with_context(|| format!("back up Codex imagegen file to {}", backup.display()))?;
    fs::write(path, managed_content.as_bytes())
        .with_context(|| format!("install managed Codex imagegen file {}", path.display()))?;
    Ok(true)
}

fn restore_managed_file(path: &Path) -> anyhow::Result<bool> {
    let backup = backup_path(path);
    if !backup.exists() {
        return Ok(false);
    }
    let current = fs::read_to_string(path)
        .with_context(|| format!("read Codex imagegen file {}", path.display()))?;
    if current.contains(MANAGED_PREFIX) {
        fs::rename(&backup, path).with_context(|| {
            format!(
                "restore Codex imagegen file {} from {}",
                path.display(),
                backup.display()
            )
        })?;
        return Ok(true);
    }
    fs::remove_file(&backup)
        .with_context(|| format!("remove stale imagegen backup {}", backup.display()))?;
    Ok(false)
}

fn backup_path(path: &Path) -> PathBuf {
    let mut backup = path.as_os_str().to_owned();
    backup.push(BACKUP_SUFFIX);
    PathBuf::from(backup)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installs_and_restores_managed_imagegen_skill() {
        let root =
            std::env::temp_dir().join(format!("codex-mixin-imagegen-test-{}", std::process::id()));
        let script_dir = root.join("skills/.system/imagegen/scripts");
        fs::create_dir_all(&script_dir).unwrap();
        let skill_path = root.join("skills/.system/imagegen/SKILL.md");
        let script_path = script_dir.join("image_gen.py");
        fs::write(&skill_path, "upstream skill\n").unwrap();
        fs::write(&script_path, "upstream script\n").unwrap();

        assert!(reconcile_imagegen_skill(&root, true).unwrap());
        assert!(!reconcile_imagegen_skill(&root, true).unwrap());
        let installed_skill = fs::read_to_string(&skill_path).unwrap();
        assert!(installed_skill.contains(&skill_path.display().to_string()));
        assert!(installed_skill.contains(&script_path.display().to_string()));
        assert!(!installed_skill.contains("__SKILL_PATH__"));
        assert!(!installed_skill.contains("__SCRIPT_PATH__"));
        assert_eq!(fs::read_to_string(&script_path).unwrap(), MANAGED_WRAPPER);
        assert!(!MANAGED_WRAPPER.contains("import openai"));
        assert_eq!(
            fs::read_to_string(backup_path(&script_path)).unwrap(),
            "upstream script\n"
        );

        fs::write(
            &skill_path,
            installed_skill.replace(MANAGED_MARKER, "codex-mixin managed imagegen skill v1"),
        )
        .unwrap();
        fs::write(
            &script_path,
            MANAGED_WRAPPER.replace(MANAGED_MARKER, "codex-mixin managed imagegen skill v1"),
        )
        .unwrap();
        assert!(reconcile_imagegen_skill(&root, true).unwrap());
        assert_eq!(
            fs::read_to_string(backup_path(&skill_path)).unwrap(),
            "upstream skill\n"
        );
        assert_eq!(
            fs::read_to_string(backup_path(&script_path)).unwrap(),
            "upstream script\n"
        );

        assert!(reconcile_imagegen_skill(&root, false).unwrap());
        assert_eq!(fs::read_to_string(&skill_path).unwrap(), "upstream skill\n");
        assert_eq!(
            fs::read_to_string(&script_path).unwrap(),
            "upstream script\n"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
