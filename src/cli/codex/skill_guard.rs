use std::path::Path;

use anyhow::Context;

use super::reconcile_imagegen_skill;
use crate::cli::atomic_file::write_atomic_if_changed;

const GUARD_SKILL: &str = r#"---
name: codex-mixin-skill-guardian
description: Restore Codex Mixin managed Skill rewrites after Codex Desktop updates replace system Skills.
---

# Codex Mixin Skill Guardian

Codex Mixin owns the managed ImageGen rewrite. The gateway reconciles it on every startup and after Provider configuration changes.

If a Codex Desktop update replaces the official ImageGen Skill, restart Codex Mixin. The guardian restores the managed rewrite from the Codex Mixin binary without relying on this Skill file as the source of truth.

Do not edit files under `.codex/skills/.system` manually. Change the managed template in Codex Mixin and rebuild the app instead.
"#;

pub(in crate::cli) fn reconcile_managed_skills(
    codex_home: &Path,
    auxiliary_provider_enabled: bool,
) -> anyhow::Result<bool> {
    let guard_path = codex_home
        .join("skills")
        .join("codex-mixin-skill-guardian")
        .join("SKILL.md");
    let guard_changed =
        write_atomic_if_changed(&guard_path, GUARD_SKILL.as_bytes()).with_context(|| {
            format!(
                "failed to install skill guardian at {}",
                guard_path.display()
            )
        })?;
    let imagegen_changed = reconcile_imagegen_skill(codex_home, auxiliary_provider_enabled)?;
    Ok(guard_changed || imagegen_changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installs_the_guard_outside_the_system_skill_directory() {
        let directory = tempfile::tempdir().unwrap();
        let imagegen_directory = directory.path().join("skills/.system/imagegen/scripts");
        std::fs::create_dir_all(&imagegen_directory).unwrap();
        std::fs::write(
            directory.path().join("skills/.system/imagegen/SKILL.md"),
            "official imagegen skill",
        )
        .unwrap();
        std::fs::write(
            imagegen_directory.join("image_gen.py"),
            "official imagegen script",
        )
        .unwrap();
        let changed = reconcile_managed_skills(directory.path(), true).unwrap();

        assert!(changed);
        assert!(
            directory
                .path()
                .join("skills/codex-mixin-skill-guardian/SKILL.md")
                .is_file()
        );
        assert!(
            directory
                .path()
                .join("skills/.system/imagegen/SKILL.md")
                .is_file()
        );

        let imagegen_skill = directory.path().join("skills/.system/imagegen/SKILL.md");
        std::fs::write(&imagegen_skill, "replacement from desktop update").unwrap();
        assert!(reconcile_managed_skills(directory.path(), true).unwrap());
        assert!(
            std::fs::read_to_string(imagegen_skill)
                .unwrap()
                .contains("codex-mixin managed imagegen skill v3")
        );
    }
}
