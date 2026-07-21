use std::path::{Component, Path, PathBuf};

use axum::http::HeaderMap;
use serde_json::Value;
use uuid::Uuid;
use walkdir::WalkDir;

use super::render::escape_html;
use super::types::{
    FusionDetail, FusionJudgeDetail, FusionJudgeStatus, FusionPanelDetail, FusionPanelStatus,
    JudgeSynthesis,
};

const MAX_VISUALIZATION_BYTES: usize = 2_000_000;
const VISUALIZATION_TITLE: &str = "Fusion · Review";

struct JudgePoint {
    title: String,
    body: String,
}

pub(super) async fn create_fusion_visualization(
    body: &Value,
    headers: &HeaderMap,
    codex_auth_path: &Path,
    panels: &[FusionPanelDetail],
    judge: &FusionJudgeDetail,
) -> Result<Option<FusionDetail>, String> {
    let Some(codex_home) = codex_auth_path.parent() else {
        return Ok(None);
    };
    let root = codex_home.join("visualizations");
    let Some(directory) = find_thread_visualization_dir(body, headers, &root) else {
        return Ok(None);
    };
    write_visualization(&root, &directory, panels, judge)
        .await
        .map(Some)
}

fn find_thread_visualization_dir(
    body: &Value,
    headers: &HeaderMap,
    root: &Path,
) -> Option<PathBuf> {
    let mut roots = Vec::new();
    collect_context_roots(body, &mut roots);
    if let Some(directory) = roots
        .into_iter()
        .find(|directory| is_thread_visualization_dir(directory, root))
    {
        return Some(directory);
    }

    let thread_id = request_thread_id(headers)?;
    WalkDir::new(root)
        .min_depth(4)
        .max_depth(4)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .find(|entry| {
            entry.file_type().is_dir() && entry.file_name() == std::ffi::OsStr::new(&thread_id)
        })
        .map(|entry| entry.into_path())
}

fn request_thread_id(headers: &HeaderMap) -> Option<String> {
    ["thread-id", "x-codex-parent-thread-id", "session-id"]
        .into_iter()
        .filter_map(|name| headers.get(name)?.to_str().ok())
        .find(|value| Uuid::parse_str(value).is_ok())
        .map(str::to_owned)
}

fn collect_context_roots(body: &Value, roots: &mut Vec<PathBuf>) {
    if let Some(instructions) = body.get("instructions").and_then(Value::as_str) {
        extract_root_tags(instructions, roots);
    }
    let Some(items) = body.get("input").and_then(Value::as_array) else {
        return;
    };
    for item in items {
        if item.get("role").and_then(Value::as_str) != Some("developer") {
            continue;
        }
        match item.get("content") {
            Some(Value::String(content)) => extract_root_tags(content, roots),
            Some(Value::Array(parts)) => {
                for text in parts
                    .iter()
                    .filter_map(|part| part.get("text").and_then(Value::as_str))
                {
                    extract_root_tags(text, roots);
                }
            }
            _ => {}
        }
    }
}

fn extract_root_tags(text: &str, roots: &mut Vec<PathBuf>) {
    let mut remaining = text;
    while let Some(start) = remaining.find("<root>") {
        let value = &remaining[start + "<root>".len()..];
        let Some(end) = value.find("</root>") else {
            break;
        };
        let path = value[..end].trim().replace("&amp;", "&");
        if !path.is_empty() {
            roots.push(PathBuf::from(path));
        }
        remaining = &value[end + "</root>".len()..];
    }
}

fn is_thread_visualization_dir(directory: &Path, root: &Path) -> bool {
    if !directory.is_absolute()
        || directory
            .components()
            .any(|part| part == Component::ParentDir)
    {
        return false;
    }
    let Ok(relative) = directory.strip_prefix(root) else {
        return false;
    };
    let parts = relative.components().collect::<Vec<_>>();
    parts.len() == 4
        && parts
            .iter()
            .all(|part| matches!(part, Component::Normal(_)))
        && directory
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|value| Uuid::parse_str(value).is_ok())
}

