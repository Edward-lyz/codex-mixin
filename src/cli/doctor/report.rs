use console::style;

use super::{DoctorReport, planned_fixes};

pub(super) fn print_doctor_report(report: &DoctorReport, fix_mode: bool) {
    println!("{}", style("Codex Mixin health check").bold());
    println!("{} {}", style("config:").dim(), report.config_path);
    for check in &report.checks {
        println!(
            "{} {}: {}",
            check.status.icon(),
            style(&check.name).bold(),
            check.message
        );
        if let Some(detail) = &check.detail {
            println!("  {}", style(detail).dim());
        }
        if let Some(hint) = &check.fix_hint {
            println!("  {} {hint}", style("hint:").cyan());
        }
        if !check.auto_fixes.is_empty() {
            println!(
                "  {} {}",
                style("auto-fix:").cyan(),
                check
                    .auto_fixes
                    .iter()
                    .map(|fix| fix.description())
                    .collect::<Vec<_>>()
                    .join("；")
            );
        }
    }
    for provider in &report.providers {
        println!(
            "{} {} {}: {}",
            provider.status.icon(),
            style("Provider").dim(),
            style(&provider.provider_id).bold(),
            provider.message
        );
        if let Some(detail) = &provider.detail {
            println!("  {}", style(detail).dim());
        }
    }
    if !report.repairs.is_empty() {
        println!("{}", style("repairs:").bold());
        for repair in &report.repairs {
            let icon = if repair.ok {
                style("✓").green().bold()
            } else {
                style("✗").red().bold()
            };
            println!("{} {}: {}", icon, repair.description, repair.message);
        }
    }
    let summary_style = if report.summary.errors > 0 {
        style("summary:").red().bold()
    } else if report.summary.warnings > 0 {
        style("summary:").yellow().bold()
    } else {
        style("summary:").green().bold()
    };
    println!(
        "{} {} ok, {} warnings, {} errors",
        summary_style, report.summary.ok, report.summary.warnings, report.summary.errors
    );
    let available = planned_fixes(&report.checks);
    if !fix_mode && !available.is_empty() {
        let restart_count = available
            .iter()
            .filter(|fix| fix.requires_restart_opt_in())
            .count();
        let plain_count = available.len() - restart_count;
        if plain_count > 0 {
            println!(
                "{} run `codex-mixin doctor --fix` to repair {plain_count} item(s)",
                style("→").cyan()
            );
        }
        if restart_count > 0 {
            println!(
                "{} app restarts need explicit confirmation; run `codex-mixin doctor --fix --restart-apps` (this interrupts live sessions)",
                style("→").cyan()
            );
        }
    }
    if report.ok {
        println!("{} doctor: ok", style("✓").green().bold());
    } else {
        println!("{} doctor: issues found", style("✗").red().bold());
    }
}
