use codex_mixin::application::{ech, error::OperationError};

use super::args::EchCommand;

pub(super) async fn run(command: EchCommand) -> anyhow::Result<()> {
    match command {
        EchCommand::Status { json } => {
            let status = ech::status()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else {
                println!(
                    "official GPT ECH: {}\nDoH: {}",
                    if status.enabled {
                        "enabled"
                    } else {
                        "disabled"
                    },
                    status.doh_url
                );
                if let Some(reason) = status.fallback_reason {
                    println!("ECH off, ordinary network settings restored: {reason}");
                }
            }
        }
        EchCommand::Test { json } => {
            test(json).await?;
        }
        EchCommand::Enable => {
            super::progress_step("Testing official GPT ECH without credentials");
            let probes = ech::probe().await;
            print_probes(&probes, false)?;
            let enabled = ech::save_probe_result(&probes)?;
            super::progress_step("Applying official GPT ECH setting");
            super::service::restart_managed().await.map_err(|error| {
                OperationError::AfterCommit {
                    stage: "gateway restart",
                    source: error,
                }
            })?;
            if enabled {
                println!("official GPT ECH enabled; relay exit is not verified");
            } else {
                return Err(OperationError::AfterCommit {
                    stage: "ECH enable",
                    source: anyhow::anyhow!("ECH unavailable: automatically disabled; ordinary network settings restored. {}",
                        probes.iter().filter_map(|probe| probe.error.as_deref()).collect::<Vec<_>>().join("; ")),
                }.into());
            }
        }
        EchCommand::Disable => {
            ech::set_enabled(false)?;
            super::service::restart_managed().await.map_err(|error| {
                OperationError::AfterCommit {
                    stage: "gateway restart",
                    source: error,
                }
            })?;
            println!("official GPT ECH disabled");
        }
    }
    Ok(())
}

async fn test(json: bool) -> anyhow::Result<()> {
    let probes = ech::probe().await;
    print_probes(&probes, json)?;
    anyhow::ensure!(
        probes.iter().all(ech::EchProbe::ready),
        "official GPT ECH test failed; no network setting was changed"
    );
    Ok(())
}

fn print_probes(probes: &[ech::EchProbe], json: bool) -> anyhow::Result<()> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({"probes": probes}))?
        );
    } else {
        for probe in probes {
            println!(
                "{}: ECH {}, addresses {:?}, connected {:?}, HTTP {:?}, {} ms",
                probe.host,
                if probe.ech_accepted {
                    "accepted"
                } else {
                    "failed"
                },
                probe.resolved_addresses,
                probe.connected_address,
                probe.http_status,
                probe.elapsed_ms
            );
            if let Some(error) = &probe.error {
                println!("error: {error}");
            }
        }
        println!("ECH acceptance does not verify the relay exit IP or region.");
    }
    Ok(())
}
