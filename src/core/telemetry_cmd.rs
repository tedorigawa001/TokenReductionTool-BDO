use anyhow::{Context, Result};
use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum TelemetrySubcommand {
    Status,
    Enable,
    Disable,
    Forget,
}

pub fn run(command: &TelemetrySubcommand) -> Result<()> {
    match command {
        TelemetrySubcommand::Status => run_status(),
        TelemetrySubcommand::Enable => run_enable(),
        TelemetrySubcommand::Disable => run_disable(),
        TelemetrySubcommand::Forget => run_forget(),
    }
}

fn run_status() -> Result<()> {
    let config = crate::core::config::Config::load().unwrap_or_default();

    let consent_str = match config.telemetry.consent_given {
        Some(true) => "yes",
        Some(false) => "no",
        None => "never asked",
    };

    let enabled_str = if config.telemetry.enabled {
        "yes"
    } else {
        "no"
    };

    let env_override = std::env::var("BDO_TELEMETRY_DISABLED").unwrap_or_default() == "1";

    println!("Telemetry status:");
    println!("  consent:       {}", consent_str);
    if let Some(date) = &config.telemetry.consent_date {
        println!("  consent date:  {}", date);
    }
    println!("  enabled:       {}", enabled_str);
    if env_override {
        println!("  env override:  BDO_TELEMETRY_DISABLED=1 (blocked)");
    }

    let salt_path = super::telemetry::salt_file_path();
    if salt_path.exists() {
        let hash = super::telemetry::generate_device_hash();
        println!("  device hash:   {}...{}", &hash[..8], &hash[56..]);
    } else {
        println!("  device hash:   (no salt file)");
    }

    println!();
    println!("Data controller: Bushido (github.com/tedorigawa001/TokenReductionTool)");
    println!("Details: https://github.com/tedorigawa001/TokenReductionTool/blob/master/docs/TELEMETRY.md");

    Ok(())
}

fn run_enable() -> Result<()> {
    use std::io::{self, BufRead, IsTerminal};

    if !io::stdin().is_terminal() {
        anyhow::bail!(
            "consent requires interactive terminal — cannot enable telemetry in piped mode"
        );
    }

    eprintln!("Bushido collects anonymous usage metrics once per day to improve filters.");
    eprintln!();
    eprintln!("  What:    command names (not arguments), token savings, OS, version");
    eprintln!("  Who:     Bushido (github.com/tedorigawa001/TokenReductionTool)");
    eprintln!("  Details: https://github.com/tedorigawa001/TokenReductionTool/blob/master/docs/TELEMETRY.md");
    eprintln!();
    eprint!("Enable anonymous telemetry? [y/N] ");

    let stdin = io::stdin();
    let mut line = String::new();
    stdin
        .lock()
        .read_line(&mut line)
        .context("Failed to read user input")?;

    let accepted = {
        let response = line.trim().to_lowercase();
        response == "y" || response == "yes"
    };

    crate::hooks::init::save_telemetry_consent(accepted)?;

    if accepted {
        println!("Telemetry enabled. Disable anytime: bdo telemetry disable");
    } else {
        println!("Telemetry not enabled.");
    }

    Ok(())
}

fn run_disable() -> Result<()> {
    crate::hooks::init::save_telemetry_consent(false)?;
    println!("Telemetry disabled.");
    Ok(())
}

fn run_forget() -> Result<()> {
    crate::hooks::init::save_telemetry_consent(false)?;

    let salt_path = super::telemetry::salt_file_path();
    let marker_path = super::telemetry::telemetry_marker_path();

    // Compute device hash before deleting the salt
    let device_hash = if salt_path.exists() {
        Some(super::telemetry::generate_device_hash())
    } else {
        None
    };

    if salt_path.exists() {
        std::fs::remove_file(&salt_path)
            .with_context(|| format!("Failed to delete {}", salt_path.display()))?;
    }

    if marker_path.exists() {
        let _ = std::fs::remove_file(&marker_path);
    }

    // Purge local tracking database (GDPR Art. 17 — right to erasure applies to local data too)
    let db_path = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(super::constants::BDO_DATA_DIR)
        .join(super::constants::HISTORY_DB);
    if db_path.exists() {
        match std::fs::remove_file(&db_path) {
            Ok(()) => println!("Local tracking database deleted: {}", db_path.display()),
            Err(e) => eprintln!("bdo: could not delete {}: {}", db_path.display(), e),
        }
    }

    // Send server-side erasure request
    if let Some(hash) = device_hash {
        match send_erasure_request(&hash) {
            Ok(()) => {
                println!("Erasure request sent to server.");
            }
            Err(e) => {
                eprintln!("bdo: could not reach server: {}", e);
                eprintln!("  To complete erasure, open an issue at github.com/tedorigawa001/TokenReductionTool/issues");
                eprintln!("  with your device hash: {}", hash);
            }
        }
    }

    println!("Local telemetry data deleted. Telemetry disabled.");
    Ok(())
}

fn send_erasure_request(device_hash: &str) -> Result<()> {
    let url = option_env!("BDO_TELEMETRY_URL");
    let url = match url {
        Some(u) => format!("{}/erasure", u),
        None => anyhow::bail!("no telemetry endpoint configured"),
    };

    let payload = serde_json::json!({
        "device_hash": device_hash,
        "action": "erasure",
    });

    let mut req = ureq::post(&url).set("Content-Type", "application/json");

    if let Some(token) = option_env!("BDO_TELEMETRY_TOKEN") {
        req = req.set("X-Bushido-Token", token);
    }

    req.timeout(std::time::Duration::from_secs(5))
        .send_string(&payload.to_string())?;

    Ok(())
}
