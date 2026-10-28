//! Stress-harness helper. Emits counts and transport errors, never pane text.
use std::{env, process::ExitCode, time::Duration};

use workbench_core::Engine;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{}", error.to_string().escape_debug());
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if args.len() != 3 {
        return Err("usage: preview_probe STATE_FILE WORK_ID SECONDS".into());
    }
    let engine = Engine::new(&args[0]);
    let id = args[1].to_str().ok_or("invalid work ID")?;
    let seconds: u64 = args[2].to_str().ok_or("invalid duration")?.parse()?;
    if seconds == 0 {
        return Err("duration must be positive".into());
    }
    let item = engine
        .work_items()?
        .into_iter()
        .find(|item| item.id == id)
        .ok_or("work item not found")?;
    let mut captures = 0_u64;
    let result = tokio::time::timeout(
        Duration::from_secs(seconds),
        engine.stream_preview(&item, |_| {
            captures += 1;
        }),
    )
    .await;
    let error = match result {
        Ok(Err(error)) => Some(error.to_string().escape_debug().to_string()),
        Ok(Ok(())) => Some("stream ended unexpectedly".into()),
        Err(_) => None,
    };
    println!(
        "{}",
        serde_json::json!({"captures": captures, "error": error})
    );
    Ok(())
}