async fn write_visualization(
    root: &Path,
    directory: &Path,
    panels: &[FusionPanelDetail],
    judge: &FusionJudgeDetail,
) -> Result<FusionDetail, String> {
    tokio::fs::create_dir_all(root)
        .await
        .map_err(|error| format!("create visualization root: {error}"))?;
    let root = tokio::fs::canonicalize(root)
        .await
        .map_err(|error| format!("canonicalize visualization root: {error}"))?;
    tokio::fs::create_dir_all(directory)
        .await
        .map_err(|error| format!("create thread visualization directory: {error}"))?;
    let directory = tokio::fs::canonicalize(directory)
        .await
        .map_err(|error| format!("canonicalize thread visualization directory: {error}"))?;
    if !directory.starts_with(&root) {
        return Err("thread visualization directory escapes CODEX_HOME".to_owned());
    }

    let id = Uuid::new_v4().simple().to_string();
    let short_id = &id[..12];
    let file_name = format!("fusion-results-{short_id}.html");
    let fragment = visualization_fragment(short_id, panels, judge);
    if fragment.len() > MAX_VISUALIZATION_BYTES {
        return Err(format!(
            "fusion visualization exceeds {MAX_VISUALIZATION_BYTES} bytes"
        ));
    }
    let path = directory.join(&file_name);
    tokio::fs::write(&path, fragment)
        .await
        .map_err(|error| format!("write {}: {error}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .await
            .map_err(|error| format!("set permissions on {}: {error}", path.display()))?;
    }

    Ok(FusionDetail {
        title: VISUALIZATION_TITLE.to_owned(),
        text: format!("::codex-inline-vis{{file=\"{file_name}\"}}"),
    })
}

fn visualization_fragment(
    id: &str,
    panels: &[FusionPanelDetail],
    judge: &FusionJudgeDetail,
) -> String {
    let cards = panels
        .iter()
        .map(|panel| {
            let (status, status_class, summary) = match panel.status {
                FusionPanelStatus::Completed => ("Completed", "text-muted", "Open response"),
                FusionPanelStatus::Failed => ("Failed", "text-destructive", "Open error"),
            };
            let model = model_label(&panel.model);
            let preview = panel_preview(&panel.text);
            format!(
                "<details class=\"card fusion-panel\"><summary aria-label=\"{summary} from {}\"><span class=\"fusion-card-heading\">{model}<span class=\"{status_class} text-small\">{status}</span></span><span class=\"fusion-preview text-muted\">{preview}</span></summary><div class=\"fusion-response\">{}</div></details>",
                escape_html(&panel.model),
                escape_html(&panel.text)
            )
        })
        .collect::<String>();
    let points = judge_points(judge);
    let point_buttons = points
        .iter()
        .enumerate()
        .map(|(index, point)| {
            format!(
                "<button type=\"button\" class=\"btn btn-ghost\" data-fusion-point=\"{index}\" data-tooltip=\"{}\" aria-controls=\"fusion-point-{id}-{index}\" aria-pressed=\"{}\">{}</button>",
                escape_html(&point.title),
                index == 0,
                index + 1
            )
        })
        .collect::<String>();
    let point_panels = points
        .iter()
        .enumerate()
        .map(|(index, point)| {
            let hidden = if index == 0 { "" } else { " hidden" };
            format!(
                "<section id=\"fusion-point-{id}-{index}\" class=\"fusion-point\"{hidden}><h3>{}</h3><div class=\"fusion-point-body\">{}</div></section>",
                escape_html(&point.title),
                escape_html(&point.body)
            )
        })
        .collect::<String>();
    let judge_status_class = match judge.status {
        FusionJudgeStatus::Completed => "text-muted",
        FusionJudgeStatus::Failed | FusionJudgeStatus::TimedOut | FusionJudgeStatus::Skipped => {
            "text-destructive"
        }
    };
    format!(
        "<div id=\"fusion-results-{id}\" aria-label=\"Fusion review\">\n<style>\n#fusion-results-{id} .fusion-section-heading {{ margin-bottom: 0.75rem; }}\n#fusion-results-{id} .fusion-panels {{ margin-bottom: 1.5rem; }}\n#fusion-results-{id} .fusion-panel summary {{ cursor: pointer; }}\n#fusion-results-{id} .fusion-card-heading {{ display: flex; align-items: baseline; justify-content: space-between; gap: 0.75rem; }}\n#fusion-results-{id} .fusion-model {{ display: flex; flex-direction: column; gap: 0.125rem; min-width: 0; }}\n#fusion-results-{id} .fusion-model > span {{ overflow-wrap: anywhere; }}\n#fusion-results-{id} .fusion-preview {{ display: block; margin-top: 0.75rem; overflow-wrap: anywhere; }}\n#fusion-results-{id} .fusion-panel[open] .fusion-preview {{ display: none; }}\n#fusion-results-{id} .fusion-response {{ margin-top: 1rem; padding-top: 1rem; border-top: 1px solid var(--border); white-space: pre-wrap; overflow-wrap: anywhere; color: var(--foreground); }}\n#fusion-results-{id} .fusion-judge-heading {{ justify-content: space-between; align-items: baseline; margin-bottom: 0.75rem; }}\n#fusion-results-{id} .fusion-point-controls {{ margin-bottom: 0.75rem; }}\n#fusion-results-{id} .fusion-point-body {{ white-space: pre-wrap; overflow-wrap: anywhere; }}\n@media (max-width: 480px) {{ #fusion-results-{id} .fusion-judge-heading {{ align-items: flex-start; }} }}\n</style>\n<section aria-labelledby=\"fusion-panels-{id}\"><h3 id=\"fusion-panels-{id}\" class=\"fusion-section-heading\">Panel perspectives</h3><div class=\"viz-grid fusion-panels\">{cards}</div></section>\n<section aria-labelledby=\"fusion-judge-{id}\"><div class=\"viz-row fusion-judge-heading\"><h3 id=\"fusion-judge-{id}\">Judge synthesis</h3><span class=\"{judge_status_class} text-small\"><code>{}</code> · {}</span></div><div class=\"viz-controls fusion-point-controls\" aria-label=\"Judge synthesis points\">{point_buttons}</div><div class=\"card\">{point_panels}</div></section>\n<script>\n(() => {{\n  const root = document.getElementById(\"fusion-results-{id}\");\n  if (!root) return;\n  const buttons = Array.from(root.querySelectorAll(\"[data-fusion-point]\"));\n  const points = Array.from(root.querySelectorAll(\".fusion-point\"));\n  root.addEventListener(\"click\", (event) => {{\n    const target = event.target;\n    if (!(target instanceof Element)) return;\n    const button = target.closest(\"[data-fusion-point]\");\n    if (!button || !root.contains(button)) return;\n    const selected = Number(button.getAttribute(\"data-fusion-point\"));\n    buttons.forEach((candidate, index) => candidate.setAttribute(\"aria-pressed\", String(index === selected)));\n    points.forEach((point, index) => {{ point.hidden = index !== selected; }});\n  }});\n}})();\n</script>\n</div>\n",
        escape_html(&judge.model),
        judge.status.label(),
    )
}

fn model_label(model: &str) -> String {
    let Some((provider, name)) = model.split_once(':') else {
        return format!(
            "<span class=\"fusion-model\"><span>{}</span></span>",
            escape_html(model)
        );
    };
    format!(
        "<span class=\"fusion-model\"><span>{}</span><span class=\"text-muted text-small\">{}</span></span>",
        escape_html(name),
        escape_html(provider)
    )
}

fn panel_preview(text: &str) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut preview = compact.chars().take(96).collect::<String>();
    if compact.chars().count() > 96 {
        preview.push('…');
    }
    escape_html(&preview)
}

