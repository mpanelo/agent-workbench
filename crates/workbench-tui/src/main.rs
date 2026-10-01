use std::{io, time::Duration};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use tokio::{
    sync::watch,
    time::{self, MissedTickBehavior},
};
use workbench_core::{Engine, Snapshot, WorkItemState};

mod cli;
mod ui;

type DiscoveryState = Option<Result<Snapshot, String>>;

#[derive(Clone, Debug, Default)]
struct AppState {
    discovery: DiscoveryState,
    work_items: Option<Result<Vec<WorkItemState>, String>>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    match start().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn start() -> io::Result<()> {
    let options = cli::parse(std::env::args_os().skip(1)).map_err(io::Error::other)?;
    if options.command == cli::Command::Help {
        print!("{}", cli::HELP);
        return Ok(());
    }
    let engine = Engine::new(options.state_path().map_err(io::Error::other)?);
    match options.command {
        cli::Command::Register(item) => {
            let id = item.id.clone();
            engine.register_work_item(item).map_err(io::Error::other)?;
            println!("Registered {id}.");
            return Ok(());
        }
        cli::Command::List => {
            let items = engine.work_items().map_err(io::Error::other)?;
            if items.is_empty() {
                println!("No work items registered.");
            }
            for item in items {
                println!(
                    "{}\tUNKNOWN\t{}\t{}\n  {}\n  Repository: {}\n  Workspace: {}\n  Branch: {}",
                    item.id,
                    item.kind,
                    item.pane_id,
                    item.title,
                    item.repository.display(),
                    item.workspace.display(),
                    item.branch.as_deref().unwrap_or("unavailable")
                );
            }
            return Ok(());
        }
        cli::Command::Run => {}
        cli::Command::Help => unreachable!("help was handled above"),
    }
    let mut terminal = match ratatui::try_init() {
        Ok(terminal) => terminal,
        Err(error) => {
            ratatui::restore();
            return Err(error);
        }
    };
    let result = run(&mut terminal, engine).await;
    let cleanup = ratatui::try_restore();
    result.and(cleanup)
}

async fn run(terminal: &mut ratatui::DefaultTerminal, engine: Engine) -> io::Result<()> {
    let (sender, mut receiver) = watch::channel::<AppState>(AppState::default());
    let discovery = tokio::spawn(async move {
        let mut refresh = time::interval(Duration::from_secs(2));
        refresh.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            refresh.tick().await;
            let discovery = engine.discover().await.map_err(|error| error.to_string());
            let work_items = engine
                .work_item_states(discovery.as_ref().ok())
                .map_err(|error| error.to_string());
            if sender
                .send(AppState {
                    discovery: Some(discovery),
                    work_items: Some(work_items),
                })
                .is_err()
            {
                break;
            }
        }
    });
    let result = event_loop(terminal, &mut receiver).await;
    discovery.abort();
    // Wait for cancellation so an in-flight command is dropped and killed.
    let _ = discovery.await;
    result
}

async fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    receiver: &mut watch::Receiver<AppState>,
) -> io::Result<()> {
    let mut state = AppState::default();
    let mut view = ui::View::Work;
    let mut scroll = 0_u16;
    let mut redraw = true;
    let mut input_tick = time::interval(Duration::from_millis(100));
    input_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        if redraw {
            terminal.draw(|frame| match view {
                ui::View::Work => ui::render_work(frame, &state, &mut scroll),
                ui::View::Sessions => ui::render(frame, &state.discovery, &mut scroll),
            })?;
            redraw = false;
        }
        tokio::select! {
            changed = receiver.changed() => {
                changed.map_err(|_| io::Error::other("Discovery task stopped unexpectedly"))?;
                state = receiver.borrow_and_update().clone();
                redraw = true;
            }
            _ = input_tick.tick() => {
                while event::poll(Duration::ZERO)? {
                    redraw = true;
                    if let Event::Key(key) = event::read()? {
                        if key.kind == KeyEventKind::Release {
                            continue;
                        }
                        match key.code {
                            KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(()),
                            KeyCode::Down => scroll = scroll.saturating_add(1),
                            KeyCode::Up => scroll = scroll.saturating_sub(1),
                            KeyCode::PageDown => scroll = scroll.saturating_add(terminal.size()?.height.saturating_sub(3)),
                            KeyCode::PageUp => scroll = scroll.saturating_sub(terminal.size()?.height.saturating_sub(3)),
                            KeyCode::Home => scroll = 0,
                            KeyCode::Char('w') => { view = ui::View::Work; scroll = 0; },
                            KeyCode::Char('s') => { view = ui::View::Sessions; scroll = 0; },
                            _ => {}
                        }
                    }
                }
            }
        }
    }
}
