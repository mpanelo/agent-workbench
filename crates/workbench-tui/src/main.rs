use std::{io, sync::Arc, time::Duration};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use tokio::{
    sync::watch,
    task::JoinSet,
    time::{self, MissedTickBehavior},
};
use workbench_core::{Engine, Snapshot, WorkItemState, validate_agent_input};

mod cli;
mod interaction;
mod ui;

type DiscoveryState = Option<Result<Snapshot, String>>;

#[derive(Clone, Debug, Default)]
struct AppState {
    discovery: DiscoveryState,
    work_items: Option<Result<Vec<WorkItemState>, String>>,
}

impl AppState {
    fn items(&self) -> &[WorkItemState] {
        self.work_items
            .as_ref()
            .and_then(|items| items.as_ref().ok())
            .map_or(&[], Vec::as_slice)
    }
}

enum RunExit {
    Quit,
    Focus(String),
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
    let engine = Arc::new(Engine::new(options.state_path().map_err(io::Error::other)?));
    match options.command {
        cli::Command::Register(item) => {
            let id = item.id.clone();
            engine.register_work_item(item).map_err(io::Error::other)?;
            println!("Registered {id}.");
            return Ok(());
        }
        cli::Command::List => {
            // Listing remains useful offline, with unavailable/UNKNOWN observations.
            let snapshot = engine.discover().await.ok();
            let items = engine
                .observe_work_item_states(snapshot.as_ref())
                .await
                .map_err(io::Error::other)?;
            if items.is_empty() {
                println!("No work items registered.");
            }
            for state in items {
                let item = state.item;
                println!(
                    "{}\t{}\t{}\t{} ({})\n  {}\n  Status: {}\n  Repository: {}\n  Workspace: {}\n  Branch: {}",
                    item.id,
                    state.status,
                    item.kind,
                    item.pane_id,
                    state.pane,
                    item.title,
                    state.status_detail,
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
    let mut interaction = interaction::Interaction::default();
    loop {
        let mut terminal = init_terminal()?;
        let result = run(&mut terminal, Arc::clone(&engine), &mut interaction).await;
        restore_terminal()?;
        match result? {
            RunExit::Quit => return Ok(()),
            RunExit::Focus(id) => {
                interaction.message = Some(match engine.focus_work_item(&id).await {
                    Ok(()) => format!(
                        "Opened {id}. Use tmux navigation to return, or detach when attached from outside."
                    ),
                    Err(error) => format!("Could not open {id}: {error}"),
                });
                interaction.reveal_selection = true;
            }
        }
    }
}

fn init_terminal() -> io::Result<ratatui::DefaultTerminal> {
    let terminal = match ratatui::try_init() {
        Ok(terminal) => terminal,
        Err(error) => {
            ratatui::restore();
            return Err(error);
        }
    };
    if let Err(error) = crossterm::execute!(io::stdout(), event::EnableBracketedPaste) {
        ratatui::restore();
        return Err(error);
    }
    Ok(terminal)
}

fn restore_terminal() -> io::Result<()> {
    let paste = crossterm::execute!(io::stdout(), event::DisableBracketedPaste);
    let restore = ratatui::try_restore();
    paste.and(restore)
}

async fn run(
    terminal: &mut ratatui::DefaultTerminal,
    engine: Arc<Engine>,
    interaction: &mut interaction::Interaction,
) -> io::Result<RunExit> {
    let (sender, mut receiver) = watch::channel::<AppState>(AppState::default());
    let discovery_engine = Arc::clone(&engine);
    let discovery = tokio::spawn(async move {
        let mut refresh = time::interval(Duration::from_secs(2));
        refresh.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            refresh.tick().await;
            let discovery = discovery_engine
                .discover()
                .await
                .map_err(|error| error.to_string());
            let work_items = discovery_engine
                .observe_work_item_states(discovery.as_ref().ok())
                .await
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
    let result = event_loop(terminal, &mut receiver, engine, interaction).await;
    discovery.abort();
    // Wait for cancellation so an in-flight command is dropped and killed.
    let _ = discovery.await;
    result
}

async fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    receiver: &mut watch::Receiver<AppState>,
    engine: Arc<Engine>,
    interaction: &mut interaction::Interaction,
) -> io::Result<RunExit> {
    let mut state = AppState::default();
    let mut view = ui::View::Work;
    let mut scroll = 0_u16;
    let mut redraw = true;
    let mut sends = JoinSet::new();
    let mut input_tick = time::interval(Duration::from_millis(100));
    input_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        if redraw {
            terminal.draw(|frame| match view {
                ui::View::Work => ui::render_work(frame, &state, &mut scroll, interaction),
                ui::View::Sessions => ui::render(frame, &state.discovery, &mut scroll),
            })?;
            redraw = false;
        }
        tokio::select! {
            changed = receiver.changed() => {
                changed.map_err(|_| io::Error::other("Discovery task stopped unexpectedly"))?;
                state = receiver.borrow_and_update().clone();
                interaction.sync(state.items());
                redraw = true;
            }
            completed = sends.join_next(), if !sends.is_empty() => {
                interaction.sending = false;
                match completed {
                    Some(Ok((id, result))) => match result {
                        Ok(()) => { interaction.message = Some(format!("Reply submitted to {id}.")); interaction.draft = None; }
                        Err(error) => interaction.message = Some(format!("Reply to {id} failed: {error}. Not retried; check the pane before resending.")),
                    },
                    _ => interaction.message = Some("Input task stopped unexpectedly. Check the pane before resending.".into()),
                }
                redraw = true;
            }
            _ = input_tick.tick() => {
                while event::poll(Duration::ZERO)? {
                    redraw = true;
                    let event = event::read()?;
                    if let Event::Paste(text) = &event
                        && !interaction.sending
                        && let Some(draft) = &mut interaction.draft
                        && let Err(error) = draft.append(text)
                    {
                        interaction.message = Some(error);
                    }
                    if let Event::Key(key) = event {
                        if key.kind == KeyEventKind::Release {
                            continue;
                        }
                        if let Some(draft) = &mut interaction.draft {
                            match key.code {
                                KeyCode::Esc | KeyCode::Char('c') if key.code == KeyCode::Esc || key.modifiers.contains(KeyModifiers::CONTROL) => {
                                    if !interaction.sending { interaction.draft = None; interaction.message = None; }
                                }
                                KeyCode::Enter if !interaction.sending => {
                                    match validate_agent_input(&draft.text) {
                                        Err(error) => interaction.message = Some(error.to_string()),
                                        Ok(()) => {
                                            let id = draft.item_id.clone();
                                            let text = draft.text.clone();
                                            let engine = Arc::clone(&engine);
                                            interaction.sending = true;
                                            interaction.message = Some(format!("Sending reply to {id}…"));
                                            sends.spawn(async move { let result = engine.send_agent_input(&id, &text).await; (id, result) });
                                        }
                                    }
                                }
                                _ if !interaction.sending => if let Err(error) = draft.edit(key) { interaction.message = Some(error); },
                                _ => {}
                            }
                            continue;
                        }
                        match key.code {
                            KeyCode::Char('q') | KeyCode::Esc => return Ok(RunExit::Quit),
                            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(RunExit::Quit),
                            KeyCode::Down | KeyCode::Char('j') if matches!(view, ui::View::Work) => interaction.move_selection(state.items(), 1),
                            KeyCode::Up | KeyCode::Char('k') if matches!(view, ui::View::Work) => interaction.move_selection(state.items(), -1),
                            KeyCode::Enter if matches!(view, ui::View::Work) => {
                                if let Some(selected) = interaction.selected(state.items()) { return Ok(RunExit::Focus(selected.item.id.clone())); }
                                interaction.message = Some("Select a registered work item first.".into());
                            }
                            KeyCode::Char('r') if matches!(view, ui::View::Work) => interaction.begin_reply(state.items()),
                            KeyCode::Tab if matches!(view, ui::View::Work) => interaction.next_attention(state.items()),
                            KeyCode::Down => scroll = scroll.saturating_add(1),
                            KeyCode::Up => scroll = scroll.saturating_sub(1),
                            KeyCode::PageDown => scroll = scroll.saturating_add(terminal.size()?.height.saturating_sub(3)),
                            KeyCode::PageUp => scroll = scroll.saturating_sub(terminal.size()?.height.saturating_sub(3)),
                            KeyCode::Home => { scroll = 0; if matches!(view, ui::View::Work) { interaction.selected_id = None; interaction.sync(state.items()); } },
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