fn judge_points(judge: &FusionJudgeDetail) -> Vec<JudgePoint> {
    if let Ok(synthesis) = serde_json::from_str::<JudgeSynthesis>(&judge.text) {
        return synthesis
            .points
            .into_iter()
            .map(|point| JudgePoint {
                title: point.title,
                body: point.body,
            })
            .collect();
    }
    markdown_judge_points(&judge.text)
}

fn markdown_judge_points(text: &str) -> Vec<JudgePoint> {
    let mut points = Vec::new();
    let mut title = "Overview".to_owned();
    let mut body = Vec::new();
    for line in text.lines() {
        if let Some(heading) = markdown_heading(line) {
            push_judge_point(&mut points, &title, &body);
            title = heading;
            body.clear();
        } else {
            body.push(line);
        }
    }
    push_judge_point(&mut points, &title, &body);
    if points.is_empty() {
        points.push(JudgePoint {
            title: "Judge result".to_owned(),
            body: text.trim().to_owned(),
        });
    }
    points
}

fn markdown_heading(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if let Some(heading) = trimmed
        .strip_prefix("**")
        .and_then(|value| value.strip_suffix("**"))
        .filter(|value| !value.trim().is_empty())
    {
        return Some(heading.trim().to_owned());
    }
    let heading = trimmed.trim_start_matches('#');
    (heading.len() < trimmed.len() && !heading.trim().is_empty()).then(|| heading.trim().to_owned())
}

fn push_judge_point(points: &mut Vec<JudgePoint>, title: &str, lines: &[&str]) {
    let body = lines.join("\n").trim().to_owned();
    if !body.is_empty() {
        points.push(JudgePoint {
            title: title.to_owned(),
            body,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::super::types::JudgeSynthesisPoint;
    use super::*;
    use tempfile::tempdir;

    fn panels() -> Vec<FusionPanelDetail> {
        vec![FusionPanelDetail {
            index: 0,
            model: "panel<a>".to_owned(),
            status: FusionPanelStatus::Completed,
            text: "finding <script>alert(1)</script>".to_owned(),
        }]
    }

    fn judge() -> FusionJudgeDetail {
        FusionJudgeDetail {
            model: "judge<a>".to_owned(),
            status: FusionJudgeStatus::Completed,
            text: serde_json::to_string(&JudgeSynthesis {
                points: vec![
                    JudgeSynthesisPoint {
                        title: "共识".to_owned(),
                        body: "shared <script>alert(2)</script>".to_owned(),
                    },
                    JudgeSynthesisPoint {
                        title: "分歧与缺口".to_owned(),
                        body: "missing coverage".to_owned(),
                    },
                    JudgeSynthesisPoint {
                        title: "建议".to_owned(),
                        body: "ship carefully".to_owned(),
                    },
                ],
            })
            .unwrap(),
        }
    }

    #[test]
    fn finds_thread_directory_from_developer_context() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("visualizations");
        let thread_id = Uuid::new_v4().to_string();
        let thread_dir = root.join("2026/07/21").join(&thread_id);
        let body = serde_json::json!({
            "input":[{
                "type":"message",
                "role":"developer",
                "content":[{"type":"input_text","text":format!(
                    "<workspace_roots><root>{}</root></workspace_roots>",
                    thread_dir.display()
                )}]
            }]
        });
        let mut headers = HeaderMap::new();
        headers.insert("thread-id", thread_id.parse().unwrap());

        assert_eq!(
            find_thread_visualization_dir(&body, &headers, &root),
            Some(thread_dir)
        );
    }

    #[tokio::test]
    async fn writes_safe_inline_visualization_fragment() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("visualizations");
        let thread_dir = root.join("2026/07/21").join(Uuid::new_v4().to_string());
        let detail = write_visualization(&root, &thread_dir, &panels(), &judge())
            .await
            .unwrap();
        let file_name = detail
            .text
            .strip_prefix("::codex-inline-vis{file=\"")
            .and_then(|value| value.strip_suffix("\"}"))
            .unwrap();
        let fragment = tokio::fs::read_to_string(thread_dir.join(file_name))
            .await
            .unwrap();

        assert_eq!(detail.title, VISUALIZATION_TITLE);
        assert!(fragment.starts_with("<div id=\"fusion-results-"));
        assert!(fragment.contains("class=\"viz-grid fusion-panels\""));
        assert!(fragment.contains("class=\"card fusion-panel\""));
        assert!(fragment.contains(">1</button>"));
        assert!(fragment.contains(">2</button>"));
        assert!(fragment.contains(">3</button>"));
        assert!(fragment.contains("data-fusion-point"));
        assert!(fragment.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(fragment.contains("shared &lt;script&gt;alert(2)&lt;/script&gt;"));
        assert!(!fragment.contains("<!doctype"));
        assert!(!fragment.contains("fetch("));
    }

    #[test]
    fn turns_markdown_judge_sections_into_points() {
        let points = markdown_judge_points(
            "**Consensus**\nShared result.\n\n## Risks\nMissing coverage.\n\n**Recommendation**\nShip carefully.",
        );
        assert_eq!(points.len(), 3);
        assert_eq!(points[0].title, "Consensus");
        assert_eq!(points[1].title, "Risks");
        assert_eq!(points[2].title, "Recommendation");
    }
}
